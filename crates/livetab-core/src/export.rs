/*
===============================================================================
Division By Zeros (DBZS) - lokale Export-/Projektdateien
Modul: export
Zweck: Exportiert Songdaten als TXT, LRC, SRT, Standard-MIDI, JSON oder
       portables `.dbztab`-ZIP-Projekt mit optional eingebettetem Audio.
Sprache: Rust
Hinweise:
- Alle Ausgaben werden lokal geschrieben; keine Daten verlassen das Gerät.
- ZIP-Import liest nur definierte Einträge und setzt Größenobergrenzen.
===============================================================================
*/
use crate::lyrics::{export_lrc, export_srt};
use crate::project_model::SongProject;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MAX_PROJECT_JSON_BYTES: u64 = 32 * 1024 * 1024;
const MAX_EMBEDDED_AUDIO_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Zweck: Erzeugt eine lesbare Textübersicht aller editierbaren Songdaten.
/// Eingaben: Validiertes oder im Arbeitsspeicher bearbeitetes `SongProject`.
/// Ausgabe: Klartext mit Metadaten, Akkorden, Lyrics und Gitarrentabs.
pub fn export_text(project: &SongProject) -> String {
    let mut output = format!(
        "{} — {}\nTempo: {:.1} BPM · Takt: {}\nTonart: {}\n\nAkkorde:\n",
        project.title,
        project.artist,
        project.bpm,
        project.time_signature,
        project
            .key_signature
            .map(|key| key.name())
            .unwrap_or_else(|| "unbekannt".into())
    );
    for chord in &project.chords {
        output.push_str(&format!(
            "{:>7.2}s–{:>7.2}s  {}  Confidence {:.0}%\n",
            chord.start_seconds(),
            chord.end_seconds(),
            chord.chord().name(),
            chord.confidence() * 100.0
        ));
    }
    output.push_str("\nLyrics:\n");
    for line in &project.lyrics_lines {
        output.push_str(&format!("[{:>7.2}s] {}\n", line.start_seconds, line.text));
    }
    for track in &project.tab_tracks {
        output.push_str(&format!("\nTab: {} ({})\n", track.name, track.tuning.name));
        for note in &track.notes {
            output.push_str(&format!(
                "{}s · Saite {} · Bund {} · MIDI {}\n",
                note.position.start_seconds(),
                note.position.string_number(),
                note.position.fret(),
                note.midi
            ));
        }
    }
    output
}

/// Zweck: Exportiert das Songprojekt abhängig vom Format in eine lokale Datei.
/// Eingaben: `project` – Songzustand; `path` – Zieldatei; `format` – txt/lrc/srt/mid/midi/json/dbztab.
/// Ausgabe: Geschriebener Zielpfad oder verständlicher Format-/Dateifehler.
pub fn export_project(
    project: &SongProject,
    path: impl AsRef<Path>,
    format: &str,
) -> Result<PathBuf, String> {
    project.validate()?;
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Exportordner konnte nicht erstellt werden: {error}"))?;
    }
    match format.to_ascii_lowercase().as_str() {
        "txt" => fs::write(path, export_text(project))
            .map_err(|error| format!("TXT-Export fehlgeschlagen: {error}"))?,
        "lrc" => fs::write(path, export_lrc(&project.lyrics_lines))
            .map_err(|error| format!("LRC-Export fehlgeschlagen: {error}"))?,
        "srt" => fs::write(path, export_srt(&project.lyrics_lines))
            .map_err(|error| format!("SRT-Export fehlgeschlagen: {error}"))?,
        "mid" | "midi" => {
            let bytes = export_midi_bytes(project)?;
            fs::write(path, bytes)
                .map_err(|error| format!("MIDI-Export fehlgeschlagen: {error}"))?;
        }
        "json" => {
            let bytes = serde_json::to_vec_pretty(project)
                .map_err(|error| format!("JSON-Export fehlgeschlagen: {error}"))?;
            fs::write(path, bytes)
                .map_err(|error| format!("JSON-Export fehlgeschlagen: {error}"))?;
        }
        "dbztab" => save_dbztab(project, path)?,
        _ => return Err(format!("Exportformat {format} wird nicht unterstützt.")),
    }
    Ok(path.to_path_buf())
}

