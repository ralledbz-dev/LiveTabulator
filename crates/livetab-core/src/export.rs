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
use crate::timeline::{MAX_TIMELINE_TICKS, TimelineMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MAX_PROJECT_JSON_BYTES: u64 = 32 * 1024 * 1024;
const MAX_EMBEDDED_AUDIO_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MIDI_EVENTS: usize = 1_000_000;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct TemporaryOutputFile {
    path: PathBuf,
    committed: bool,
}

impl Drop for TemporaryOutputFile {
/// Zweck: Entfernt eine nicht abgeschlossene temporäre Ausgabedatei.
    /// Eingaben: `self` – temporärer Dateipfad und Commit-Status.
    /// Ausgabe: Keine; Aufräumfehler verändern niemals die bestehende Zieldatei.
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

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
        "txt" => write_bytes_atomically(path, export_text(project).as_bytes(), "TXT")?,
        "lrc" => write_bytes_atomically(path, export_lrc(&project.lyrics_lines).as_bytes(), "LRC")?,
        "srt" => write_bytes_atomically(path, export_srt(&project.lyrics_lines).as_bytes(), "SRT")?,
        "mid" | "midi" => {
            let bytes = export_midi_bytes(project)?;
            write_bytes_atomically(path, &bytes, "MIDI")?;
        }
        "json" => {
            let bytes = serde_json::to_vec_pretty(project)
                .map_err(|error| format!("JSON-Export fehlgeschlagen: {error}"))?;
            write_bytes_atomically(path, &bytes, "JSON")?;
        }
        "dbztab" => save_dbztab(project, path)?,
        _ => return Err(format!("Exportformat {format} wird nicht unterstützt.")),
    }
    Ok(path.to_path_buf())
}

