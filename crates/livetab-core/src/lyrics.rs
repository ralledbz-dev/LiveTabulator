/*
===============================================================================
Division By Zeros (DBZS) - Rust-Lyrics-Pipeline
Modul: lyrics
Zweck: Lokale Worttranskription über eine optionale On-Device-Python-Workergrenze,
       Beatgrid-Mapping, Zeilenbildung und validierte Textkorrektur.
Sprache: Rust
Hinweise:
- Cloud-ASR ist nicht implementiert und wird nicht still als Fallback verwendet.
- Fehlt ein lokales Modell oder faster-whisper, liefert der Worker einen klaren Fehler.
===============================================================================
*/
use crate::domain::{MusicalPosition, TimeSignature};
use crate::project_model::LyricsLine;
use crate::song::LyricsWord;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Wortsegment eines lokalen Spracherkennungs-Workers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscribedWord {
    pub text: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub confidence: f32,
}

/// Zweck: Bewahrt Wortsegmente und die optionale Sprachkennung der lokalen Transkription.
/// Eingaben: Vom ASR-Backend gelieferte Wörter und erkannter oder angeforderter Sprachcode.
/// Ausgabe: Unveränderte Metadaten zur weiteren Lyrics-Ausrichtung.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionOutput {
    /// Zeitlich verankerte Wortsegmente.
    pub words: Vec<TranscribedWord>,
    /// ISO-Sprachkennung, sofern das lokale Backend sie zurückliefert.
    pub language: Option<String>,
}

/// Ergebnis einer lokalen Lyrics-Erkennung einschließlich Backendhinweisen.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsResult {
    pub words: Vec<LyricsWord>,
    pub lines: Vec<LyricsLine>,
    pub warnings: Vec<String>,
    pub language: Option<String>,
}

/// Beschreibt die lokale Grenze zu einem plattformspezifischen oder Python-basierten ASR-Worker.
pub trait LyricsBackend: Send + Sync {
    /// Zweck: Transkribiert eine Audiodatei lokal und gibt zeitlich verankerte Wortsegmente zurück.
    /// Eingaben: Absoluter Audio-Pfad und Sprachcode oder `None` für lokale Auto-Erkennung.
    /// Ausgabe: Wortsegmente samt optionaler Sprachkennung oder klare Meldung für fehlendes Modell/Runtimes/Audio.
    fn transcribe(
        &self,
        audio_path: &Path,
        language: Option<&str>,
    ) -> Result<TranscriptionOutput, String>;
}

/// Konkreter lokaler Adapter zu einem installierten Python-Worker und faster-whisper-Modellordner.
#[derive(Debug, Clone)]
pub struct PythonWorkerLyricsBackend {
    pub python_executable: PathBuf,
    pub worker_script: PathBuf,
    pub model_directory: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerRequest<'a> {
    audio_path: &'a str,
    model_path: &'a str,
    language: Option<&'a str>,
    word_timestamps: bool,
}

