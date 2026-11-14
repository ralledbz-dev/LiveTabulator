/*
===============================================================================
Division By Zeros (DBZS) - Rust-AppController
Modul: controller
Zweck: Orchestriert Songmodell, Audioanalyse, Korrekturen, Tabs, Lyrics,
       Playback, Projekt-Events und Observability ohne UI-Abhängigkeit.
Sprache: Rust
Hinweise:
- UI-Code ruft nur diese Controller-API auf.
- LLM-Inferenz ist kein Teil der Audio-/Akkord-/Beat-Pipeline.
===============================================================================
*/
use crate::analysis::{AnalysisConfig, AnalysisResult, analyze_wave_with_cancel};
use crate::audio::{AudioFrame, WaveFile, read_audio_with_progress};
use crate::domain::{Chord, ChordQuality, KeyMode, KeySignature, NoteName};
use crate::dsp::{extract_chroma, measure_level, waveform_bars};
use crate::event_bus::{AppEvent, EventBus};
use crate::export::export_project;
use crate::jobs::JobManager;
use crate::lyrics::{LyricsBackend, align_transcription_with_language};
use crate::music::analyze_chroma;
use crate::observability::{LogCategory, LogLevel, Observer};
use crate::playback::{PlaybackScheduler, PlaybackTick};
use crate::project_model::{BeatEvent, GuitarTuning, SongProject, StemTrack};
use crate::song::{ChordEvent, LyricsWord, NoteEvent};
use crate::tab_engine::generate_tab_track;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Thread-safe application orchestrator backed by a single editable project state.
#[derive(Debug, Clone)]
pub struct AppController {
    project: Arc<Mutex<SongProject>>,
    observer: Observer,
    events: EventBus,
    jobs: Arc<JobManager>,
    playback: Arc<Mutex<Option<PlaybackScheduler>>>,
    key_alternatives: Arc<Mutex<Vec<String>>>,
    config: AnalysisConfig,
}

impl AppController {
    /// Zweck: Erstellt einen Controller mit leerem Songprojekt und begrenzter Diagnosehistorie.
    /// Eingaben: Projekttitel und Künstlername.
    /// Ausgabe: Thread-sicherer AppController; Jobs und Eventbus sind für Worker vorbereitet.
    pub fn new(title: impl Into<String>, artist: impl Into<String>) -> Self {
        Self {
            project: Arc::new(Mutex::new(SongProject::new(title, artist))),
            observer: Observer::default(),
            events: EventBus::new(),
            jobs: Arc::new(JobManager::new()),
            playback: Arc::new(Mutex::new(None)),
            key_alternatives: Arc::new(Mutex::new(Vec::new())),
            config: AnalysisConfig::default(),
        }
    }

    /// Zweck: Liefert eine konsistente Kopie des aktuellen Songzustands.
    /// Eingaben: `self` – Controller.
    /// Ausgabe: Projektkopie oder Synchronisationsfehler.
    pub fn snapshot(&self) -> Result<SongProject, String> {
        self.project
            .lock()
            .map(|project| project.clone())
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".into())
    }