/// Zweck: Speichert ein portables `.dbztab`-Archiv aus Projekt-JSON, Audioquelle und Stem-WAVs.
/// Eingaben: Valides Songprojekt und Zielpfad; Medien werden nur aus bestehenden lokalen Dateien eingebettet.
/// Ausgabe: `Ok(())` nach Abschluss oder ein JSON-/ZIP-/Dateifehler.
pub fn save_dbztab(project: &SongProject, path: impl AsRef<Path>) -> Result<(), String> {
    project.validate()?;
    let path = path.as_ref();
    if path.exists() && !path.is_file() {
        return Err("Projektarchivziel ist keine reguläre Datei.".into());
    }
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("dbztab"))
    {
        return Err("Das Projektarchiv benötigt die Endung .dbztab.".into());
    }
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Projektordner konnte nicht angelegt werden: {error}"))?;
    }
    let temporary_path = temporary_sibling_path(path);
    let mut staging = TemporaryOutputFile { path: temporary_path, committed: false };
    let output = OpenOptions::new().write(true).create_new(true).open(&staging.path)
        .map_err(|error| format!("Projektarchiv kann nicht erstellt werden: {error}"))?;
    let mut archive = ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut portable = project.clone();
    let embedded_audio = project
        .audio_path
        .as_ref()
        .filter(|audio| audio.is_file())
        .cloned();
    let audio_entry_name = embedded_audio
        .as_ref()
        .map(|audio| -> Result<&'static str, String> {
            let extension = audio
                .extension()
                .and_then(|value| value.to_str())
                .ok_or_else(|| "Eingebettetes Audio benötigt `.wav` oder `.mp3`.".to_string())?;
            if extension.eq_ignore_ascii_case("wav") {
                Ok("audio/source.wav")
            } else if extension.eq_ignore_ascii_case("mp3") {
                Ok("audio/source.mp3")
            } else {
                Err("Eingebettetes Audio benötigt `.wav` oder `.mp3`.".into())
            }
        })
        .transpose()?;
    let mut embedded_stems = Vec::with_capacity(project.stem_tracks.len());
    for track in &project.stem_tracks {
        if !track.file_path.is_file() {
            return Err(format!(
                "Stem-WAV für {} fehlt und kann nicht portabel gespeichert werden.",
                track.name
            ));
        }
        let metadata = fs::metadata(&track.file_path)
            .map_err(|error| format!("Stem-WAV konnte nicht geprüft werden: {error}"))?;
        if metadata.len() > MAX_EMBEDDED_AUDIO_BYTES {
            return Err(format!("Stem-WAV {} überschreitet zwei GiB.", track.name));
        }
        embedded_stems.push((track.id.clone(), track.file_path.clone()));
    }
    portable.audio_path = audio_entry_name.map(PathBuf::from);
    for track in &mut portable.stem_tracks {
        track.file_path = PathBuf::from(format!("audio/stems/{}.wav", track.id));
    }
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
        let entry_name = audio_entry_name
            .as_deref()
            .ok_or_else(|| "Eingebetteter Audioeintrag fehlt im Manifest.".to_string())?;
        archive
            .start_file(entry_name, options)
            .map_err(|error| format!("Audioeintrag konnte nicht begonnen werden: {error}"))?;
        let mut audio = File::open(audio_path)
            .map_err(|error| format!("Audiodatei konnte nicht geöffnet werden: {error}"))?;
        std::io::copy(&mut audio, &mut archive)
            .map_err(|error| format!("Audiodatei konnte nicht eingebettet werden: {error}"))?;
    }
    for (id, stem_path) in embedded_stems {
        let entry_name = format!("audio/stems/{id}.wav");
        archive
            .start_file(&entry_name, options)
            .map_err(|error| format!("Stem-Eintrag konnte nicht begonnen werden: {error}"))?;
        let mut stem = File::open(stem_path)
            .map_err(|error| format!("Stem-WAV konnte nicht geöffnet werden: {error}"))?;
        std::io::copy(&mut stem, &mut archive)
            .map_err(|error| format!("Stem-WAV konnte nicht eingebettet werden: {error}"))?;
    }
    let completed_file = archive
        .finish()
        .map_err(|error| format!("Projektarchiv konnte nicht abgeschlossen werden: {error}"))?;
    completed_file
        .sync_all()
        .map_err(|error| format!("Projektarchiv konnte nicht dauerhaft geschrieben werden: {error}"))?;
    replace_file_preserving_existing(&staging.path, path)?;
    staging.committed = true;
    Ok(())
}

/// Zweck: Erzeugt einen kollisionsarmen temporären Dateipfad im selben Ordner wie das Ziel.
/// Eingaben: Zieldateipfad und globaler Prozesszähler.
/// Ausgabe: Geschwisterpfad für einen später sicher ersetzbaren Export.
fn temporary_sibling_path(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|value| value.to_str()).unwrap_or("livetab-project");
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.livetab-{}-{sequence}.tmp", std::process::id()))
}

/// Zweck: Schreibt Nutzbytes über den gemeinsamen atomaren Dateipfad.
/// Eingaben: Zielpfad, vollständige Nutzbytes und nutzerlesbarer Exportformatname.
/// Ausgabe: `Ok(())` nach sicherem Austausch oder ein Fehler, wobei ein vorhandenes Ziel geschützt bleibt.
fn write_bytes_atomically(destination: &Path, bytes: &[u8], format: &str) -> Result<(), String> {
    write_file_atomically(destination, format, |temporary| {
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(temporary)
            .map_err(|error| format!("Temporäre {format}-Datei konnte nicht geöffnet werden: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("{format}-Export konnte nicht vollständig geschrieben werden: {error}"))
    })
}

/// Zweck: Veröffentlicht eine lokale Ausgabe erst nach vollständigem Schreiben und Synchronisieren.
/// Eingaben: Zielpfad, Formatname und Writer, der ausschließlich die temporäre Schwesterdatei befüllt.
/// Ausgabe: `Ok(())` nach sicherem Austausch oder ein verständlicher Datei-/Writerfehler.
pub fn write_file_atomically<F>(destination: &Path, format: &str, writer: F) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    if destination.exists() && !destination.is_file() {
        return Err(format!("{format}-Ziel ist keine reguläre Datei."));
    }
    if let Some(parent) = destination.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| format!("{format}-Zielordner konnte nicht angelegt werden: {error}"))?;
    }
    let mut staging = create_temporary_output_file(destination, format)?;
    writer(&staging.path)?;
    let completed = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&staging.path)
        .map_err(|error| format!("Temporäre {format}-Datei fehlt nach dem Schreiben: {error}"))?;
    completed
        .sync_all()
        .map_err(|error| format!("{format}-Datei konnte nicht dauerhaft geschrieben werden: {error}"))?;
    drop(completed);
    replace_file_preserving_existing(&staging.path, destination)?;
    staging.committed = true;
    Ok(())
}

