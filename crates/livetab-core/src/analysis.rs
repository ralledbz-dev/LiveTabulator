/*
===============================================================================
Division By Zeros (DBZS) - Rust-Analysepipeline
Modul: analysis
Zweck: Offline-/Live-Frames über Feature Extraction, Chroma, Pitch, Akkord- und
       Tonartklassifikation sowie Beatgrid zu editierbaren Ereignissen bündeln.
Sprache: Rust
Hinweise:
- Live-DSP ist rein lokal und läuft unabhängig von LLM-/Lyrics-Worker-Threads.
- Ereignisse mit unzureichender Energie oder geringer Konfidenz werden nicht erfunden.
===============================================================================
*/
use crate::audio::WaveFile;
use crate::beat::{BeatGrid, estimate_beat_grid};
use crate::domain::{Chord, ChordQuality, KeyMode, KeySignature, NoteName, TimeSignature};
use crate::dsp::{estimate_pitch, extract_chroma};
use crate::music::{KeyEstimate, estimate_chord, estimate_key_candidates};
use crate::project_model::BeatEvent;
use crate::song::{ChordEvent, NoteEvent};

/// Konfigurierbare Framegröße und Konfidenzgrenzen der Live-/Offlineanalyse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisConfig {
    pub frame_samples: usize,
    pub hop_samples: usize,
    pub minimum_chord_confidence: f32,
    pub minimum_pitch_confidence: f32,
}

impl Default for AnalysisConfig {
    /// Zweck: Liefert robuste Standardparameter für circa 93-ms-Fenster und 46-ms-Hops bei 44,1 kHz.
    /// Eingaben: Keine.
    /// Ausgabe: Analyseparameter mit begrenzten Konfidenzschwellen.
    fn default() -> Self {
        Self {
            frame_samples: 4096,
            hop_samples: 2048,
            minimum_chord_confidence: 0.24,
            minimum_pitch_confidence: 0.72,
        }
    }
}

/// Ergebnis eines Audioanalysepasses samt musikalischen Events und Diagnosewerten.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisResult {
    pub chords: Vec<ChordEvent>,
    pub notes: Vec<NoteEvent>,
    pub key: Option<KeySignature>,
    pub key_confidence: Option<f32>,
    pub key_alternatives: Vec<KeyEstimate>,
    pub beat_grid: Option<BeatGrid>,
    pub analyzed_frames: usize,
    pub duration_seconds: f64,
    pub warnings: Vec<String>,
}

/// Zweck: Analysiert WAV-Audio mit dem lokalen Rust-DSP und baut geglättete Songereignisse.
/// Eingaben: Dekodierte WAV-Daten, Taktart und `AnalysisConfig`; Audio wird für Chroma auf Mono abgeleitet.
/// Ausgabe: Akkord-/Notenereignisse, Tonart, Beatgrid, Framezahl, Dauer und verständliche Warnungen.
pub fn analyze_wave(
    wave: &WaveFile,
    time_signature: TimeSignature,
    config: AnalysisConfig,
) -> Result<AnalysisResult, String> {
    analyze_wave_with_cancel(wave, time_signature, config, || false)
}