#[derive(Deserialize)]
struct WorkerResponse {
    ok: bool,
    #[serde(default)]
    words: Vec<TranscribedWord>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl LyricsBackend for PythonWorkerLyricsBackend {
    /// Zweck: Führt einen isolierten lokalen Python-Worker mit JSON-Request/Response aus.
    /// Eingaben: Absoluter Audiodateipfad und optionaler Sprachcode; Modell- und Workerpfad müssen lokal vorhanden sein.
    /// Ausgabe: Wortsegmente bei erfolgreichem Worker oder präziser Fehlertext ohne Cloud-Fallback.
    fn transcribe(
        &self,
        audio_path: &Path,
        language: Option<&str>,
    ) -> Result<TranscriptionOutput, String> {
        if !audio_path.is_file() {
            return Err(format!(
                "Audiodatei nicht gefunden: {}",
                audio_path.display()
            ));
        }
        if !self.worker_script.is_file() {
            return Err(format!(
                "Lokaler Lyrics-Worker fehlt: {}",
                self.worker_script.display()
            ));
        }
        if !self.model_directory.is_dir() {
            return Err(format!(
                "Lokales ASR-Modellverzeichnis fehlt: {}",
                self.model_directory.display()
            ));
        }
        if !audio_path.is_absolute() || !self.model_directory.is_absolute() {
            return Err("Audio- und Modellpfad müssen absolute lokale Pfade sein.".into());
        }
        let mut child = Command::new(&self.python_executable)
            .arg("-I")
            .arg(&self.worker_script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Python-Worker konnte nicht gestartet werden: {error}"))?;
        let audio_path_text = audio_path.to_string_lossy().into_owned();
        let model_path_text = self.model_directory.to_string_lossy().into_owned();
        let request = WorkerRequest {
            audio_path: &audio_path_text,
            model_path: &model_path_text,
            language,
            word_timestamps: true,
        };
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Python-Worker besitzt keinen Eingabekanal.".to_string())?;
        serde_json::to_writer(stdin, &request)
            .map_err(|error| format!("Lyrics-Request konnte nicht serialisiert werden: {error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("Python-Worker konnte nicht beendet werden: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Lokaler Lyrics-Worker endete mit Status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let response: WorkerResponse = serde_json::from_slice(&output.stdout).map_err(|error| {
            format!(
                "Lyrics-Worker lieferte ungültiges JSON: {error}; stderr={}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })?;
        if !response.ok {
            return Err(response.error.unwrap_or_else(|| {
                "Lokaler Lyrics-Worker meldete einen unbekannten Fehler.".into()
            }));
        }
        if response.words.is_empty() {
            return Err("Lokales ASR hat keine Wortsegmente erzeugt.".into());
        }
        Ok(TranscriptionOutput {
            words: response.words,
            language: response.language,
        })
    }
}

/// Zweck: Validiert ASR-Wörter und ordnet sie anhand von BPM und Taktart in Takt-/Beatpositionen ein.
/// Eingaben: Transkribierte Rohwörter, Viertelnoten-BPM und Taktart.
/// Ausgabe: `LyricsResult` mit validierten `LyricsWord`-Werten, Zeilen und ungültige Segmente zählender Warnung.
pub fn align_transcription(
    words: Vec<TranscribedWord>,
    bpm: f64,
    time_signature: TimeSignature,
) -> Result<LyricsResult, String> {
    align_transcription_with_language(words, None, bpm, time_signature)
}

/// Zweck: Validiert ASR-Wörter, ordnet sie musikalisch ein und bewahrt die erkannte Sprache.
/// Eingaben: Rohwörter, optionale Sprachkennung, Viertelnoten-BPM und Taktart.
/// Ausgabe: Beat-ausgerichtete `LyricsResult`-Daten oder ein Validierungsfehler.
pub fn align_transcription_with_language(
    words: Vec<TranscribedWord>,
    language: Option<String>,
    bpm: f64,
    time_signature: TimeSignature,
) -> Result<LyricsResult, String> {
    if !bpm.is_finite() || bpm <= 0.0 {
        return Err("Lyrics können ohne positives Tempo nicht auf Beats abgebildet werden.".into());
    }
    let mut ordered = words;
    ordered.sort_by(|left, right| left.start_seconds.total_cmp(&right.start_seconds));
    let mut valid = Vec::new();
    let mut rejected = 0usize;
    for word in ordered {
        if let Ok(item) = LyricsWord::with_confidence(
            word.text,
            word.start_seconds,
            word.end_seconds,
            word.confidence,
        ) {
            valid.push(item);
        } else {
            rejected += 1;
        }
    }
    if valid.is_empty() {
        return Err("Das lokale ASR lieferte keine gültigen Wort-Zeitstempel.".into());
    }
    let lines = build_lines(&valid, bpm, time_signature)?;
    let warnings = if rejected == 0 {
        Vec::new()
    } else {
        vec![format!(
            "{rejected} ungültige ASR-Wortsegmente wurden verworfen."
        )]
    };
    Ok(LyricsResult {
        words: valid,
        lines,
        warnings,
        language,
    })
}

/// Zweck: Gruppiert zeitlich zusammenhängende Wörter bis zu Satzzeichen, Pausen oder maximal zwölf Wörtern.
/// Eingaben: Geordnete Lyrics-Wörter, Songtempo und Taktart.
/// Ausgabe: Lyrics-Zeilen mit Text, Start-/Endzeit und zugehörigen Taktpositionen.
pub fn build_lines(
    words: &[LyricsWord],
    bpm: f64,
    time_signature: TimeSignature,
) -> Result<Vec<LyricsLine>, String> {
    if !bpm.is_finite() || bpm <= 0.0 {
        return Err("Zeilenbildung benötigt ein positives BPM.".into());
    }
    let mut ordered = words.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
    let mut groups: Vec<Vec<&LyricsWord>> = Vec::new();
    for word in ordered {
        let should_break = groups
            .last()
            .and_then(|group| group.last())
            .is_some_and(|previous| {
                word.start_seconds() - previous.end_seconds() > 0.65
                    || previous
                        .text()
                        .chars()
                        .last()
                        .is_some_and(|character| matches!(character, '.' | '!' | '?'))
                    || group_len(groups.last()) >= 12
            });
        if groups.is_empty() || should_break {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(word);
    }
    groups
        .into_iter()
        .map(|group| {
            let first = group.first().unwrap();
            let last = group.last().unwrap();
            let start = MusicalPosition::at(first.start_seconds(), bpm, time_signature)
                .map_err(|error| error.to_string())?;
            let end = MusicalPosition::at(last.end_seconds(), bpm, time_signature)
                .map_err(|error| error.to_string())?;
            Ok(LyricsLine {
                text: group
                    .iter()
                    .map(|word| word.text())
                    .collect::<Vec<_>>()
                    .join(" "),
                start_seconds: first.start_seconds(),
                end_seconds: last.end_seconds(),
                start_bar: start.bar,
                end_bar: end.bar,
                words: group.into_iter().cloned().collect(),
            })
        })
        .collect()
}

/// Zweck: Liefert die Zahl der bereits gruppierten Wörter ohne Eigentumsübernahme.
/// Eingaben: Optionale letzte Lyrics-Gruppe.
/// Ausgabe: Wortzahl oder null, wenn noch keine Gruppe vorhanden ist.
fn group_len(group: Option<&Vec<&LyricsWord>>) -> usize {
    group.map_or(0, Vec::len)
}

/// Zweck: Formatiert Lyrics-Zeilen als LRC mit Zeitmarke im Format `[mm:ss.xx]`.
/// Eingaben: Lyrics-Zeilen in beliebiger Reihenfolge.
/// Ausgabe: Chronologisch sortierter, zeilenbasierter LRC-Text.
pub fn export_lrc(lines: &[LyricsLine]) -> String {
    let mut ordered = lines.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.start_seconds.total_cmp(&right.start_seconds));
    ordered
        .into_iter()
        .map(|line| format!("[{}]{}", lrc_timestamp(line.start_seconds), line.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Zweck: Formatiert Lyrics-Zeilen als SubRip-Untertitel mit Start-/Endzeitstempeln.
/// Eingaben: Lyrics-Zeilen in beliebiger Reihenfolge.
/// Ausgabe: Gültiger SRT-Text mit nummerierten Blöcken.
pub fn export_srt(lines: &[LyricsLine]) -> String {
    let mut ordered = lines.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.start_seconds.total_cmp(&right.start_seconds));
    ordered
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            format!(
                "{}\n{} --> {}\n{}",
                index + 1,
                srt_timestamp(line.start_seconds),
                srt_timestamp(line.end_seconds),
                line.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Zweck: Formatiert eine nichtnegative Zeit als LRC-Zeitmarke auf Hundertstelsekunden.
/// Eingaben: Zeit in Sekunden.
/// Ausgabe: `mm:ss.xx`; ungültige Zeit wird als Null interpretiert.
fn lrc_timestamp(seconds: f64) -> String {
    let total = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let minutes = (total / 60.0).floor() as u64;
    let rest = total - minutes as f64 * 60.0;
    format!("{minutes:02}:{rest:05.2}")
}

/// Zweck: Formatiert eine nichtnegative Zeit als SRT-Zeitmarke mit Millisekunden.
/// Eingaben: Zeit in Sekunden.
/// Ausgabe: `hh:mm:ss,mmm`; ungültige Zeit wird als Null interpretiert.
fn srt_timestamp(seconds: f64) -> String {
    let total_ms = if seconds.is_finite() {
        (seconds.max(0.0) * 1000.0).round() as u64
    } else {
        0
    };
    let hours = total_ms / 3_600_000;
    let minutes = total_ms / 60_000 % 60;
    let seconds = total_ms / 1000 % 60;
    let millis = total_ms % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

/// Zweck: Ersetzt ein einzelnes Lyrics-Wort durch eine validierte Nutzerkorrektur.
/// Eingaben: Wortliste, nullbasierter Wortindex und neuer sichtbarer Text.
/// Ausgabe: Neue Wortliste oder Fehler bei leerem/außerhalb liegendem Index.
pub fn correct_word(
    words: &[LyricsWord],
    index: usize,
    replacement: &str,
) -> Result<Vec<LyricsWord>, String> {
    let mut result = words.to_vec();
    let current = result
        .get(index)
        .ok_or_else(|| "Lyrics-Wortindex liegt außerhalb der Liste.".to_string())?;
    let corrected = LyricsWord::with_confidence(
        replacement,
        current.start_seconds(),
        current.end_seconds(),
        current.confidence(),
    )
    .map_err(|error| error.to_string())?;
    result[index] = corrected;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft Beatmapping, automatische Zeilenbildung und LRC-/SRT-Zeitformate.
    /// Eingaben: Keine; verwendet zwei zusammenhängende deutsche Wörter mit Zeitstempeln.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Zeitmarken und Textgruppierung.
    #[test]
    fn aligns_words_and_formats_subtitles() {
        let words = vec![
            TranscribedWord {
                text: "Hallo".into(),
                start_seconds: 0.2,
                end_seconds: 0.6,
                confidence: 0.9,
            },
            TranscribedWord {
                text: "Welt!".into(),
                start_seconds: 0.7,
                end_seconds: 1.1,
                confidence: 0.8,
            },
        ];
        let result = align_transcription(words, 120.0, TimeSignature::default()).unwrap();
        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.lines[0].text, "Hallo Welt!");
        assert!(export_lrc(&result.lines).starts_with("[00:00.20]"));
        assert!(export_srt(&result.lines).contains("00:00:00,200 --> 00:00:01,100"));
    }

    /// Zweck: Sichert Korrekturvalidierung und den Fehlerpfad für leere Transkriptionen ab.
    /// Eingaben: Keine; korrigiert ein Wort und fordert ein leeres Ergebnis an.
    /// Ausgabe: Keine Rückgabe; Assertions prüfen geänderten Text und erklärenden Fehler.
    #[test]
    fn corrects_word_text_and_rejects_empty_result() {
        let word = LyricsWord::new("fire", 0.0, 0.5).unwrap();
        let corrected = correct_word(&[word], 0, "feuer").unwrap();
        assert_eq!(corrected[0].text(), "feuer");
        assert!(align_transcription(Vec::new(), 120.0, TimeSignature::default()).is_err());
    }

    /// Zweck: Sichert, dass eine vom lokalen Worker erkannte Sprache das Beatmapping überlebt.
    /// Eingaben: Ein gültiges deutsches Wortsegment und Sprachkennung `de`.
    /// Ausgabe: Keine Rückgabe; die Ergebniskennung muss unverändert `de` lauten.
    #[test]
    fn preserves_detected_language_during_alignment() {
        let words = vec![TranscribedWord {
            text: "Hallo".into(),
            start_seconds: 0.2,
            end_seconds: 0.6,
            confidence: 0.9,
        }];
        let result = align_transcription_with_language(
            words,
            Some("de".into()),
            120.0,
            TimeSignature::default(),
        )
        .unwrap();
        assert_eq!(result.language.as_deref(), Some("de"));
    }
}