/// Zweck: Reserviert kollisionsgeschützt eine temporäre Schwesterdatei mit begrenzten Wiederholungen.
/// Eingaben: Zielpfad und nutzerlesbarer Formatname.
/// Ausgabe: Guard für eine exklusive temporäre Datei oder ein Datei-/Kollisionsfehler.
fn create_temporary_output_file(destination: &Path, format: &str) -> Result<TemporaryOutputFile, String> {
    for _ in 0..32 {
        let path = temporary_sibling_path(destination);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                drop(file);
                return Ok(TemporaryOutputFile { path, committed: false });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Temporäre {format}-Datei konnte nicht erstellt werden: {error}")),
        }
    }
    Err(format!("Nach 32 Versuchen konnte keine temporäre {format}-Datei reserviert werden."))
}

/// Zweck: Ersetzt ein vorhandenes Ziel erst nach vollständigem Schreiben und stellt es bei Fehler wieder her.
/// Eingaben: Vollständige temporäre Schwesterdatei und Zieldatei im selben Verzeichnis.
/// Ausgabe: `Ok(())` nach Ersetzung oder Fehler mit Pfad zur eventuell nötigen Sicherung.
fn replace_file_preserving_existing(temporary: &Path, destination: &Path) -> Result<(), String> {
    if !destination.exists() {
        return fs::rename(temporary, destination)
            .map_err(|error| format!("Zieldatei konnte nicht veröffentlicht werden: {error}"));
    }
    let backup = temporary_sibling_path(destination).with_extension("backup");
    fs::rename(destination, &backup)
        .map_err(|error| format!("Vorhandene Zieldatei konnte nicht sicher vorgemerkt werden: {error}"))?;
    match fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(error) => match fs::rename(&backup, destination) {
            Ok(()) => Err(format!("Neue Zieldatei konnte nicht veröffentlicht werden; das bestehende Ziel wurde wiederhergestellt: {error}")),
            Err(restore_error) => Err(format!("Neue Zieldatei konnte nicht veröffentlicht und das bisherige Ziel nicht zurückverschoben werden; Sicherung liegt unter {}: {error}; {restore_error}", backup.display())),
        },
    }
}