/// Zweck: Führt denselben lokalen DSP-Pass aus und prüft zwischen Frames ein kooperatives Abbruchsignal.
/// Eingaben: WAV-Daten, Taktart, Analyseprofil und `is_cancelled`-Abfrage.
/// Ausgabe: Vollständiges Analyseergebnis oder eine sichtbare Abbruch-/Validierungsmeldung.
pub fn analyze_wave_with_cancel<F>(
    wave: &WaveFile,
    time_signature: TimeSignature,
    config: AnalysisConfig,
    mut is_cancelled: F,
) -> Result<AnalysisResult, String>
where
    F: FnMut() -> bool,
{
    if wave.format.sample_rate == 0 || wave.format.channels == 0 {
        return Err("WAV-Format enthält keine gültige Sample-Rate oder Kanalzahl.".into());
    }
    if config.frame_samples < 256
        || config.hop_samples == 0
        || config.hop_samples > config.frame_samples
    {
        return Err("Analysefenster oder Hopgröße ist ungültig.".into());
    }
    let mono = wave.mono_samples();
    if mono.len() < config.frame_samples / 2 {
        return Err("Die Audiodatei ist für eine stabile Analyse zu kurz.".into());
    }
    let duration_seconds = mono.len() as f64 / f64::from(wave.format.sample_rate);
    let mut frames = Vec::new();
    let mut key_chroma = [0.0f32; 12];
    let mut cursor = 0usize;
    while cursor < mono.len() {
        let end = (cursor + config.frame_samples).min(mono.len());
        if end - cursor >= config.frame_samples / 2 {
            let mut samples = mono[cursor..end].to_vec();
            samples.resize(config.frame_samples, 0.0);
            let chroma = extract_chroma(&samples, wave.format.sample_rate);
            if chroma.iter().any(|value| *value > 0.0) {
                for (sum, value) in key_chroma.iter_mut().zip(chroma) {
                    *sum += value;
                }
                frames.push((cursor, chroma));
            }
        }
        cursor = cursor.saturating_add(config.hop_samples);
    }
    let key_alternatives = estimate_key_candidates(&key_chroma, 3);
    let global_key = key_alternatives.first().cloned();
    let key = global_key
        .as_ref()
        .and_then(|estimate| parse_key(estimate.tonic, estimate.mode));
    let key_confidence = global_key
        .as_ref()
        .map(|estimate| estimate.confidence.clamp(0.0, 1.0));
    let mut chords = Vec::new();
    let mut notes = Vec::new();
    let mut current: Option<(Chord, f64, f32, usize)> = None;
    for (start_sample, chroma) in &frames {
        if is_cancelled() {
            return Err("Analysejob wurde abgebrochen.".into());
        }
        let start = *start_sample as f64 / f64::from(wave.format.sample_rate);
        if let Some(estimate) = estimate_chord(chroma)
            .filter(|value| value.confidence >= config.minimum_chord_confidence)
        {
            if let Some(chord) = parse_chord(estimate.root, estimate.quality) {
                match current {
                    Some((old, old_start, sum, count)) if old == chord => {
                        current = Some((old, old_start, sum + estimate.confidence, count + 1))
                    }
                    Some((old, old_start, sum, count)) => {
                        let old_end = start.max(
                            old_start
                                + config.hop_samples as f64 / f64::from(wave.format.sample_rate),
                        );
                        if let Ok(event) = ChordEvent::with_confidence(
                            old,
                            old_start,
                            old_end,
                            sum / count.max(1) as f32,
                        ) {
                            chords.push(event);
                        }
                        current = Some((chord, start, estimate.confidence, 1));
                    }
                    None => current = Some((chord, start, estimate.confidence, 1)),
                }
            }
        }
        let start_offset = *start_sample;
        let end = (start_offset + config.frame_samples).min(mono.len());
        if let Some(pitch) = estimate_pitch(
            &mono[start_offset..end],
            wave.format.sample_rate,
            70.0,
            1_100.0,
        )
        .filter(|pitch| pitch.confidence >= config.minimum_pitch_confidence)
        {
            let duration = config.hop_samples as f64 / f64::from(wave.format.sample_rate);
            if let Ok(note) =
                NoteEvent::with_metadata(pitch.midi, start, duration, 88, pitch.confidence)
            {
                if notes.last().is_none_or(|previous: &NoteEvent| {
                    previous.midi() != note.midi()
                        || (note.start_seconds()
                            - (previous.start_seconds() + previous.duration_seconds()))
                        .abs()
                            > duration * 1.5
                }) {
                    notes.push(note);
                } else if let Some(previous) = notes.last_mut() {
                    let extended = previous.duration_seconds() + duration;
                    if let Ok(replacement) = NoteEvent::with_metadata(
                        previous.midi(),
                        previous.start_seconds(),
                        extended,
                        previous.velocity(),
                        previous.confidence().max(pitch.confidence),
                    ) {
                        *previous = replacement;
                    }
                }
            }
        }
    }
    if let Some((chord, start, sum, count)) = current {
        if duration_seconds > start {
            if let Ok(event) = ChordEvent::with_confidence(
                chord,
                start,
                duration_seconds,
                sum / count.max(1) as f32,
            ) {
                chords.push(event);
            }
        }
    }
    merge_short_chord_fluctuations(&mut chords, 0.12);
    let beat_grid = if duration_seconds >= 0.5 {
        estimate_beat_grid(
            &mono,
            wave.format.sample_rate,
            duration_seconds,
            time_signature,
        )
        .ok()
    } else {
        None
    };
    let warnings = if frames.is_empty() {
        vec!["Kein auswertbares harmonisches Signal erkannt; Pegel prüfen.".into()]
    } else {
        Vec::new()
    };
    Ok(AnalysisResult {
        chords,
        notes,
        key,
        key_confidence,
        key_alternatives,
        beat_grid,
        analyzed_frames: frames.len(),
        duration_seconds,
        warnings,
    })
}