    /// Zweck: Validiert und übernimmt ein geöffnetes Projekt als neuen bearbeitbaren Zustand.
    /// Eingaben: `project` – deserialisiertes Songprojekt, dessen Schema und Ereignisse geprüft werden.
    /// Ausgabe: `Ok(())` nach Austausch und Playback-Neuaufbau oder ein Validierungs-/Sperrfehler.
    pub fn replace_project(&self, project: SongProject) -> Result<(), String> {
        project.validate()?;
        *self
            .project
            .lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())? = project;
        if let Ok(mut alternatives) = self.key_alternatives.lock() {
            alternatives.clear();
        }
        self.rebuild_playback()?;
        self.observer.log(
            LogLevel::Info,
            LogCategory::Ui,
            "Projektzustand geladen",
            "Projekt wurde lokal geöffnet.",
            BTreeMap::new(),
            None,
        );
        Ok(())
    }

    /// Zweck: Liefert einen Klon des threadsicheren Diagnose-Observers.
    /// Eingaben: `self` – Controller.
    /// Ausgabe: Observer mit Logs, Audio-/Worker-/Modellstatus und optionalen Messwerten.
    pub fn observer(&self) -> Observer {
        self.observer.clone()
    }

    /// Zweck: Liefert einen Klon des plattformneutralen App-Eventbus.
    /// Eingaben: `self` – Controller.
    /// Ausgabe: Eventbus für UI- und Workflow-Subscriber.
    pub fn event_bus(&self) -> EventBus {
        self.events.clone()
    }

    /// Zweck: Liefert den Jobmanager für getrennte Lyrics-, Deep- und Exportjobs.
    /// Eingaben: `self` – Controller.
    /// Ausgabe: Gemeinsamer Jobmanager als Arc.
    pub fn job_manager(&self) -> Arc<JobManager> {
        Arc::clone(&self.jobs)
    }

    /// Zweck: Liefert die letzten aus dem lokalen DSP abgeleiteten Tonartkandidaten für die Anzeige.
    /// Eingaben: `self` – Controller.
    /// Ausgabe: Konfidenzbeschriftete Alternativtonarten; bei leerem/gesperrtem Cache eine leere Liste.
    pub fn key_alternatives(&self) -> Vec<String> {
        self.key_alternatives
            .lock()
            .map(|alternatives| alternatives.clone())
            .unwrap_or_default()
    }

    /// Zweck: Setzt den lokalen Audiodateipfad und die aus WAV gelesene Dauer.
    /// Eingaben: Audio-Dateipfad und vollständige WAV-Metadaten.
    /// Ausgabe: `Ok(())` nach Aktualisierung des Projekts oder Sperrfehler.
    pub fn set_audio_source(
        &self,
        path: impl Into<PathBuf>,
        wave: &WaveFile,
    ) -> Result<(), String> {
        let path = path.into();
        let duration = wave.duration_seconds();
        let mut project = self
            .project
            .lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
        project.audio_path = Some(path.clone());
        project.duration_seconds = duration;
        project.touch();
        self.observer.set_audio_source(format!(
            "{} · {:.2}s · {} Hz · {} Kanal/Kanäle",
            path.display(),
            duration,
            wave.format.sample_rate,
            wave.format.channels
        ));
        Ok(())
    }

    /// Zweck: Validiert und übernimmt das vollständige Ergebnis der lokalen Stem-Trennung.
    /// Eingaben: `tracks` – synchronisierte, bereits als WAV geprüfte Stem-Metadaten.
    /// Ausgabe: `Ok(())` nach Projektaktualisierung oder ein Validierungs-/Sperrfehler.
    pub fn replace_stem_tracks(&self, tracks: Vec<StemTrack>) -> Result<(), String> {
        let mut project = self
            .project
            .lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
        project.replace_stem_tracks(tracks)?;
        self.observer.log(
            LogLevel::Info,
            LogCategory::Audio,
            "Stem-Trennung abgeschlossen",
            "Vier lokale WAV-Spuren wurden dem Songprojekt hinzugefügt.",
            BTreeMap::new(),
            None,
        );
        Ok(())
    }

    /// Zweck: Öffnet und dekodiert eine lokale WAV- oder MP3-Datei ohne UI-Threadarbeit.
    /// Eingaben: Lokaler Audiodateipfad mit unterstützter WAV-/MP3-Endung.
    /// Ausgabe: `WaveFile` mit dekodierten Samples oder format-/dateibezogener Fehler.
    pub fn open_audio(&self, path: impl AsRef<Path>) -> Result<WaveFile, String> {
        self.open_audio_with_progress(path, |_| {})
    }

    /// Zweck: Öffnet eine WAV-/MP3-Datei und meldet während des realen Dekodierens den gemessenen Fortschritt.
    /// Eingaben: Lokaler WAV-/MP3-Pfad und Callback mit monotonem Anteil von null bis eins.
    /// Ausgabe: Dekodierte `WaveFile` und aktualisiertes Projekt oder ein Datei-/Format-/Sperrfehler.
    pub fn open_audio_with_progress<F>(
        &self,
        path: impl AsRef<Path>,
        mut progress: F,
    ) -> Result<WaveFile, String>
    where
        F: FnMut(f32),
    {
        let path = path.as_ref();
        let wave =
            read_audio_with_progress(path, &mut progress).map_err(|error| error.to_string())?;
        self.set_audio_source(path.to_path_buf(), &wave)?;
        self.observer.log(
            LogLevel::Info,
            LogCategory::Audio,
            format!("Audio dekodiert: {} Frames", wave.frame_count()),
            format!(
                "Audioquelle geöffnet: {}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Audiodatei")
            ),
            BTreeMap::new(),
            None,
        );
        Ok(wave)
    }

    /// Zweck: Führt den vollständigen Rust-DSP-Offlinepass aus und übernimmt erkannte Ereignisse ins Projekt.
    /// Eingaben: Lokaler WAV-/MP3-Pfad und gültige Songtaktart; Datei wird einmal dekodiert.
    /// Ausgabe: Analyseergebnis mit Chords, Pitch, Key, Beatgrid, Warnungen und Dauer.
    pub fn analyze_audio_file(&self, path: impl AsRef<Path>) -> Result<AnalysisResult, String> {
        self.analyze_audio_file_with_cancel(path, || false)
    }

    /// Zweck: Führt den Offline-DSP-Pass auf einem Worker aus und prüft vor jedem Analyseframe den Abbruchstatus.
    /// Eingaben: WAV-/MP3-Pfad und kooperativer Abfragecallback, typischerweise aus einem `CancellationToken`.
    /// Ausgabe: Analyseergebnis oder expliziter Abbruch-/Dateifehler; ein abgebrochener Pass übernimmt keine Teilergebnisse.
    pub fn analyze_audio_file_with_cancel<F>(
        &self,
        path: impl AsRef<Path>,
        is_cancelled: F,
    ) -> Result<AnalysisResult, String>
    where
        F: FnMut() -> bool,
    {
        let wave = self.open_audio(path.as_ref())?;
        let time_signature = self.snapshot()?.time_signature;
        let result = analyze_wave_with_cancel(&wave, time_signature, self.config, is_cancelled)?;
        {
            let mut project = self
                .project
                .lock()
                .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
            project.chords = result.chords.clone();
            project.notes = result.notes.clone();
            project.duration_seconds = result.duration_seconds;
            project.key_signature = result.key;
            project.bpm = result
                .beat_grid
                .as_ref()
                .map(|grid| grid.bpm)
                .unwrap_or(project.bpm);
            project.beats = result
                .beat_grid
                .as_ref()
                .map(|grid| grid.beats.clone())
                .unwrap_or_default();
            project.touch();
        }
        let alternatives = result
            .key_alternatives
            .iter()
            .map(|candidate| {
                let mode = match candidate.mode {
                    KeyMode::Major => "Dur",
                    KeyMode::Minor => "Moll",
                };
                format!(
                    "{} {mode} ({:.0}%)",
                    candidate.tonic,
                    candidate.confidence * 100.0
                )
            })
            .collect();
        if let Ok(mut stored) = self.key_alternatives.lock() {
            *stored = alternatives;
        }
        if let Some(grid) = &result.beat_grid {
            self.events.publish(AppEvent::BeatGridUpdated {
                bpm: grid.bpm,
                beats: grid.beats.clone(),
                confidence: grid.confidence,
            });
        }
        for chord in &result.chords {
            self.events.publish(AppEvent::ChordDetected(*chord));
        }
        self.observer.set_worker_status(format!(
            "Offline-DSP abgeschlossen · {} Frames",
            result.analyzed_frames
        ));
        self.observer.log(
            LogLevel::Info,
            LogCategory::Dsp,
            format!(
                "Analysiert: {} Frames, {} Akkorde, {} Noten",
                result.analyzed_frames,
                result.chords.len(),
                result.notes.len()
            ),
            "Audioanalyse abgeschlossen.",
            BTreeMap::new(),
            None,
        );
        for warning in &result.warnings {
            self.observer.log(
                LogLevel::Warning,
                LogCategory::Dsp,
                warning.clone(),
                warning.clone(),
                BTreeMap::new(),
                None,
            );
        }
        self.rebuild_playback()?;
        Ok(result)
    }

    /// Zweck: Analysiert einen Liveframe und aktualisiert Chroma-Akkord-, Tonart- und Pegelzustand.
    /// Eingaben: Mono-Audioframe mit Sample-Rate und absoluter Startsampleposition.
    /// Ausgabe: Neue Akkordschätzung oder `None` bei Stille/geringer Energie; Fehler bei ungültigem Framezustand.
    pub fn process_live_frame(&self, frame: &AudioFrame) -> Result<Option<ChordEvent>, String> {
        let started = std::time::Instant::now();
        let level = measure_level(&frame.samples);
        let chroma = extract_chroma(&frame.samples, frame.sample_rate);
        let analysis = analyze_chroma(&chroma);
        let start = frame.start_sample as f64 / f64::from(frame.sample_rate);
        let duration = frame.samples.len() as f64 / f64::from(frame.sample_rate);
        self.events.publish(AppEvent::AudioLevel {
            rms: level.rms,
            peak: level.peak,
            waveform: waveform_bars(&frame.samples, 48),
        });
        let Some(estimate) = analysis
            .chord
            .filter(|estimate| estimate.confidence >= 0.18)
        else {
            return Ok(None);
        };
        let root = parse_note_name(estimate.root).ok_or_else(|| {
            "Akkordklassifizierer lieferte einen unbekannten Grundton.".to_string()
        })?;
        let chord = Chord {
            root,
            quality: estimate.quality,
        };
        let event =
            ChordEvent::with_confidence(chord, start, start + duration, estimate.confidence)
                .map_err(|error| error.to_string())?;
        let changed = {
            let mut project = self
                .project
                .lock()
                .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
            if let Some(last) = project.chords.last_mut() {
                if last.chord() == chord && start <= last.end_seconds() + 0.15 {
                    *last = ChordEvent::with_confidence(
                        chord,
                        last.start_seconds(),
                        (start + duration).max(last.end_seconds()),
                        last.confidence().max(event.confidence()),
                    )
                    .map_err(|error| error.to_string())?;
                    None
                } else {
                    project.chords.push(event);
                    Some(event)
                }
            } else {
                project.chords.push(event);
                Some(event)
            }
        };
        if let Some(key) = analysis
            .key
            .and_then(|estimate| parse_key_name(estimate.tonic, estimate.mode))
        {
            if let Ok(mut project) = self.project.lock() {
                project.key_signature = Some(key);
            }
        }
        if let Some(changed_event) = changed {
            self.events.publish(AppEvent::ChordDetected(changed_event));
            self.observer.log(
                LogLevel::Info,
                LogCategory::ChordDetection,
                format!(
                    "Liveakkord {} Confidence {:.2}",
                    chord.name(),
                    estimate.confidence
                ),
                format!("Akkord erkannt: {}", chord.name()),
                BTreeMap::new(),
                Some(started.elapsed().as_millis()),
            );
        }
        Ok(Some(event))
    }

    /// Zweck: Erzeugt oder ersetzt eine vollständige Gitarrentabspur aus Melodie-/Akkorddaten.
    /// Eingaben: Gitarrenstimmung und Spurbezeichnung; fehlen NoteEvents, werden Akkordtöne generiert.
    /// Ausgabe: Erzeugte Tabspur oder ein klarer Fehler bei leerer/unspielbarer Musik.
    pub fn generate_tabs(
        &self,
        tuning: GuitarTuning,
        name: impl Into<String>,
    ) -> Result<crate::project_model::TabTrack, String> {
        let name = name.into();
        let mut project = self
            .project
            .lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
        let notes = if project.notes.is_empty() {
            let mut derived = Vec::new();
            for event in &project.chords {
                for midi in event
                    .chord()
                    .midi_notes(3)
                    .map_err(|error| error.to_string())?
                {
                    derived.push(
                        NoteEvent::with_metadata(
                            midi,
                            event.start_seconds(),
                            event.duration_seconds(),
                            82,
                            event.confidence(),
                        )
                        .map_err(|error| error.to_string())?,
                    );
                }
            }
            derived
        } else {
            project.notes.clone()
        };
        if notes.is_empty() {
            return Err(
                "Für die Tab-Generierung sind noch keine Noten oder Akkorde vorhanden.".into(),
            );
        }
        let track = generate_tab_track(&notes, tuning, name)?;
        project.upsert_tab_track(track.clone());
        self.observer.log(
            LogLevel::Info,
            LogCategory::TabGeneration,
            format!("{} Tabnoten erzeugt", track.notes.len()),
            "Gitarrentabulatur aktualisiert.",
            BTreeMap::new(),
            None,
        );
        self.events.publish(AppEvent::JobCompleted {
            id: "tab-generation".into(),
            message: format!("{} Tabnoten", track.notes.len()),
        });
        Ok(track)
    }

    /// Zweck: Ersetzt einen Akkord in der Nähe seiner Zeitposition durch einen validierten Nutzerwert.
    /// Eingaben: Startzeit in Sekunden und Akkordname wie `Em`, `F#`, `Am7` oder `G7`.
    /// Ausgabe: Aktualisierter `ChordEvent` oder verständlicher Fehler bei unbekanntem Akkord/Position.
    pub fn correct_chord(&self, time_seconds: f64, chord_name: &str) -> Result<ChordEvent, String> {
        if !time_seconds.is_finite() || time_seconds < 0.0 {
            return Err("Korrekturzeit muss endlich und nichtnegativ sein.".into());
        }
        let chord = parse_chord_name(chord_name)?;
        let mut project = self
            .project
            .lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
        let index = project
            .chords
            .iter()
            .enumerate()
            .min_by(|left, right| {
                (left.1.start_seconds() - time_seconds)
                    .abs()
                    .total_cmp(&(right.1.start_seconds() - time_seconds).abs())
            })
            .map(|(index, event)| (index, (event.start_seconds() - time_seconds).abs()))
            .filter(|(_, distance)| *distance <= 2.0)
            .map(|(index, _)| index)
            .ok_or_else(|| {
                "Kein Akkordereignis innerhalb von zwei Sekunden für die Korrektur gefunden."
                    .to_string()
            })?;
        let old = project.chords[index];
        let corrected =
            ChordEvent::with_confidence(chord, old.start_seconds(), old.end_seconds(), 1.0)
                .map_err(|error| error.to_string())?;
        project.chords[index] = corrected;
        project.touch();
        drop(project);
        self.events.publish(AppEvent::ChordDetected(corrected));
        self.observer.log(
            LogLevel::Info,
            LogCategory::ChordDetection,
            format!("Nutzerkorrektur: {}", chord.name()),
            format!("Akkord wurde auf {} korrigiert.", chord.name()),
            BTreeMap::new(),
            None,
        );
        Ok(corrected)
    }

    /// Zweck: Verschiebt den Downbeat und regeneriert Zählzeiten ohne Songdauer oder BPM zu verändern.
    /// Eingaben: Neuer erster Beatoffset in Sekunden; er muss endlich, nichtnegativ und vor dem Songende liegen.
    /// Ausgabe: Neue Beatereignisse oder Validierungsfehler; UI-Event und Diagnose werden aktualisiert.
    pub fn correct_downbeat(&self, offset_seconds: f64) -> Result<Vec<BeatEvent>, String> {
        if !offset_seconds.is_finite() || offset_seconds < 0.0 {
            return Err("Downbeat-Offset muss endlich und nichtnegativ sein.".into());
        }
        let (beats, bpm, confidence) = {
            let mut project = self
                .project
                .lock()
                .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
            if project.beats.is_empty() {
                return Err(
                    "Vor der Downbeatkorrektur muss ein Beatgrid analysiert werden.".into(),
                );
            }
            if !project.bpm.is_finite()
                || project.bpm <= 0.0
                || !project.duration_seconds.is_finite()
                || offset_seconds >= project.duration_seconds
            {
                return Err(
                    "Downbeat-Offset muss innerhalb einer gültigen Audiodauer liegen.".into(),
                );
            }
            let bar_length = usize::from(project.time_signature.beats_per_bar());
            if bar_length == 0 {
                return Err("Die Taktart enthält keine gültige Zählzeit pro Takt.".into());
            }
            let beat_duration = 60.0 / project.bpm;
            let confidence = project
                .beats
                .iter()
                .map(|beat| beat.confidence)
                .sum::<f32>()
                / project.beats.len() as f32;
            let count = (((project.duration_seconds - offset_seconds) / beat_duration).floor()
                as usize)
                .saturating_add(1);
            if count == 0 || count > 100_000 {
                return Err(
                    "Berechnetes Beatgrid überschreitet die sichere Ereignisgrenze.".into(),
                );
            }
            let beats = (0..count)
                .map(|index| {
                    BeatEvent::new(
                        offset_seconds + index as f64 * beat_duration,
                        index % bar_length == 0,
                        confidence,
                    )
                    .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            project.beats = beats.clone();
            project.touch();
            (beats, project.bpm, confidence)
        };
        self.events.publish(AppEvent::BeatGridUpdated {
            bpm,
            beats: beats.clone(),
            confidence,
        });
        self.observer.log(
            LogLevel::Info,
            LogCategory::BeatTracking,
            format!("Downbeat manuell auf {offset_seconds:.3}s verschoben"),
            "Downbeatkorrektur übernommen.",
            BTreeMap::new(),
            None,
        );
        Ok(beats)
    }

    /// Zweck: Bearbeitet eine Lyrics-Zeile und verteilt Zeitfenster gleichmäßig auf die korrigierten Wörter.
    /// Eingaben: Nullbasierter Zeilenindex und nichtleerer Nutzertext.
    /// Ausgabe: Aktualisierte Lyrics-Zeile oder Fehler bei ungültigem Text/Index.
    pub fn edit_lyrics_line(&self, line_index: usize, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("Lyrics-Zeile darf nicht leer sein.".into());
        }
        let mut project = self
            .project
            .lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
        let line = project
            .lyrics_lines
            .get_mut(line_index)
            .ok_or_else(|| "Lyrics-Zeilenindex liegt außerhalb der Liste.".to_string())?;
        let tokens = text
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if tokens.is_empty() {
            return Err("Lyrics-Zeile enthält keine Wörter.".into());
        }
        let word_count = tokens.len();
        let start = line.start_seconds;
        let end = line.end_seconds;
        let step = (end - start) / word_count as f64;
        let mut new_words = Vec::with_capacity(word_count);
        for (index, token) in tokens.into_iter().enumerate() {
            let word_start = start + index as f64 * step;
            let word_end = if index + 1 == word_count {
                end
            } else {
                start + (index + 1) as f64 * step
            };
            new_words.push(
                LyricsWord::with_confidence(token, word_start, word_end, 1.0)
                    .map_err(|error| error.to_string())?,
            );
        }
        line.text = text.trim().to_string();
        line.words = new_words;
        project.lyrics_words = project
            .lyrics_lines
            .iter()
            .flat_map(|item| item.words.iter().cloned())
            .collect();
        project.touch();
        self.observer.log(
            LogLevel::Info,
            LogCategory::Lyrics,
            format!("Lyrics-Zeile {line_index} manuell korrigiert"),
            "Lyrics-Korrektur gespeichert.",
            BTreeMap::new(),
            None,
        );
        Ok(())
    }

    /// Zweck: Führt einen optionalen lokalen Lyrics-Backendjob aus und speichert Wort-/Zeilenergebnis.
    /// Eingaben: Implementierung des lokalen LyricsBackend und optionale Sprachkennung.
    /// Ausgabe: LyricsResult mit Beatmapping oder klarer Fehlermeldung ohne Cloud-Fallback.
    pub fn transcribe_lyrics(
        &self,
        backend: &dyn LyricsBackend,
        language: Option<&str>,
    ) -> Result<crate::lyrics::LyricsResult, String> {
        let project = self.snapshot()?;
        let audio_path = project.audio_path.ok_or_else(|| {
            "Für Lyrics muss zuerst eine lokale Audiodatei geöffnet werden.".to_string()
        })?;
        let transcription = backend.transcribe(&audio_path, language)?;
        let result = align_transcription_with_language(
            transcription.words,
            transcription.language,
            project.bpm,
            project.time_signature,
        )?;
        let mut current = self
            .project
            .lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
        current.replace_lyrics(result.words.clone(), result.lines.clone());
        drop(current);
        self.observer.log(
            LogLevel::Info,
            LogCategory::Lyrics,
            format!("{} Wortsegmente aligned", result.words.len()),
            "Lyrics-Transkription abgeschlossen.",
            BTreeMap::new(),
            None,
        );
        Ok(result)
    }

    /// Zweck: Speichert das aktuelle Projekt als lokales `.dbztab`-Archiv.
    /// Eingaben: Projektzielpfad.
    /// Ausgabe: Speicherort oder Validierungs-/ZIP-/Dateifehler.
    pub fn save_project(&self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let project = self.snapshot()?;
        let result = export_project(&project, path, "dbztab");
        self.record_export_result(&result);
        result
    }

    /// Zweck: Exportiert Songdaten im angeforderten Klartext-, Untertitel-, MIDI-, JSON- oder Projektformat.
    /// Eingaben: Zielformat und Dateipfad.
    /// Ausgabe: Geschriebener Pfad oder verständliche Exportfehlermeldung.
    pub fn export(&self, format: &str, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let project = self.snapshot()?;
        let result = export_project(&project, path, format);
        self.record_export_result(&result);
        result
    }

    /// Zweck: Startet eine Playback-Sitzung aus dem aktuellen Songzustand.
    /// Eingaben: `self` – Projekt mit positiver Dauer und BPM.
    /// Ausgabe: Keine Rückgabe; Scheduler steht auf Play oder liefert eine Validierungsfehlermeldung.
    pub fn start_playback(&self) -> Result<(), String> {
        self.ensure_playback_scheduler()?;
        let mut playback = self
            .playback
            .lock()
            .map_err(|_| "Playbackstatus konnte nicht gesperrt werden.".to_string())?;
        playback
            .as_mut()
            .ok_or_else(|| "Playback-Scheduler ist nicht verfügbar.".to_string())?
            .play();
        self.observer.set_worker_status("Playback läuft");
        Ok(())
    }

    /// Zweck: Pausiert den Playback-Scheduler ohne Cursorverlust.
    /// Eingaben: `self` – Controller.
    /// Ausgabe: Keine Rückgabe; Playbackstatus wird als pausiert vermerkt.
    pub fn pause_playback(&self) {
        if let Ok(mut playback) = self.playback.lock() {
            if let Some(player) = playback.as_mut() {
                player.pause();
            }
        }
        self.observer.set_worker_status("Playback pausiert");
    }

    /// Zweck: Stoppt Playback und setzt den Cursor zurück.
    /// Eingaben: `self` – Controller.
    /// Ausgabe: Keine Rückgabe; Scheduler bleibt für den nächsten Start vorhanden.
    pub fn stop_playback(&self) {
        if let Ok(mut playback) = self.playback.lock() {
            if let Some(player) = playback.as_mut() {
                player.stop();
            }
        }
        self.observer.set_worker_status("Playback gestoppt");
    }

    /// Zweck: Verarbeitet einen Playbacktick und veröffentlicht die UI-synchronisierte Songposition.
    /// Eingaben: Vergangene Wandzeit in Sekunden.
    /// Ausgabe: PlaybackTick mit aktiven Noten/Lyrics oder Fehler, wenn Scheduler/Delta ungültig ist.
    pub fn playback_tick(&self, elapsed_seconds: f64) -> Result<PlaybackTick, String> {
        let tick = self
            .playback
            .lock()
            .map_err(|_| "Playbackstatus konnte nicht gesperrt werden.".to_string())?
            .as_mut()
            .ok_or_else(|| "Playback-Scheduler wurde noch nicht aufgebaut.".to_string())?
            .advance(elapsed_seconds)?;
        self.events.publish(AppEvent::PlaybackPosition {
            seconds: tick.position_seconds,
            bar: tick.position.bar,
            beat: tick.position.beat,
            lyric: tick.highlighted_lyric.clone(),
        });
        Ok(tick)
    }

    /// Zweck: Setzt das relative Playbacktempo des aktiven Schedulers.
    /// Eingaben: Prozentwert zwischen 50 und 150.
    /// Ausgabe: `Ok(())` oder verständliche Bereichsmeldung.
    pub fn set_playback_tempo(&self, percent: u16) -> Result<(), String> {
        self.ensure_playback_scheduler()?;
        self.playback
            .lock()
            .map_err(|_| "Playbackstatus konnte nicht gesperrt werden.".to_string())?
            .as_mut()
            .ok_or_else(|| "Playback-Scheduler wurde noch nicht aufgebaut.".to_string())?
            .set_tempo_percent(percent)
    }

    /// Zweck: Aktualisiert Loopbereich oder deaktiviert die Wiederholung.
    /// Eingaben: Optionales Start-/Endzeitpaar in Sekunden.
    /// Ausgabe: `Ok(())` oder Fehler bei ungültigem Bereich.
    pub fn set_playback_loop(&self, range: Option<(f64, f64)>) -> Result<(), String> {
        self.ensure_playback_scheduler()?;
        self.playback
            .lock()
            .map_err(|_| "Playbackstatus konnte nicht gesperrt werden.".to_string())?
            .as_mut()
            .ok_or_else(|| "Playback-Scheduler wurde noch nicht aufgebaut.".to_string())?
            .set_loop(range)
    }

    /// Zweck: Erstellt nur dann einen neuen Scheduler, wenn noch keiner für das Projekt existiert.
    /// Eingaben: `self` – Controller mit validiertem Songzustand.
    /// Ausgabe: `Ok(())` bei vorhandenem oder neu aufgebautem Scheduler.
    fn ensure_playback_scheduler(&self) -> Result<(), String> {
        if self
            .playback
            .lock()
            .map_err(|_| "Playbackstatus konnte nicht gesperrt werden.".to_string())?
            .is_some()
        {
            return Ok(());
        }
        self.rebuild_playback()
    }

    /// Zweck: Baut Playbackereignisse nach Änderungen an Projekt, Noten oder Lyrics neu.
    /// Eingaben: `self` – Controller mit aktuellem Projekt-Snapshot.
    /// Ausgabe: Keine Rückgabe; bei leerem Projekt wird der Scheduler entfernt.
    fn rebuild_playback(&self) -> Result<(), String> {
        let project = self.snapshot()?;
        if project.duration_seconds <= 0.0 {
            if let Ok(mut playback) = self.playback.lock() {
                *playback = None;
            }
            return Ok(());
        }
        let scheduler = PlaybackScheduler::new(
            project.duration_seconds,
            project.bpm,
            project.time_signature,
            project.notes,
            project.lyrics_words,
        )?;
        *self
            .playback
            .lock()
            .map_err(|_| "Playbackstatus konnte nicht gesperrt werden.".to_string())? =
            Some(scheduler);
        Ok(())
    }

    /// Zweck: Protokolliert erfolgreichen oder fehlgeschlagenen Export mit Nutzer- und Techniktext.
    /// Eingaben: Exportergebnis mit Zieldatei oder Fehler.
    /// Ausgabe: Keine Rückgabe; aktualisiert Observer und Exportkategorie.
    fn record_export_result(&self, result: &Result<PathBuf, String>) {
        match result {
            Ok(path) => self.observer.log(
                LogLevel::Info,
                LogCategory::Export,
                format!("Export geschrieben: {}", path.display()),
                format!(
                    "Export gespeichert: {}",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Datei")
                ),
                BTreeMap::new(),
                None,
            ),
            Err(error) => self.observer.log(
                LogLevel::Error,
                LogCategory::Export,
                error.clone(),
                "Export fehlgeschlagen.",
                BTreeMap::new(),
                None,
            ),
        }
    }
}