/// Zweck: Speichert ein portables `.dbztab`-Archiv aus Projekt-JSON und optionaler Audiodatei.
/// Eingaben: Valides Songprojekt und Zielpfad; nur eine vorhandene lokale Audioquelle wird eingebettet.
/// Ausgabe: `Ok(())` nach Abschluss oder ein JSON-/ZIP-/Dateifehler.
pub fn save_dbztab(project: &SongProject, path: impl AsRef<Path>) -> Result<(), String> {
    project.validate()?;
    let path = path.as_ref();
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("dbztab"))
    {
        return Err("Das Projektarchiv benötigt die Endung .dbztab.".into());
    }
    let output = File::create(path)
        .map_err(|error| format!("Projektarchiv kann nicht erstellt werden: {error}"))?;
    let mut archive = ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut portable = project.clone();
    let embedded_audio = project
        .audio_path
        .as_ref()
        .filter(|audio| audio.is_file())
        .cloned();
    portable.audio_path = embedded_audio
        .as_ref()
        .map(|_| PathBuf::from("audio/source.wav"));
    archive
        .start_file("project.json", options)
        .map_err(|error| format!("Projektmanifest konnte nicht begonnen werden: {error}"))?;
    serde_json::to_writer_pretty(&mut archive, &portable)
        .map_err(|error| format!("Projektmanifest konnte nicht geschrieben werden: {error}"))?;
    if let Some(audio_path) = embedded_audio {
        let metadata = fs::metadata(&audio_path)
            .map_err(|error| format!("Audiodatei konnte nicht geprüft werden: {error}"))?;
        if metadata.len() > MAX_EMBEDDED_AUDIO_BYTES {
            return Err("Eingebettete Audiodatei überschreitet zwei GiB.".into());
        }
        archive
            .start_file("audio/source.wav", options)
            .map_err(|error| format!("Audioeintrag konnte nicht begonnen werden: {error}"))?;
        let mut audio = File::open(audio_path)
            .map_err(|error| format!("Audiodatei konnte nicht geöffnet werden: {error}"))?;
        std::io::copy(&mut audio, &mut archive)
            .map_err(|error| format!("Audiodatei konnte nicht eingebettet werden: {error}"))?;
    }
    archive
        .finish()
        .map_err(|error| format!("Projektarchiv konnte nicht abgeschlossen werden: {error}"))?;
    Ok(())
}