/// Zweck: Wandelt ein Akkordschätzergebnis in den typsicheren Core-Akkord um.
/// Eingaben: Kanonischer Notenname und Akkordqualität des Klassifizierers.
/// Ausgabe: `Chord` mit passendem Grundton oder `None` bei unbekannter Schreibweise.
fn parse_chord(root: &str, quality: ChordQuality) -> Option<Chord> {
    let pitch = match root {
        "C" => 0,
        "C♯" | "C#" => 1,
        "D" => 2,
        "D♯" | "D#" => 3,
        "E" => 4,
        "F" => 5,
        "F♯" | "F#" => 6,
        "G" => 7,
        "G♯" | "G#" => 8,
        "A" => 9,
        "A♯" | "A#" => 10,
        "B" => 11,
        _ => return None,
    };
    Some(Chord {
        root: NoteName::from_pitch_class(pitch),
        quality,
    })
}

/// Zweck: Wandelt eine Tonartschätzung in den typsicheren Core-Tonartwert um.
/// Eingaben: Grundtonzeichenkette und Dur-/Moll-Modus.
/// Ausgabe: `KeySignature` oder `None`, falls der Grundton nicht kanonisch ist.
fn parse_key(tonic: &str, mode: KeyMode) -> Option<KeySignature> {
    let quality = match tonic {
        "C" => 0,
        "C♯" | "C#" => 1,
        "D" => 2,
        "D♯" | "D#" => 3,
        "E" => 4,
        "F" => 5,
        "F♯" | "F#" => 6,
        "G" => 7,
        "G♯" | "G#" => 8,
        "A" => 9,
        "A♯" | "A#" => 10,
        "B" => 11,
        _ => return None,
    };
    Some(KeySignature {
        root: NoteName::from_pitch_class(quality),
        mode,
    })
}

/// Zweck: Fasst sehr kurze, eingeschobene Akkordwechsel in die musikalisch stabilere Nachbarschaft ein.
/// Eingaben: Akkordereignisse in Zeitreihenfolge und minimale Ereignisdauer in Sekunden.
/// Ausgabe: Keine Rückgabe; sehr kurze Mittelsegmente werden bei identischen Nachbarn gemerged.
fn merge_short_chord_fluctuations(chords: &mut Vec<ChordEvent>, minimum_seconds: f64) {
    if chords.len() < 3 {
        return;
    }
    let mut index = 1;
    while index + 1 < chords.len() {
        if chords[index].duration_seconds() < minimum_seconds
            && chords[index - 1].chord() == chords[index + 1].chord()
        {
            let previous = chords[index - 1];
            let next = chords[index + 1];
            if let Ok(merged) = ChordEvent::with_confidence(
                previous.chord(),
                previous.start_seconds(),
                next.end_seconds(),
                previous.confidence().min(next.confidence()),
            ) {
                chords[index - 1] = merged;
                chords.remove(index + 1);
                chords.remove(index);
                continue;
            }
        }
        index += 1;
    }
}