/// Zweck: Lädt ein `.dbztab`-Archiv, validiert das Manifest und extrahiert nur kanonische Audio-/Stem-Einträge.
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
    project.migrate_schema()?;
    project.validate()?;
    for track in &project.stem_tracks {
        let expected = format!("audio/stems/{}.wav", track.id);
        let occurrences = archive
            .file_names()
            .filter(|name| **name == expected)
            .count();
        if occurrences != 1 {
            return Err(format!(
                "Projektarchiv benötigt genau einen Stem-Eintrag {expected}."
            ));
        }
    }
    let audio_entry_name = ["audio/source.mp3", "audio/source.wav"]
        .into_iter()
        .find(|entry_name| archive.file_names().any(|name| name == *entry_name));
    let audio_size = if let Some(entry_name) = audio_entry_name {
        Some(
            archive
                .by_name(entry_name)
                .map(|entry| entry.size())
                .map_err(|error| format!("Audioeintrag konnte nicht geprüft werden: {error}"))?,
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
        let entry_name = audio_entry_name
            .ok_or_else(|| "Eingebetteter Audioeintrag fehlt im Archiv.".to_string())?;
        let file_name = Path::new(entry_name)
            .file_name()
            .ok_or_else(|| "Eingebetteter Audioeintrag besitzt keinen Dateinamen.".to_string())?;
        let audio_path = destination.join(file_name);
        let input = archive
            .by_name(entry_name)
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
    let stem_destination = destination.as_ref().join("audio").join("stems");
    for track in &mut project.stem_tracks {
        let entry_name = format!("audio/stems/{}.wav", track.id);
        let entry_size = archive
            .by_name(&entry_name)
            .map_err(|error| format!("Stem-Eintrag konnte nicht geprüft werden: {error}"))?
            .size();
        if entry_size > MAX_EMBEDDED_AUDIO_BYTES {
            return Err(format!(
                "Stem-Eintrag {} überschreitet zwei GiB.",
                track.name
            ));
        }
        fs::create_dir_all(&stem_destination)
            .map_err(|error| format!("Stem-Zielordner konnte nicht angelegt werden: {error}"))?;
        let stem_path = stem_destination.join(format!("{}.wav", track.id));
        let input = archive
            .by_name(&entry_name)
            .map_err(|error| format!("Stem-Eintrag konnte nicht geöffnet werden: {error}"))?;
        let mut output = File::create(&stem_path)
            .map_err(|error| format!("Stem-Ziel kann nicht geschrieben werden: {error}"))?;
        let copied = std::io::copy(&mut input.take(MAX_EMBEDDED_AUDIO_BYTES + 1), &mut output)
            .map_err(|error| format!("Stem-Eintrag konnte nicht extrahiert werden: {error}"))?;
        if copied > MAX_EMBEDDED_AUDIO_BYTES {
            return Err(format!(
                "Stem-Eintrag {} überschreitet zwei GiB.",
                track.name
            ));
        }
        track.file_path = stem_path;
    }
    Ok(project)
}

#[derive(Clone, Copy)]
struct MidiEvent {
    tick: u64,
    kind: u8,
    data: [u8; 7],
    data_len: u8,
}

/// Zweck: Erzeugt einen SMF-Format-0-Standard-MIDI-Stream mit Noten sowie Tempo-/Taktartwechseln.
/// Eingaben: Validiertes Songprojekt mit gemeinsamer Timeline, Noten- und/oder Akkordereignissen.
/// Ausgabe: MIDI-Dateibytes oder Fehler bei ungültiger Timeline, Ereignisgrenze oder Dateilänge.
pub fn export_midi_bytes(project: &SongProject) -> Result<Vec<u8>, String> {
    project.validate()?;
    let timeline = project.timeline_map()?;
    const PPQ: u16 = 480;
    let mut events = Vec::new();
    if !project.tempo_changes.iter().any(|change| change.time_seconds == 0.0) {
        push_tempo_event(&mut events, 0, project.bpm)?;
    }
    for change in &project.tempo_changes {
        let tick = timeline.seconds_to_ticks(change.time_seconds, project.bpm, PPQ)?;
        push_tempo_event(&mut events, tick, change.bpm)?;
    }
    if !project.meter_changes.iter().any(|change| change.time_seconds == 0.0) {
        push_meter_event(&mut events, 0, project.time_signature)?;
    }
    for change in &project.meter_changes {
        let tick = timeline.seconds_to_ticks(change.time_seconds, project.bpm, PPQ)?;
        push_meter_event(&mut events, tick, change.time_signature)?;
    }
    if !project.notes.is_empty() {
        for note in &project.notes {
            midi_note_events(
                &mut events,
                &timeline,
                project.bpm,
                PPQ,
                note.start_seconds(),
                note.duration_seconds(),
                note.midi(),
                note.velocity(),
            )?;
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
                    &timeline,
                    project.bpm,
                    PPQ,
                    chord.start_seconds(),
                    chord.duration_seconds(),
                    midi,
                    80,
                )?;
            }
        }
    }
    events.sort_by_key(|event| (event.tick, event.kind));
    let mut track = Vec::new();
    let mut previous_tick = 0u64;
    for event in events {
        let delta = event.tick.saturating_sub(previous_tick);
        write_variable_length(delta, &mut track);
        track.extend_from_slice(&event.data[..usize::from(event.data_len)]);
        previous_tick = event.tick;
    }
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let track_len = u32::try_from(track.len())
        .map_err(|_| "MIDI-Track überschreitet das 4-GiB-Limit.".to_string())?;
    let mut output = b"MThd".to_vec();
    output.extend_from_slice(&6u32.to_be_bytes());
    output.extend_from_slice(&0u16.to_be_bytes());
    output.extend_from_slice(&1u16.to_be_bytes());
    output.extend_from_slice(&PPQ.to_be_bytes());
    output.extend_from_slice(b"MTrk");
    output.extend_from_slice(&track_len.to_be_bytes());
    output.extend_from_slice(&track);
    Ok(output)
}