/// Zweck: Wandelt einen Klassifizierer-Notennamen in die kanonische chromatische Tonklasse um.
/// Eingaben: Notenbezeichnung im ASCII- oder Unicode-Kreuzformat.
/// Ausgabe: `NoteName` oder `None` für unbekannte Namen.
fn parse_note_name(name: &str) -> Option<NoteName> {
    let pitch = match name {
        "C" => 0,
        "C#" | "C♯" => 1,
        "D" => 2,
        "D#" | "D♯" => 3,
        "E" => 4,
        "F" => 5,
        "F#" | "F♯" => 6,
        "G" => 7,
        "G#" | "G♯" => 8,
        "A" => 9,
        "A#" | "A♯" => 10,
        "B" => 11,
        _ => return None,
    };
    Some(NoteName::from_pitch_class(pitch))
}

/// Zweck: Wandelt eine klassifizierte Tonart in den gemeinsamen Core-Tonarttyp um.
/// Eingaben: Grundtonname und Modus.
/// Ausgabe: Typsichere Tonart oder `None`, wenn der Grundton nicht bekannt ist.
fn parse_key_name(name: &str, mode: KeyMode) -> Option<KeySignature> {
    Some(KeySignature {
        root: parse_note_name(name)?,
        mode,
    })
}

/// Zweck: Parst eine manuelle Akkordbezeichnung aus deutschen und internationalen Notennamen.
/// Eingaben: Name wie C, Cm, C#m, Fis, Hm, B, Bb, G7 oder Dmaj7.
/// Ausgabe: Typsicherer Akkord oder ein verständlicher Parsingfehler.
fn parse_chord_name(text: &str) -> Result<Chord, String> {
    let value = text.trim().replace('♯', "#").replace('♭', "b");
    if value.is_empty() {
        return Err("Akkordname darf nicht leer sein.".into());
    }
    let root_len = if value.starts_with("Cis")
        || value.starts_with("Dis")
        || value.starts_with("Fis")
        || value.starts_with("Gis")
        || value.starts_with("Ais")
    {
        3
    } else if value
        .chars()
        .nth(1)
        .is_some_and(|character| matches!(character, '#' | 'b'))
    {
        2
    } else {
        1
    };
    let (root_text, suffix) = value.split_at(root_len.min(value.len()));
    let root = match root_text {
        "C" => NoteName::C,
        "C#" | "Db" | "Cis" => NoteName::CSharp,
        "D" => NoteName::D,
        "D#" | "Eb" | "Dis" => NoteName::DSharp,
        "E" => NoteName::E,
        "F" => NoteName::F,
        "F#" | "Gb" | "Fis" => NoteName::FSharp,
        "G" => NoteName::G,
        "G#" | "Ab" | "Gis" => NoteName::GSharp,
        "A" => NoteName::A,
        "A#" | "Bb" | "Ais" => NoteName::ASharp,
        "B" | "H" => NoteName::B,
        _ => return Err(format!("Unbekannter Akkordgrundton: {root_text}")),
    };
    let suffix = suffix.to_ascii_lowercase();
    let quality = match suffix.as_str() {
        "" | "maj" | "major" => ChordQuality::Major,
        "m" | "min" | "minor" => ChordQuality::Minor,
        "7" | "dom7" => ChordQuality::Dominant7,
        "maj7" | "major7" => ChordQuality::Major7,
        "m7" | "min7" => ChordQuality::Minor7,
        "5" => ChordQuality::PowerChord,
        "dim" => ChordQuality::Diminished,
        "aug" => ChordQuality::Augmented,
        "sus2" => ChordQuality::Suspended2,
        "sus4" => ChordQuality::Suspended4,
        _ => return Err(format!("Akkordqualität {suffix} wird nicht unterstützt.")),
    };
    Ok(Chord { root, quality })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft WAV-Import, vollständige Offlineanalyse und Songzustandsübernahme.
    /// Eingaben: Keine; erzeugt eine temporäre PCM-Datei mit A3/C4/E4-Akkordtönen.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Dauer, Akkorde, Tonart und Diagnoseeintrag.
    #[test]
    fn controller_imports_analyzes_and_updates_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("am.wav");
        let rate = 44_100u32;
        let samples = (0..rate as usize * 2)
            .map(|index| {
                let time = index as f32 / rate as f32;
                [220.0, 261.63, 329.63]
                    .iter()
                    .map(|frequency| (std::f32::consts::TAU * frequency * time).sin())
                    .sum::<f32>()
                    * 0.14
            })
            .collect::<Vec<_>>();
        crate::audio::write_wav(&path, &samples, rate, 1).unwrap();
        let controller = AppController::new("Test", "DBZS");
        let result = controller.analyze_audio_file(&path).unwrap();
        let project = controller.snapshot().unwrap();
        assert!(result.duration_seconds > 1.9);
        assert!(!project.chords.is_empty());
        assert!(project.revision > 0);
        assert!(!controller.key_alternatives().is_empty());
        assert!(
            controller
                .observer()
                .logs()
                .iter()
                .any(|entry| entry.category == LogCategory::Dsp)
        );
    }

    /// Zweck: Prüft Nutzerkorrekturen, Tabgenerierung aus Akkorden und klare Parsingfehler.
    /// Eingaben: Keine; erstellt manuell ein Em-Ereignis und wandelt es in Tabnoten um.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Akkordname, Tabnoten und Fehlertext.
    #[test]
    fn controller_applies_corrections_and_generates_tabs() {
        let controller = AppController::new("Test", "DBZS");
        let chord = ChordEvent::new(
            Chord {
                root: NoteName::E,
                quality: ChordQuality::Minor,
            },
            0.0,
            2.0,
        )
        .unwrap();
        {
            let mut project = controller.project.lock().unwrap();
            project.chords.push(chord);
            project.duration_seconds = 2.0;
        }
        let corrected = controller.correct_chord(0.0, "Am").unwrap();
        assert_eq!(corrected.chord().name(), "Am");
        let track = controller
            .generate_tabs(GuitarTuning::standard(), "Rhythm")
            .unwrap();
        assert!(!track.notes.is_empty());
        assert!(parse_chord_name("Hdim").is_ok());
        assert!(parse_chord_name("Q#").is_err());
    }

    /// Zweck: Prüft, dass eine Downbeatkorrektur neue Zählzeiten erzeugt und die Audiodauer unverändert lässt.
    /// Eingaben: Vier-Sekunden-Projekt mit 120 BPM und einem vorhandenen 4/4-Beatgrid.
    /// Ausgabe: Keine; Assertions sichern Startoffset, Beatabstand, Downbeatmarkierung und ungültige Eingabe.
    #[test]
    fn controller_corrects_downbeat_without_changing_song_duration() {
        let controller = AppController::new("Test", "DBZS");
        {
            let mut project = controller.project.lock().unwrap();
            project.duration_seconds = 4.0;
            project.bpm = 120.0;
            project.beats = (0..8)
                .map(|index| BeatEvent::new(index as f64 * 0.5, index % 4 == 0, 0.8).unwrap())
                .collect();
        }
        let corrected = controller.correct_downbeat(0.25).unwrap();
        let project = controller.snapshot().unwrap();
        assert!((corrected[0].time_seconds - 0.25).abs() < 1e-9);
        assert!((corrected[1].time_seconds - 0.75).abs() < 1e-9);
        assert!(corrected[0].is_downbeat);
        assert_eq!(project.duration_seconds, 4.0);
        assert!(controller.correct_downbeat(f64::NAN).is_err());
    }

    /// Zweck: Prüft Textkorrektur einer Lyrics-Zeile sowie lokale Workerfehler ohne Audioquelle.
    /// Eingaben: Keine; setzt ein Wort, korrigiert den Text und fordert anschließend Transkription ohne Pfad an.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Wortzeitstempel und erklärenden Fehler.
    #[test]
    fn controller_corrects_lyrics_and_requires_local_audio() {
        let controller = AppController::new("Test", "DBZS");
        {
            let mut project = controller.project.lock().unwrap();
            let word = LyricsWord::new("Feuer!", 0.0, 1.0).unwrap();
            project.replace_lyrics(
                vec![word.clone()],
                vec![crate::project_model::LyricsLine {
                    text: "Feuer!".into(),
                    start_seconds: 0.0,
                    end_seconds: 1.0,
                    start_bar: 1,
                    end_bar: 1,
                    words: vec![word],
                }],
            );
        }
        controller.edit_lyrics_line(0, "Licht brennt").unwrap();
        assert_eq!(controller.snapshot().unwrap().lyrics_words.len(), 2);
        struct NoopBackend;
        impl LyricsBackend for NoopBackend {
            /// Zweck: Liefert im Test keinen externen ASR-Aufruf.
            /// Eingaben: Audiopfad und Sprache.
            /// Ausgabe: Testfehler, falls ein lokaler Pfad unerwartet vorhanden wäre.
            fn transcribe(
                &self,
                _: &Path,
                _: Option<&str>,
            ) -> Result<crate::lyrics::TranscriptionOutput, String> {
                Err("nicht erwartet".into())
            }
        }
        assert!(
            controller
                .transcribe_lyrics(&NoopBackend, None)
                .unwrap_err()
                .contains("Audiodatei")
        );
    }
}