/// Zweck: Überführt ein Beatgrid in die Projekt-Beatereignisse.
/// Eingaben: `grid` – erkanntes Tempo- und Downbeat-Grid.
/// Ausgabe: Chronologische BeatEvent-Liste für Songmodell und UI.
pub fn beat_events(grid: &BeatGrid) -> Vec<BeatEvent> {
    grid.beats.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioFormat, WaveFile};

    /// Zweck: Prüft, dass eine künstliche A-Moll-Akkord-WAV-Analyse Akkord- und Tonartdaten erzeugt.
    /// Eingaben: Keine; synthetisiert A3-C4-E4 mit zeitlichem Verlauf bei 44,1 kHz.
    /// Ausgabe: Keine Rückgabe; Assertions prüfen erkannte Musikereignisse und Dauer.
    #[test]
    fn analyzes_synthetic_minor_chord_audio() {
        let rate = 44_100u32;
        let samples = (0..rate as usize * 2)
            .map(|index| {
                let t = index as f32 / rate as f32;
                [220.0, 261.63, 329.63]
                    .iter()
                    .map(|frequency| (std::f32::consts::TAU * frequency * t).sin())
                    .sum::<f32>()
                    * 0.16
            })
            .collect::<Vec<_>>();
        let wave = WaveFile {
            format: AudioFormat {
                sample_rate: rate,
                channels: 1,
                bits_per_sample: 32,
                is_float: true,
            },
            samples,
        };
        let result =
            analyze_wave(&wave, TimeSignature::default(), AnalysisConfig::default()).unwrap();
        assert!(result.duration_seconds > 1.9);
        assert!(!result.chords.is_empty());
        assert!(
            result
                .chords
                .iter()
                .any(|event| event.chord().root == NoteName::A
                    && event.chord().quality == ChordQuality::Minor)
        );
    }

    /// Zweck: Verifiziert, dass zu kurze Audiodateien statt einer Scheingenauigkeit einen klaren Fehler liefern.
    /// Eingaben: Keine; verwendet einen sehr kurzen stillen Frame.
    /// Ausgabe: Keine Rückgabe; Assertion erwartet Ablehnung unterhalb der Minimaldauer.
    #[test]
    fn rejects_audio_too_short_for_stable_analysis() {
        let wave = WaveFile {
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 1,
                bits_per_sample: 32,
                is_float: true,
            },
            samples: vec![0.0; 100],
        };
        assert!(
            analyze_wave(&wave, TimeSignature::default(), AnalysisConfig::default())
                .unwrap_err()
                .contains("zu kurz")
        );
    }

    /// Zweck: Prüft, dass der DSP-Pass zwischen Frames kooperativ mit einer expliziten Meldung abbricht.
    /// Eingaben: Mehrsekündige synthetische WAV und ein Callback, der nach dem ersten Frame Abbruch anfordert.
    /// Ausgabe: Keine Rückgabe; Assertion erwartet den klaren Abbruchfehler.
    #[test]
    fn deep_analysis_observes_cancellation_between_frames() {
        let rate = 8_000u32;
        let samples = (0..rate as usize * 3)
            .map(|index| (std::f32::consts::TAU * 220.0 * index as f32 / rate as f32).sin() * 0.2)
            .collect();
        let wave = WaveFile {
            format: AudioFormat {
                sample_rate: rate,
                channels: 1,
                bits_per_sample: 32,
                is_float: true,
            },
            samples,
        };
        let mut checks = 0usize;
        let result = analyze_wave_with_cancel(
            &wave,
            TimeSignature::default(),
            AnalysisConfig::default(),
            || {
                checks += 1;
                checks > 1
            },
        );
        assert!(result.unwrap_err().contains("abgebrochen"));
    }
}