/// Zweck: Fügt Note-On- und Note-Off-Ereignisse mithilfe der kanonischen Timeline ein.
/// Eingaben: Eventliste, TimelineMap, Basis-BPM, PPQ, Zeitfenster, MIDI-Ton und Velocity.
/// Ausgabe: `Ok(())` oder kontrollierter Fehler bei ungültiger Zeit/Tickgrenze.
fn midi_note_events(
    events: &mut Vec<MidiEvent>,
    timeline: &TimelineMap,
    bpm: f64,
    ppq: u16,
    start: f64,
    duration: f64,
    pitch: u8,
    velocity: u8,
) -> Result<(), String> {
    let end = start + duration;
    if !start.is_finite() || !duration.is_finite() || !end.is_finite() || start < 0.0 || duration <= 0.0 {
        return Err("MIDI-Notenzeit muss endlich, nichtnegativ und positiv lang sein.".into());
    }
    let start_tick = timeline.seconds_to_ticks(start, bpm, ppq)?;
    let end_tick = timeline.seconds_to_ticks(end, bpm, ppq)?
        .max(start_tick.saturating_add(1));
    push_midi_message(events, start_tick, 0x90, &[0x90, pitch, velocity])?;
    push_midi_message(events, end_tick, 0x80, &[0x80, pitch, 0])
}

/// Zweck: Fügt ein SMF-Tempo-Metereignis mit Mikrosekunden je Viertelnote ein.
/// Eingaben: Eventliste, absolute Tickposition und positives Viertelnoten-BPM.
/// Ausgabe: `Ok(())` oder kontrollierter Fehler bei Wert-/Eventgrenzen.
fn push_tempo_event(events: &mut Vec<MidiEvent>, tick: u64, bpm: f64) -> Result<(), String> {
    if !bpm.is_finite() || bpm <= 0.0 {
        return Err("MIDI-Tempo muss positiv und endlich sein.".into());
    }
    let micros = (60_000_000.0 / bpm).round().clamp(1.0, 0xFF_FFFF as f64) as u32;
    push_midi_message(events, tick, 0, &[
        0xFF, 0x51, 0x03, (micros >> 16) as u8, (micros >> 8) as u8, micros as u8,
    ])
}

/// Zweck: Fügt ein SMF-Taktart-Metereignis für Zähler und Zweierpotenz-Nenner ein.
/// Eingaben: Eventliste, absolute Tickposition und validierte Taktart.
/// Ausgabe: `Ok(())` oder Begrenzungsfehler.
fn push_meter_event(
    events: &mut Vec<MidiEvent>,
    tick: u64,
    meter: crate::domain::TimeSignature,
) -> Result<(), String> {
    push_midi_message(events, tick, 0, &[
        0xFF, 0x58, 0x04, meter.numerator(), meter.denominator().trailing_zeros() as u8, 24, 8,
    ])
}