/// Zweck: Lädt ein `.dbztab`-Archiv, validiert das Projektmanifest und extrahiert nur den bekannten Audioeintrag.
/// Eingaben: Archivpfad und lokaler Zielordner für gegebenenfalls eingebettete Audiodaten.
/// Ausgabe: Geprüftes Songprojekt mit lokaler Audioreferenz oder eine verständliche Archiv-/Validierungsmeldung.
pub fn load_dbztab(
    path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<SongProject, String> {
    let file = File::open(path)
        .map_err(|error| format!("Projektarchiv konnte nicht geöffnet werden: {error}"))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("Projektarchiv ist kein gültiges ZIP: {error}"))?;
    let project_entry = archive
        .by_name("project.json")
        .map_err(|error| format!("Projektarchiv enthält kein project.json: {error}"))?;
    if project_entry.size() > MAX_PROJECT_JSON_BYTES {
        return Err("Projektmanifest überschreitet 32 MiB.".into());
    }
    let mut bytes = Vec::with_capacity(project_entry.size() as usize);
    project_entry
        .take(MAX_PROJECT_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Projektmanifest konnte nicht gelesen werden: {error}"))?;
    let mut project: SongProject = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Projektmanifest ist ungültig: {error}"))?;
    project.validate()?;
    let has_audio = archive.file_names().any(|name| name == "audio/source.wav");
    let audio_size = if has_audio {
        Some(
            archive
                .by_name("audio/source.wav")
                .map_err(|error| format!("Audioeintrag konnte nicht geprüft werden: {error}"))?
                .size(),
        )
    } else {
        None
    };
    if let Some(size) = audio_size {
        if size > MAX_EMBEDDED_AUDIO_BYTES {
            return Err("Eingebettetes Audio überschreitet zwei GiB.".into());
        }
        let destination = destination.as_ref().join("audio");
        fs::create_dir_all(&destination)
            .map_err(|error| format!("Audioziel konnte nicht angelegt werden: {error}"))?;
        let audio_path = destination.join("source.wav");
        let input = archive
            .by_name("audio/source.wav")
            .map_err(|error| format!("Audioeintrag konnte nicht geöffnet werden: {error}"))?;
        let mut output = File::create(&audio_path)
            .map_err(|error| format!("Audioziel kann nicht geschrieben werden: {error}"))?;
        let copied = std::io::copy(&mut input.take(MAX_EMBEDDED_AUDIO_BYTES + 1), &mut output)
            .map_err(|error| format!("Audioeintrag konnte nicht extrahiert werden: {error}"))?;
        if copied > MAX_EMBEDDED_AUDIO_BYTES {
            return Err("Audioeintrag überschreitet die zulässige Größe.".into());
        }
        project.audio_path = Some(audio_path);
    } else {
        project.audio_path = None;
    }
    Ok(project)
}

#[derive(Clone, Copy)]
struct MidiEvent {
    tick: u64,
    kind: u8,
    pitch: u8,
    velocity: u8,
}

/// Zweck: Erzeugt einen SMF-Format-0-Standard-MIDI-Stream mit Tempo und Noten-On/Off-Ereignissen.
/// Eingaben: Songprojekt mit BPM, Noten- und/oder Akkordereignissen.
/// Ausgabe: MIDI-Dateibytes oder Fehler bei ungültigem Tempo/Dateilängen.
pub fn export_midi_bytes(project: &SongProject) -> Result<Vec<u8>, String> {
    if !project.bpm.is_finite() || project.bpm <= 0.0 {
        return Err("MIDI-Export benötigt ein positives Songtempo.".into());
    }
    let mut events = Vec::new();
    if !project.notes.is_empty() {
        for note in &project.notes {
            midi_note_events(
                &mut events,
                project.bpm,
                note.start_seconds(),
                note.duration_seconds(),
                note.midi(),
                note.velocity(),
            );
        }
    } else {
        for chord in &project.chords {
            let notes = chord
                .chord()
                .midi_notes(4)
                .map_err(|error| error.to_string())?;
            for midi in notes {
                midi_note_events(
                    &mut events,
                    project.bpm,
                    chord.start_seconds(),
                    chord.duration_seconds(),
                    midi,
                    80,
                );
            }
        }
    }
    events.sort_by_key(|event| (event.tick, event.kind));
    let micros = (60_000_000.0 / project.bpm)
        .round()
        .clamp(1.0, 0xFF_FFFF as f64) as u32;
    let mut track = vec![
        0x00,
        0xFF,
        0x51,
        0x03,
        (micros >> 16) as u8,
        (micros >> 8) as u8,
        micros as u8,
    ];
    let mut previous_tick = 0u64;
    for event in events {
        let delta = event.tick.saturating_sub(previous_tick);
        write_variable_length(delta, &mut track);
        track.extend_from_slice(&[event.kind, event.pitch, event.velocity]);
        previous_tick = event.tick;
    }
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let track_len = u32::try_from(track.len())
        .map_err(|_| "MIDI-Track überschreitet das 4-GiB-Limit.".to_string())?;
    let mut output = b"MThd".to_vec();
    output.extend_from_slice(&6u32.to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes());
    output.extend_from_slice(&1u16.to_be_bytes());
    output.extend_from_slice(&480u16.to_be_bytes());
    output.extend_from_slice(b"MTrk");
    output.extend_from_slice(&track_len.to_be_bytes());
    output.extend_from_slice(&track);
    Ok(output)
}

/// Zweck: Fügt Note-On- und Note-Off-Ereignisse an die MIDI-Ereignisliste an.
/// Eingaben: Tempo, Start/Dauer in Sekunden, MIDI-Ton und Velocity.
/// Ausgabe: Keine Rückgabe; Ereignisse verwenden 480 Ticks je Viertelnote.
fn midi_note_events(
    events: &mut Vec<MidiEvent>,
    bpm: f64,
    start: f64,
    duration: f64,
    pitch: u8,
    velocity: u8,
) {
    let to_tick = |seconds: f64| (seconds.max(0.0) * bpm / 60.0 * 480.0).round() as u64;
    let start_tick = to_tick(start);
    let end_tick = to_tick(start + duration).max(start_tick + 1);
    events.push(MidiEvent {
        tick: start_tick,
        kind: 0x90,
        pitch,
        velocity,
    });
    events.push(MidiEvent {
        tick: end_tick,
        kind: 0x80,
        pitch,
        velocity: 0,
    });
}

/// Zweck: Kodiert ein MIDI-Delta als Standard-Variable-Length-Quantity.
/// Eingaben: Nichtnegative Tickzahl und Zieldatenpuffer.
/// Ausgabe: Keine Rückgabe; Bytes werden in Big-Endian-7-Bit-Gruppen angehängt.
fn write_variable_length(mut value: u64, output: &mut Vec<u8>) {
    let mut buffer = [0u8; 10];
    let mut index = buffer.len() - 1;
    buffer[index] = (value & 0x7F) as u8;
    while {
        value >>= 7;
        value > 0
    } {
        index -= 1;
        buffer[index] = ((value & 0x7F) as u8) | 0x80;
    }
    output.extend_from_slice(&buffer[index..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TimeSignature;
    use crate::song::NoteEvent;

    /// Zweck: Prüft Text- und MIDI-Export samt SMF-Kopf und Tempo-/Notenereignissen.
    /// Eingaben: Keine; erstellt ein Projekt mit einer zeitlich begrenzten MIDI-Note.
    /// Ausgabe: Keine Rückgabe; Assertions sichern lesbaren Text und MThd/MTrk-Struktur.
    #[test]
    fn exports_text_and_standard_midi() {
        let mut project = SongProject::new("Probe", "DBZS");
        project.set_bpm(120.0).unwrap();
        project.notes.push(NoteEvent::new(69, 0.5, 0.5).unwrap());
        project.duration_seconds = 1.0;
        let text = export_text(&project);
        assert!(text.contains("Probe"));
        assert!(text.contains("120.0 BPM"));
        let midi = export_midi_bytes(&project).unwrap();
        assert_eq!(&midi[..4], b"MThd");
        assert!(midi.windows(4).any(|window| window == b"MTrk"));
        assert!(midi.windows(3).any(|window| window == [0x07, 0xA1, 0x20]));
        assert_eq!(project.time_signature, TimeSignature::default());
    }

    /// Zweck: Prüft das portable ZIP-Projektformat und den JSON-/Audio-Pfad-Roundtrip.
    /// Eingaben: Keine; erstellt ein minimales Projektarchiv im temporären Ordner.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Projekttitel und entfernte nicht eingebettete Quelle.
    #[test]
    fn saves_and_loads_portable_project_archive() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("probe.dbztab");
        let mut project = SongProject::new("Archivprobe", "DBZS");
        project.duration_seconds = 4.0;
        save_dbztab(&project, &archive).unwrap();
        let loaded = load_dbztab(&archive, dir.path().join("loaded")).unwrap();
        assert_eq!(loaded.title, "Archivprobe");
        assert_eq!(loaded.duration_seconds, 4.0);
        assert!(loaded.audio_path.is_none());
        assert!(save_dbztab(&project, dir.path().join("wrong.zip")).is_err());
    }
}