/// Zweck: Speichert eine begrenzte SMF-Nachricht in der sortierbaren MIDI-Ereignisliste.
/// Eingaben: Eventliste, Tick, Sortierpriorität und höchstens sieben Nachrichtenbytes.
/// Ausgabe: `Ok(())` oder kontrollierter Fehler bei ungültiger Nachricht, Tick oder Kapazität.
fn push_midi_message(
    events: &mut Vec<MidiEvent>,
    tick: u64,
    kind: u8,
    message: &[u8],
) -> Result<(), String> {
    if tick > MAX_TIMELINE_TICKS || message.is_empty() || message.len() > 7 {
        return Err("MIDI-Nachricht überschreitet Tick- oder Längenlimit.".into());
    }
    if events.len() >= MAX_MIDI_EVENTS {
        return Err(format!("MIDI-Export überschreitet {MAX_MIDI_EVENTS} Ereignisse."));
    }
    events.try_reserve(1)
        .map_err(|_| "MIDI-Eventliste kann nicht sicher reserviert werden.".to_string())?;
    let mut data = [0_u8; 7];
    data[..message.len()].copy_from_slice(message);
    events.push(MidiEvent { tick, kind, data, data_len: message.len() as u8 });
    Ok(())
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
    use crate::timeline::{MeterChange, TempoChange};

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

    /// Zweck: Prüft, dass Tempo-/Taktwechsel als echte SMF-Metaevents auf der integrierten Timeline erscheinen.
    /// Eingaben: Projekt mit 120→60 BPM und einem Wechsel von 4/4 auf 3/4 nach zwei Sekunden.
    /// Ausgabe: Keine Rückgabe; Assertions sichern die beiden Tempo- und Taktart-Metadaten.
    #[test]
    fn midi_export_preserves_tempo_and_meter_changes() {
        let mut project = SongProject::new("Map", "DBZS");
        project.duration_seconds = 4.0;
        project.notes.push(NoteEvent::new(69, 3.0, 1.0).unwrap());
        project.set_tempo_changes(vec![TempoChange { time_seconds: 2.0, bpm: 60.0 }]).unwrap();
        project.set_meter_changes(vec![MeterChange {
            time_seconds: 2.0,
            time_signature: TimeSignature::new(3, 4).unwrap(),
        }]).unwrap();
        let midi = export_midi_bytes(&project).unwrap();
        assert!(midi.windows(6).any(|bytes| bytes == [0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20]));
        assert!(midi.windows(6).any(|bytes| bytes == [0xFF, 0x51, 0x03, 0x0F, 0x42, 0x40]));
        assert!(midi.windows(7).any(|bytes| bytes == [0xFF, 0x58, 0x04, 0x04, 0x02, 24, 8]));
        assert!(midi.windows(7).any(|bytes| bytes == [0xFF, 0x58, 0x04, 0x03, 0x02, 24, 8]));
    }

    /// Zweck: Prüft atomaren MIDI-Export auf ein vorhandenes Ziel und schützt Verzeichnisse vor Ersetzung.
    /// Eingaben: Temporäres Ziel mit Altinhalt sowie ein minimales, gültiges Songprojekt.
    /// Ausgabe: Keine Rückgabe; Assertions sichern vollständige Veröffentlichung und Zielschutz.
    #[test]
    fn exports_replace_files_atomically_and_reject_directories() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("atomic.mid");
        std::fs::write(&destination, b"old data").unwrap();
        let mut project = SongProject::new("Atomic", "DBZS");
        project.duration_seconds = 1.0;
        project.notes.push(NoteEvent::new(60, 0.0, 0.5).unwrap());
        export_project(&project, &destination, "mid").unwrap();
        assert!(std::fs::read(&destination).unwrap().starts_with(b"MThd"));
        let directory_target = dir.path().join("keep-directory");
        std::fs::create_dir(&directory_target).unwrap();
        assert!(export_project(&project, &directory_target, "txt").is_err());
        assert!(directory_target.is_dir());
    }

    /// Zweck: Prüft, dass ein fehlschlagender Dateiwriter nur sein Staging entfernt und das alte Ziel erhält.
    /// Eingaben: Vorhandene Zieldatei und Writer, der partielle Temporärbytes schreibt und danach einen Fehler liefert.
    /// Ausgabe: Keine Rückgabe; Assertion sichert unveränderten Altinhalt.
    #[test]
    fn failed_atomic_writer_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("existing.audio");
        std::fs::write(&destination, b"previous complete data").unwrap();
        let result = write_file_atomically(&destination, "TEST", |temporary| {
            std::fs::write(temporary, b"partial replacement")
                .map_err(|error| error.to_string())?;
            Err("synthetic writer failure".into())
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous complete data");
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

    /// Zweck: Prüft, dass eingebettete MP3-Quelldaten im Projektarchiv ihre echte Dateiendung behalten.
    /// Eingaben: Keine; speichert kurze MP3-Testbytes in einem temporären Projekt und lädt das Archiv erneut.
    /// Ausgabe: Keine Rückgabe; Assertions sichern ZIP-Eintrag und extrahierten `.mp3`-Pfad.
    #[test]
    fn preserves_mp3_extension_in_portable_project_archive() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.mp3");
        std::fs::write(&source, b"synthetic MP3 test bytes").unwrap();
        let archive_path = dir.path().join("with-audio.dbztab");
        let mut project = SongProject::new("MP3 Archiv", "DBZS");
        project.audio_path = Some(source);

        save_dbztab(&project, &archive_path).unwrap();
        let loaded = load_dbztab(&archive_path, dir.path().join("restored")).unwrap();

        assert_eq!(
            loaded.audio_path.as_deref().and_then(Path::extension),
            Some(std::ffi::OsStr::new("mp3"))
        );
        assert!(loaded.audio_path.unwrap().is_file());
    }

    /// Zweck: Prüft das Einbetten und sichere Wiederherstellen aller vier Stem-WAVs.
    /// Eingaben: Keine; erstellt vier synthetische kurze PCM-WAVs und ein `.dbztab`-Projekt.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Spurmetadaten und kanonische lokale Zielpfade.
    #[test]
    fn saves_and_loads_four_stem_wavs_portably() {
        use crate::audio::write_wav;
        use crate::project_model::{StemKind, StemTrack};

        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("stems.dbztab");
        let mut project = SongProject::new("Stems", "DBZS");
        let kinds = [
            StemKind::Vocals,
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
        ];
        for (index, kind) in kinds.into_iter().enumerate() {
            let wav = dir.path().join(format!("stem-{index}.wav"));
            write_wav(&wav, &[0.1, -0.1], 44_100, 1).unwrap();
            project.stem_tracks.push(StemTrack {
                id: format!("stem-{index}"),
                name: kind.display_name().into(),
                kind,
                file_path: wav,
                sample_rate: 44_100,
                channels: 1,
                duration_seconds: 2.0 / 44_100.0,
                model_filename: "htdemucs.yaml".into(),
                model_version: "0.48.0".into(),
            });
        }
        project.validate().unwrap();
        save_dbztab(&project, &archive).unwrap();
        let loaded_root = dir.path().join("loaded");
        let loaded = load_dbztab(&archive, &loaded_root).unwrap();
        assert_eq!(loaded.stem_tracks.len(), 4);
        assert!(
            loaded
                .stem_tracks
                .iter()
                .all(|track| track.file_path.is_file())
        );
        assert_eq!(
            loaded.stem_tracks[0].file_path,
            loaded_root.join("audio/stems/stem-0.wav")
        );
    }
}
