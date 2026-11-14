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
use crate::beat::{BeatGrid, estimate_beat_grid};
use crate::audio::{AudioFrame, WaveFile, read_audio_with_progress};
use crate::domain::{Chord, ChordQuality, KeyMode, KeySignature, NoteName};
use crate::dsp::{PitchEstimate, estimate_pitch, extract_chroma, measure_level, waveform_bars};
use crate::event_bus::{AppEvent, EventBus};
use crate::export::export_project;
use crate::jobs::JobManager;
use crate::lyrics::{LyricsBackend, align_transcription_with_language};
use crate::music::{analyze_chroma, estimate_key_candidates};
use crate::observability::{LogCategory, LogLevel, Observer};
use crate::playback::{PlaybackScheduler, PlaybackTick};
use crate::project_model::{BeatEvent, EventOrigin, GuitarTuning, SongProject, StemTrack, MAX_PROJECT_EVENTS};
use crate::song::{ChordEvent, LyricsWord, NoteEvent};
use crate::tab_engine::generate_tab_track;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Thread-safe application orchestrator backed by a single editable project state.
#[derive(Debug, Clone)]
pub struct AppController {
    project: Arc<Mutex<SongProject>>,
    edit_history: Arc<Mutex<ProjectEditHistory>>,
    observer: Observer,
    events: EventBus,
    jobs: Arc<JobManager>,
    playback: Arc<Mutex<Option<PlaybackScheduler>>>,
    key_alternatives: Arc<Mutex<Vec<String>>>,
    live_dsp: Arc<Mutex<LiveDspState>>,
    config: AnalysisConfig,
}

const MAX_EDIT_HISTORY: usize = 64;
const LIVE_CHROMA_HISTORY_FRAMES: usize = 9;
const LIVE_PITCH_HISTORY_FRAMES: usize = 5;
const LIVE_BEAT_WINDOW_SECONDS: usize = 12;

#[derive(Debug, Clone, Copy)]
struct ActiveLiveNote {
    id: u64,
    midi: u8,
    start_sample: u64,
    last_end_sample: u64,
    last_publish_sample: u64,
}

#[derive(Debug, Default)]
struct LiveDspState {
    sample_rate: Option<u32>,
    last_frame_end_sample: Option<u64>,
    last_frame_start_sample: Option<u64>,
    chroma_history: VecDeque<[f32; 12]>,
    pitch_history: VecDeque<(u64, Option<PitchEstimate>)>,
    active_note: Option<ActiveLiveNote>,
    pitch_ui_initialized: bool,
    last_pitch_ui_midi: Option<u8>,
    last_pitch_ui_sample: u64,
    rolling_audio: VecDeque<f32>,
    rolling_audio_start_sample: u64,
    last_beat_analysis_sample: u64,
    beat_bpm_history: VecDeque<f64>,
    last_key_candidate: Option<String>,
    key_candidate_streak: usize,
    last_key_signature: Option<String>,
    last_key_alternatives: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct StableLivePitch {
    estimate: PitchEstimate,
    start_sample: u64,
}

struct LiveNoteDraft {
    event: NoteEvent,
    is_new: bool,
    active: ActiveLiveNote,
    previous: Option<ActiveLiveNote>,
    publish: bool,
}

struct LiveKeyUpdate {
    alternatives: Vec<String>,
    confidence: f32,
    signature: Option<KeySignature>,
}

struct LiveFrameDecisions {
    chroma: [f32; 12],
    pitch: Option<StableLivePitch>,
    publish_pitch: bool,
    note: Option<LiveNoteDraft>,
    key: Option<LiveKeyUpdate>,
    beat_window: Option<(Vec<f32>, u64)>,
}

#[derive(Debug, Default)]
struct ProjectEditHistory {
    undo: Vec<SongProject>,
    redo: Vec<SongProject>,
}

impl AppController {
    /// Zweck: Erstellt einen Controller mit leerem Songprojekt und begrenzter Diagnosehistorie.
    /// Eingaben: Projekttitel und Künstlername.
    /// Ausgabe: Thread-sicherer AppController; Jobs und Eventbus sind für Worker vorbereitet.
    pub fn new(title: impl Into<String>, artist: impl Into<String>) -> Self {
        Self {
            project: Arc::new(Mutex::new(SongProject::new(title, artist))),
            edit_history: Arc::new(Mutex::new(ProjectEditHistory::default())),
            observer: Observer::default(),
            events: EventBus::new(),
            jobs: Arc::new(JobManager::new()),
            playback: Arc::new(Mutex::new(None)),
            key_alternatives: Arc::new(Mutex::new(Vec::new())),
            live_dsp: Arc::new(Mutex::new(LiveDspState::default())),
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

    /// Zweck: Sichert einen Zustand vor einer expliziten Nutzerkorrektur und leert den Redo-Zweig.
    /// Eingaben: `before` – geprüfter Projektzustand unmittelbar vor der Mutation.
    /// Ausgabe: `Ok(())` oder Synchronisationsfehler; älteste Einträge werden oberhalb des Limits verworfen.
    fn push_edit_snapshot(&self, before: &SongProject) -> Result<(), String> {
        let mut history = self.edit_history.lock()
            .map_err(|_| "Undo-/Redo-Verlauf konnte nicht gesperrt werden.".to_string())?;
        if history.undo.len() == MAX_EDIT_HISTORY {
            history.undo.remove(0);
        }
        history.undo.push(before.clone());
        history.redo.clear();
        Ok(())
    }

    /// Zweck: Stellt den letzten explizit bearbeiteten Projektzustand wieder her.
    /// Eingaben: `self` – Controller mit begrenztem Undo-Verlauf.
    /// Ausgabe: Wiederhergestelltes Projekt oder Meldung, falls nichts rückgängig zu machen ist.
    pub fn undo_edit(&self) -> Result<SongProject, String> {
        let mut project = self.project.lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
        let mut history = self.edit_history.lock()
            .map_err(|_| "Undo-/Redo-Verlauf konnte nicht gesperrt werden.".to_string())?;
        let mut previous = history.undo.last().cloned()
            .ok_or_else(|| "Es gibt keine manuelle Projektänderung zum Rückgängigmachen.".to_string())?;
        previous.validate()?;
        history.undo.pop();
        if history.redo.len() == MAX_EDIT_HISTORY { history.redo.remove(0); }
        history.redo.push(project.clone());
        previous.revision = project.revision;
        previous.touch();
        *project = previous.clone();
        drop(history);
        drop(project);
        self.rebuild_playback()?;
        Ok(previous)
    }

    /// Zweck: Stellt den zuletzt rückgängig gemachten Zustand wieder her.
    /// Eingaben: `self` – Controller mit begrenztem Redo-Verlauf.
    /// Ausgabe: Wiederhergestelltes Projekt oder Meldung, falls kein Redo verfügbar ist.
    pub fn redo_edit(&self) -> Result<SongProject, String> {
        let mut project = self.project.lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
        let mut history = self.edit_history.lock()
            .map_err(|_| "Undo-/Redo-Verlauf konnte nicht gesperrt werden.".to_string())?;
        let mut next = history.redo.last().cloned()
            .ok_or_else(|| "Es gibt keine rückgängig gemachte Änderung zum Wiederherstellen.".to_string())?;
        next.validate()?;
        history.redo.pop();
        if history.undo.len() == MAX_EDIT_HISTORY { history.undo.remove(0); }
        history.undo.push(project.clone());
        next.revision = project.revision;
        next.touch();
        *project = next.clone();
        drop(history);
        drop(project);
        self.rebuild_playback()?;
        Ok(next)
    }

    /// Zweck: Validiert und übernimmt ein geöffnetes Projekt als neuen bearbeitbaren Zustand.
    /// Eingaben: `project` – deserialisiertes Songprojekt, dessen Schema und Ereignisse geprüft werden.
    /// Ausgabe: `Ok(())` nach Austausch und Playback-Neuaufbau oder ein Validierungs-/Sperrfehler.
    pub fn replace_project(&self, mut project: SongProject) -> Result<(), String> {
        project.migrate_schema()?;
        project.validate()?;
        *self
            .project
            .lock()
            .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())? = project;
        if let Ok(mut history) = self.edit_history.lock() {
            history.undo.clear();
            history.redo.clear();
        }
        if let Ok(mut alternatives) = self.key_alternatives.lock() {
            alternatives.clear();
        }
        self.reset_live_analysis();
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

    /// Zweck: Verwirft geglättete Pitch-/Chroma-/Beatfenster an einer neuen Audioquellgrenze.
    /// Eingaben: `self` – Controller mit gemeinsam genutztem Live-DSP-Zustand.
    /// Ausgabe: Keine; nachfolgende Liveframes beginnen ohne Messhistorie der alten Quelle.
    pub fn reset_live_analysis(&self) {
        if let Ok(mut state) = self.live_dsp.lock() {
            *state = LiveDspState::default();
        }
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
        drop(project);
        self.reset_live_analysis();
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

    /// Zweck: Führt den Offline-DSP-Pass abbrechbar aus und übernimmt Audioreferenz/Ergebnis erst atomar nach Erfolg.
    /// Eingaben: WAV-/MP3-Pfad und kooperativer Abfragecallback, typischerweise aus einem `CancellationToken`.
    /// Ausgabe: Analyseergebnis oder Abbruch-/Datei-/Validierungsfehler ohne teilweise Projektübernahme.
    pub fn analyze_audio_file_with_cancel<F>(
        &self,
        path: impl AsRef<Path>,
        is_cancelled: F,
    ) -> Result<AnalysisResult, String>
    where
        F: FnMut() -> bool,
    {
        let path = path.as_ref();
        let wave = read_audio_with_progress(path, |_| {})
            .map_err(|error| error.to_string())?;
        let time_signature = self.snapshot()?.time_signature;
        let result = analyze_wave_with_cancel(&wave, time_signature, self.config, is_cancelled)?;
        {
            let mut project = self
                .project
                .lock()
                .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
            let mut next = project.clone();
            let replaced_ids = next.chords.iter().map(|event| event.id())
                .chain(next.notes.iter().map(NoteEvent::id))
                .chain(next.beats.iter().map(|event| event.id))
                .collect::<HashSet<_>>();
            next.event_provenance.retain(|entry| !replaced_ids.contains(&entry.event_id));
            next.audio_path = Some(path.to_path_buf());
            next.duration_seconds = result.duration_seconds;
            next.key_signature = result.key;
            next.bpm = result
                .beat_grid
                .as_ref()
                .map(|grid| grid.bpm)
                .unwrap_or(next.bpm);
            next.chords = result.chords.clone();
            next.notes = result.notes.clone();
            next.beats = result
                .beat_grid
                .as_ref()
                .map(|grid| grid.beats.clone())
                .unwrap_or_default();
            let provenance = result.chords.iter().map(|event| (
                event.id(), EventOrigin::OfflineAnalysis,
                Some("livetab-core-dsp".into()), Some("Akkordereignis aus Offlineanalyse".into()),
            )).chain(result.notes.iter().map(|event| (
                event.id(), EventOrigin::OfflineAnalysis,
                Some("livetab-core-dsp".into()), Some("Notenereignis aus Offlineanalyse".into()),
            ))).chain(result.beat_grid.iter().flat_map(|grid| grid.beats.iter().map(|event| (
                event.id, EventOrigin::OfflineAnalysis,
                Some("livetab-core-beat".into()), Some("Beatgrid-Ereignis aus Offlineanalyse".into()),
            )))).collect::<Vec<_>>();
            if provenance.is_empty() {
                next.touch();
            } else {
                next.record_provenance_batch(provenance)?;
            }
            next.validate()?;
            *project = next;
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

    /// Zweck: Glättet bounded Live-Messungen und bildet ein Rollingfenster für die getrennte Beat-Analyse.
    /// Eingaben: Mono-Audioframe, bereits gemessener Pegel, Chroma und optionaler Rohpitch.
    /// Ausgabe: Stabiler Pitch-/Notenkandidat, Tonartänderung und höchstens zwölf Sekunden Beat-Audio.
    fn update_live_dsp_state(
        &self,
        frame: &AudioFrame,
        level: crate::dsp::Level,
        chroma: [f32; 12],
        raw_pitch: Option<PitchEstimate>,
    ) -> Result<LiveFrameDecisions, String> {
        if frame.sample_rate == 0 {
            return Err("Live-DSP benötigt eine positive Sample-Rate.".into());
        }
        let sample_rate = frame.sample_rate;
        let sample_rate_u64 = u64::from(sample_rate);
        let end_sample = frame.start_sample.saturating_add(frame.samples.len() as u64);
        let mut state = self.live_dsp.lock()
            .map_err(|_| "Live-DSP-Zustand konnte nicht gesperrt werden.".to_string())?;
        let reset = state.sample_rate.is_some_and(|rate| rate != sample_rate)
            || state.last_frame_start_sample.is_some_and(|previous| frame.start_sample < previous)
            || state.last_frame_end_sample.is_some_and(|previous| frame.start_sample > previous);
        if reset {
            *state = LiveDspState::default();
        }
        state.sample_rate = Some(sample_rate);

        state.chroma_history.push_back(chroma);
        while state.chroma_history.len() > LIVE_CHROMA_HISTORY_FRAMES {
            state.chroma_history.pop_front();
        }
        let mut smoothed_chroma = [0.0_f32; 12];
        for historical in &state.chroma_history {
            for (sum, value) in smoothed_chroma.iter_mut().zip(historical) {
                *sum += *value;
            }
        }
        let history_count = state.chroma_history.len().max(1) as f32;
        for value in &mut smoothed_chroma {
            *value /= history_count;
        }

        let accepted_pitch = raw_pitch.filter(|pitch| {
            level.rms >= 0.003 && pitch.confidence >= self.config.minimum_pitch_confidence
        });
        state.pitch_history.push_back((frame.start_sample, accepted_pitch));
        while state.pitch_history.len() > LIVE_PITCH_HISTORY_FRAMES {
            state.pitch_history.pop_front();
        }
        let last_three = state.pitch_history.iter().rev().take(3).collect::<Vec<_>>();
        let stable_pitch = if last_three.len() == 3 {
            match last_three[0].1 {
                Some(first) if last_three.iter().all(|(_, pitch)| pitch.map_or(false, |item| item.midi == first.midi)) => {
                    let pitches = last_three.iter().filter_map(|(_, pitch)| *pitch).collect::<Vec<_>>();
                    Some(StableLivePitch {
                        estimate: PitchEstimate {
                            midi: first.midi,
                            frequency_hz: pitches.iter().map(|pitch| pitch.frequency_hz).sum::<f32>() / pitches.len() as f32,
                            confidence: pitches.iter().map(|pitch| pitch.confidence).sum::<f32>() / pitches.len() as f32,
                        },
                        start_sample: last_three[2].0,
                    })
                }
                _ => None,
            }
        } else {
            None
        };

        let stable_midi = stable_pitch.map(|pitch| pitch.estimate.midi);
        let all_three_silent = last_three.len() == 3 && last_three.iter().all(|(_, pitch)| pitch.is_none());
        let mut publish_pitch = false;
        if stable_pitch.is_some() {
            publish_pitch = !state.pitch_ui_initialized
                || state.last_pitch_ui_midi != stable_midi
                || end_sample.saturating_sub(state.last_pitch_ui_sample) >= sample_rate_u64 / 10;
        } else if all_three_silent && state.pitch_ui_initialized && state.last_pitch_ui_midi.is_some() {
            publish_pitch = true;
        }
        if publish_pitch {
            state.pitch_ui_initialized = true;
            state.last_pitch_ui_midi = stable_midi;
            state.last_pitch_ui_sample = end_sample;
        }

        let mut note = None;
        if let Some(pitch) = stable_pitch {
            let previous = state.active_note;
            let continues = previous.is_some_and(|active| {
                active.midi == pitch.estimate.midi
                    && pitch.start_sample <= active.last_end_sample.saturating_add(sample_rate_u64 / 6)
            });
            let is_new = !continues;
            let (id, note_start_sample) = if continues {
                let active = previous.expect("continuing note has an active event");
                (Some(active.id), active.start_sample)
            } else {
                (None, pitch.start_sample)
            };
            let note_duration = end_sample.saturating_sub(note_start_sample).max(1) as f64
                / f64::from(sample_rate);
            let event = match id {
                Some(id) => NoteEvent::with_metadata_and_id(
                    id, pitch.estimate.midi, note_start_sample as f64 / f64::from(sample_rate),
                    note_duration, 90, pitch.estimate.confidence,
                ),
                None => NoteEvent::with_metadata(
                    pitch.estimate.midi, note_start_sample as f64 / f64::from(sample_rate),
                    note_duration, 90, pitch.estimate.confidence,
                ),
            }.map_err(|error| error.to_string())?;
            let should_publish = is_new || previous.map_or(true, |active| {
                end_sample.saturating_sub(active.last_publish_sample) >= sample_rate_u64 / 4
            });
            let active = ActiveLiveNote {
                id: event.id(),
                midi: pitch.estimate.midi,
                start_sample: note_start_sample,
                last_end_sample: end_sample,
                last_publish_sample: if should_publish { end_sample } else {
                    previous.map(|active| active.last_publish_sample).unwrap_or(end_sample)
                },
            };
            note = Some(LiveNoteDraft { event, is_new, active, previous: if is_new { previous } else { None }, publish: should_publish });
            state.active_note = Some(active);
        } else if all_three_silent {
            state.active_note = None;
        }

        let mut key = None;
        if state.chroma_history.len() >= 8 {
            let distinct_pitch_classes = smoothed_chroma.iter().filter(|value| **value >= 0.12).count();
            if distinct_pitch_classes >= 3 {
                let candidates = estimate_key_candidates(&smoothed_chroma, 3);
                if let Some(best) = candidates.first().filter(|best| best.confidence >= 0.30) {
                    let margin = best.confidence - candidates.get(1).map(|next| next.confidence).unwrap_or(0.0);
                    if margin >= 0.025 {
                        let mode = match best.mode { KeyMode::Major => "Dur", KeyMode::Minor => "Moll" };
                        let label = format!("{} {mode}", best.tonic);
                        if state.last_key_candidate.as_deref() == Some(label.as_str()) {
                            state.key_candidate_streak = state.key_candidate_streak.saturating_add(1);
                        } else {
                            state.last_key_candidate = Some(label.clone());
                            state.key_candidate_streak = 1;
                        }
                        if state.key_candidate_streak >= 3 && best.confidence >= 0.45 {
                            let alternatives = candidates.iter().map(|candidate| {
                                let mode = match candidate.mode { KeyMode::Major => "Dur", KeyMode::Minor => "Moll" };
                                let rounded_percent = (candidate.confidence.clamp(0.0, 1.0) * 20.0).round() as i32 * 5;
                                format!("{} {mode} · {rounded_percent}%", candidate.tonic)
                            }).collect::<Vec<_>>();
                            let signature_changed = state.last_key_signature.as_deref() != Some(label.as_str());
                            let alternatives_changed = state.last_key_alternatives != alternatives;
                            if signature_changed || alternatives_changed {
                                key = Some(LiveKeyUpdate {
                                    alternatives: alternatives.clone(),
                                    confidence: best.confidence,
                                    signature: parse_key_name(best.tonic, best.mode),
                                });
                                state.last_key_signature = Some(label);
                                state.last_key_alternatives = alternatives;
                            }
                        }
                    } else {
                        state.key_candidate_streak = 0;
                        state.last_key_candidate = None;
                    }
                } else {
                    state.key_candidate_streak = 0;
                    state.last_key_candidate = None;
                }
            }
        }

        let previous_end = state.last_frame_end_sample;
        let skip_samples = previous_end.map_or(0, |previous| {
            previous.saturating_sub(frame.start_sample).min(frame.samples.len() as u64) as usize
        });
        if state.rolling_audio.is_empty() {
            state.rolling_audio_start_sample = frame.start_sample.saturating_add(skip_samples as u64);
        }
        for sample in frame.samples.iter().skip(skip_samples) {
            state.rolling_audio.push_back(if sample.is_finite() { sample.clamp(-1.0, 1.0) } else { 0.0 });
        }
        let maximum_beat_samples = (sample_rate as usize).saturating_mul(LIVE_BEAT_WINDOW_SECONDS);
        while state.rolling_audio.len() > maximum_beat_samples {
            state.rolling_audio.pop_front();
            state.rolling_audio_start_sample = state.rolling_audio_start_sample.saturating_add(1);
        }
        state.last_frame_start_sample = Some(frame.start_sample);
        state.last_frame_end_sample = Some(end_sample);

        let minimum_beat_samples = (sample_rate as usize).saturating_mul(3);
        let beat_interval = sample_rate_u64.saturating_mul(2);
        let beat_window = if state.rolling_audio.len() >= minimum_beat_samples
            && end_sample.saturating_sub(state.last_beat_analysis_sample) >= beat_interval
        {
            state.last_beat_analysis_sample = end_sample;
            Some((state.rolling_audio.iter().copied().collect(), state.rolling_audio_start_sample))
        } else {
            None
        };
        Ok(LiveFrameDecisions { chroma: smoothed_chroma, pitch: stable_pitch, publish_pitch, note, key, beat_window })
    }

    /// Zweck: Ersetzt nur den jüngsten Live-Beatfensterabschnitt und erhält stabile IDs sowie Nutzerkorrekturen.
    /// Eingaben: Lokales BeatGrid, globaler Sekundenstart des Rollingfensters und aktuelle Aufnahmezeit.
    /// Ausgabe: Aktualisiertes kanonisches Live-BeatGrid, `None` bei unverändertem Ergebnis oder Validierungsfehler.
    fn apply_live_beat_grid(
        &self,
        grid: &BeatGrid,
        window_start_seconds: f64,
        current_seconds: f64,
    ) -> Result<Option<BeatGrid>, String> {
        if !window_start_seconds.is_finite() || window_start_seconds < 0.0
            || !current_seconds.is_finite() || current_seconds < window_start_seconds
            || !grid.bpm.is_finite() || grid.bpm < 60.0 || grid.bpm > 200.0
        {
            return Err("Live-Beatgrid enthält eine ungültige Zeit- oder Tempobasis.".into());
        }
        let mut project = self.project.lock()
            .map_err(|_| "Songprojekt konnte für das Live-Beatgrid nicht gesperrt werden.".to_string())?;
        let old_beats = project.beats.clone();
        let user_edited = project.event_provenance.iter()
            .filter(|entry| entry.origin == EventOrigin::UserEdit)
            .map(|entry| entry.event_id)
            .collect::<HashSet<_>>();
        let beat_duration = 60.0 / grid.bpm;
        let tolerance = (beat_duration * 0.18).clamp(0.06, 0.20);
        let replace_before = (window_start_seconds - tolerance).max(0.0);
        let mut beats = old_beats.iter().copied()
            .filter(|beat| beat.time_seconds < replace_before || user_edited.contains(&beat.id))
            .collect::<Vec<_>>();
        let mut used_ids = beats.iter().map(|beat| beat.id).collect::<HashSet<_>>();
        let mut provenance_ids = Vec::new();
        for candidate in &grid.beats {
            let time_seconds = window_start_seconds + candidate.time_seconds;
            if time_seconds > current_seconds + tolerance || time_seconds > crate::timeline::MAX_TIMELINE_SECONDS {
                continue;
            }
            if old_beats.iter().any(|beat| {
                user_edited.contains(&beat.id) && (beat.time_seconds - time_seconds).abs() <= tolerance
            }) {
                continue;
            }
            let previous = old_beats.iter()
                .filter(|beat| !used_ids.contains(&beat.id))
                .min_by(|left, right| {
                    (left.time_seconds - time_seconds).abs()
                        .total_cmp(&(right.time_seconds - time_seconds).abs())
                })
                .filter(|beat| (beat.time_seconds - time_seconds).abs() <= tolerance);
            let beat = if let Some(previous) = previous {
                used_ids.insert(previous.id);
                if !project.event_provenance.iter().any(|entry| {
                    entry.event_id == previous.id
                        && entry.origin == EventOrigin::LiveDsp
                        && entry.provider.as_deref() == Some("livetab-live-beat")
                }) {
                    provenance_ids.push(previous.id);
                }
                BeatEvent { id: previous.id, time_seconds, is_downbeat: previous.is_downbeat, confidence: candidate.confidence }
            } else {
                let event = BeatEvent::new(time_seconds, candidate.is_downbeat, candidate.confidence)
                    .map_err(|error| error.to_string())?;
                used_ids.insert(event.id);
                provenance_ids.push(event.id);
                event
            };
            beats.push(beat);
        }
        beats.sort_by(|left, right| left.time_seconds.total_cmp(&right.time_seconds));
        let mut next = project.clone();
        next.beats = beats;
        next.bpm = grid.bpm;
        next.duration_seconds = next.duration_seconds.max(current_seconds).max(next.calculated_duration_seconds());
        let active_beat_ids = next.beats.iter().map(|beat| beat.id).collect::<HashSet<_>>();
        let old_beat_ids = old_beats.iter().map(|beat| beat.id).collect::<HashSet<_>>();
        next.event_provenance.retain(|entry| {
            !old_beat_ids.contains(&entry.event_id) || active_beat_ids.contains(&entry.event_id)
        });
        if next.canonical_event_count() > MAX_PROJECT_EVENTS {
            return Err("Live-Beatgrid würde das sichere Projekt-Eventlimit überschreiten.".into());
        }
        let provenance = provenance_ids.into_iter().map(|event_id| (
            event_id, EventOrigin::LiveDsp,
            Some("livetab-live-beat".into()),
            Some("Geglättete Beat-/Downbeat-Schätzung aus dem Rollingfenster".into()),
        )).collect::<Vec<_>>();
        let provenance_changed = !provenance.is_empty();
        let previous_revision = project.revision;
        if !provenance.is_empty() {
            next.record_provenance_batch(provenance)?;
        }
        let changed = provenance_changed || next.beats != project.beats || (next.bpm - project.bpm).abs() >= 0.1
            || next.duration_seconds > project.duration_seconds;
        if !changed {
            return Ok(None);
        }
        next.revision = previous_revision;
        next.touch();
        next.validate()?;
        let result = BeatGrid {
            bpm: next.bpm,
            offset_seconds: window_start_seconds + grid.offset_seconds,
            confidence: grid.confidence,
            time_signature: next.time_signature,
            beats: next.beats.clone(),
        };
        *project = next;
        Ok(Some(result))
    }

    /// Zweck: Verarbeitet ein Mono-Audioframe in chord-, pitch-, tonart- und beatbewusste Projektupdates.
    /// Eingaben: Mono-Audioframe mit Sample-Rate und absoluter Startsampleposition.
    /// Ausgabe: Neue Akkordschätzung oder `None` bei Stille, geringer Energie oder erreichtem Eventlimit.
    pub fn process_live_frame(&self, frame: &AudioFrame) -> Result<Option<ChordEvent>, String> {
        let started = std::time::Instant::now();
        let level = measure_level(&frame.samples);
        let chroma = extract_chroma(&frame.samples, frame.sample_rate);
        let raw_pitch = if level.rms >= 0.003 {
            estimate_pitch(&frame.samples, frame.sample_rate, 70.0, 1_100.0)
        } else {
            None
        };
        let decisions = self.update_live_dsp_state(frame, level, chroma, raw_pitch)?;
        let start = frame.start_sample as f64 / f64::from(frame.sample_rate);
        let end = frame.start_sample.saturating_add(frame.samples.len() as u64) as f64
            / f64::from(frame.sample_rate);
        self.events.publish(AppEvent::AudioLevel {
            rms: level.rms,
            peak: level.peak,
            waveform: waveform_bars(&frame.samples, 48),
        });

        if decisions.publish_pitch {
            let pitch = decisions.pitch.map(|pitch| pitch.estimate);
            self.events.publish(AppEvent::LivePitch {
                time_seconds: end,
                midi: pitch.map(|estimate| estimate.midi),
                frequency_hz: pitch.map(|estimate| estimate.frequency_hz),
                confidence: pitch.map(|estimate| estimate.confidence).unwrap_or(0.0),
            });
        }

        if let Some(draft) = decisions.note {
            let mut project = self.project.lock()
                .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
            if let Some(previous) = draft.previous {
                if let Some(old) = project.notes.iter().find(|event| event.id() == previous.id).cloned() {
                    let duration = draft.event.start_seconds() - old.start_seconds();
                    if duration >= 0.005 {
                        let closed = NoteEvent::with_metadata_and_id(
                            old.id(), old.midi(), old.start_seconds(), duration,
                            old.velocity(), old.confidence(),
                        ).map_err(|error| error.to_string())?;
                        project.upsert_note(closed);
                    }
                }
            }
            let inserted = draft.is_new;
            if inserted && project.canonical_event_count() >= MAX_PROJECT_EVENTS {
                if let Ok(mut state) = self.live_dsp.lock() {
                    state.active_note = None;
                }
                self.observer.set_worker_status("Projekt-Eventlimit erreicht; Live-Pitch bleibt sichtbar, neue NoteEvents werden nicht mehr gespeichert.");
            } else if inserted {
                let mut next = project.clone();
                next.upsert_note(draft.event.clone());
                next.record_provenance(
                    draft.event.id(), EventOrigin::LiveDsp,
                    Some("livetab-live-pitch".into()),
                    Some("Geglättetes monophones Live-NoteEvent".into()),
                )?;
                next.revision = project.revision;
                next.touch();
                next.validate()?;
                *project = next;
                if draft.publish {
                    self.events.publish(AppEvent::NoteDetected(draft.event));
                }
            } else if project.notes.iter().any(|event| event.id() == draft.event.id()) {
                project.upsert_note(draft.event.clone());
                if draft.publish {
                    self.events.publish(AppEvent::NoteDetected(draft.event));
                }
            }
        }

        if let Some(update) = decisions.key {
            if let Some(signature) = update.signature {
                if let Ok(mut project) = self.project.lock() {
                    if project.key_signature != Some(signature) {
                        project.key_signature = Some(signature);
                        project.touch();
                    }
                }
            }
            if let Ok(mut alternatives) = self.key_alternatives.lock() {
                *alternatives = update.alternatives.clone();
            }
            self.events.publish(AppEvent::LiveKeyCandidates {
                alternatives: update.alternatives,
                confidence: update.confidence,
            });
        }

        if let Some((samples, window_start_sample)) = decisions.beat_window {
            let time_signature = self.project.lock()
                .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?
                .time_signature;
            let window_duration = samples.len() as f64 / f64::from(frame.sample_rate);
            if let Ok(mut grid) = estimate_beat_grid(&samples, frame.sample_rate, window_duration, time_signature) {
                if grid.confidence >= 0.15 {
                    let stable_bpm = if let Ok(mut state) = self.live_dsp.lock() {
                        state.beat_bpm_history.push_back(grid.bpm);
                        while state.beat_bpm_history.len() > 3 {
                            state.beat_bpm_history.pop_front();
                        }
                        if state.beat_bpm_history.len() >= 2 {
                            let mut values = state.beat_bpm_history.iter().copied().collect::<Vec<_>>();
                            values.sort_by(f64::total_cmp);
                            Some(values[values.len() / 2])
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(bpm) = stable_bpm {
                        grid.bpm = bpm;
                        let window_start = window_start_sample as f64 / f64::from(frame.sample_rate);
                        match self.apply_live_beat_grid(&grid, window_start, end) {
                            Ok(Some(applied)) => {
                                self.events.publish(AppEvent::BeatGridUpdated {
                                    bpm: applied.bpm,
                                    beats: applied.beats,
                                    confidence: applied.confidence,
                                });
                            }
                            Ok(None) => {}
                            Err(message) => self.observer.log(
                                LogLevel::Warning, LogCategory::BeatTracking, message.clone(),
                                "Live-Beatgrid wurde nicht ins Projekt übernommen.", BTreeMap::new(), None,
                            ),
                        }
                    }
                }
            }
        }

        let analysis = analyze_chroma(&decisions.chroma);
        let Some(estimate) = analysis.chord.filter(|estimate| estimate.confidence >= 0.18) else {
            return Ok(None);
        };
        let root = parse_note_name(estimate.root)
            .ok_or_else(|| "Akkordklassifizierer lieferte einen unbekannten Grundton.".to_string())?;
        let chord = Chord { root, quality: estimate.quality };
        let event = ChordEvent::with_confidence(chord, start, end, estimate.confidence)
            .map_err(|error| error.to_string())?;
        let (changed, event_limit_reached) = {
            let mut project = self.project.lock()
                .map_err(|_| "Songprojekt konnte nicht gesperrt werden.".to_string())?;
            let can_add_event = project.canonical_event_count() < MAX_PROJECT_EVENTS;
            if let Some(last) = project.chords.last_mut() {
                if last.chord() == chord && start <= last.end_seconds() + 0.15 {
                    *last = ChordEvent::with_event_id(
                        last.id(), chord, last.start_seconds(), end.max(last.end_seconds()),
                        last.confidence().max(event.confidence()),
                    ).map_err(|error| error.to_string())?;
                    project.touch();
                    (None, false)
                } else if !can_add_event {
                    (None, true)
                } else {
                    project.chords.push(event);
                    project.touch();
                    (Some(event), false)
                }
            } else if !can_add_event {
                (None, true)
            } else {
                project.chords.push(event);
                project.touch();
                (Some(event), false)
            }
        };
        if event_limit_reached {
            self.observer.set_worker_status("Projekt-Eventlimit erreicht; Live-Audio läuft ohne weitere Akkordübernahme weiter.");
            return Ok(None);
        }
        if let Some(changed_event) = changed {
            if let Ok(mut project) = self.project.lock() {
                let _ = project.record_provenance(
                    changed_event.id(), EventOrigin::LiveDsp,
                    Some("livetab-live-dsp".into()),
                    Some("Konfidenzbehafteter Live-Akkordframe".into()),
                );
            }
            self.events.publish(AppEvent::ChordDetected(changed_event));
            self.observer.log(
                LogLevel::Info, LogCategory::ChordDetection,
                format!("Liveakkord {} Confidence {:.2}", chord.name(), estimate.confidence),
                format!("Akkord erkannt: {}", chord.name()), BTreeMap::new(),
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
        let mut next = project.clone();
        let previous_revision = next.revision;
        next.upsert_tab_track(track.clone());
        let provenance = std::iter::once((
            track.id, EventOrigin::RuleBasedGeneration,
            Some("livetab-tab-engine".into()), Some("Regelbasierte Tabspur".into()),
        )).chain(track.notes.iter().map(|note| (
            note.position.id(), EventOrigin::RuleBasedGeneration,
            Some("livetab-tab-engine".into()), Some("Regelbasierte Saiten-/Bundzuordnung".into()),
        ))).collect();
        next.record_provenance_batch(provenance)?;
        next.revision = previous_revision;
        next.touch();
        next.validate()?;
        self.push_edit_snapshot(&project)?;
        *project = next;
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
            ChordEvent::with_event_id(old.id(), chord, old.start_seconds(), old.end_seconds(), 1.0)
                .map_err(|error| error.to_string())?;
        let mut next = project.clone();
        next.chords[index] = corrected;
        next.record_provenance(
            corrected.id(), EventOrigin::UserEdit, Some("LiveTabulator-Editor".into()),
            Some("Akkord manuell korrigiert".into()),
        )?;
        next.validate()?;
        self.push_edit_snapshot(&project)?;
        *project = next;
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
            let mut next = project.clone();
            let replaced_ids = next.beats.iter().map(|beat| beat.id).collect::<HashSet<_>>();
            next.event_provenance.retain(|entry| !replaced_ids.contains(&entry.event_id));
            next.beats = beats.clone();
            let provenance = beats.iter().map(|beat| (
                beat.id, EventOrigin::UserEdit, Some("LiveTabulator-Editor".into()),
                Some("Downbeat manuell verschoben".into()),
            )).collect();
            next.record_provenance_batch(provenance)?;
            next.validate()?;
            self.push_edit_snapshot(&project)?;
            *project = next;
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
        if line_index >= project.lyrics_lines.len() {
            return Err("Lyrics-Zeilenindex liegt außerhalb der Liste.".into());
        }
        let mut next = project.clone();
        let replaced_ids = next.lyrics_lines[line_index].words.iter()
            .map(LyricsWord::id).collect::<HashSet<_>>();
        let start = next.lyrics_lines[line_index].start_seconds;
        let end = next.lyrics_lines[line_index].end_seconds;
        let tokens = text.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        if tokens.is_empty() {
            return Err("Lyrics-Zeile enthält keine Wörter.".into());
        }
        let word_count = tokens.len();
        let step = (end - start) / word_count as f64;
        let mut new_words = Vec::with_capacity(word_count);
        for (index, token) in tokens.into_iter().enumerate() {
            let word_start = start + index as f64 * step;
            let word_end = if index + 1 == word_count { end } else { start + (index + 1) as f64 * step };
            new_words.push(LyricsWord::with_confidence(token, word_start, word_end, 1.0)
                .map_err(|error| error.to_string())?);
        }
        let new_word_ids = new_words.iter().map(LyricsWord::id).collect::<Vec<_>>();
        let line = next.lyrics_lines
            .get_mut(line_index)
            .ok_or_else(|| "Lyrics-Zeilenindex liegt außerhalb der Liste.".to_string())?;
        line.text = text.trim().to_string();
        line.words = new_words;
        next.lyrics_words = next
            .lyrics_lines
            .iter()
            .flat_map(|item| item.words.iter().cloned())
            .collect();
        next.event_provenance.retain(|entry| !replaced_ids.contains(&entry.event_id));
        let provenance = new_word_ids.into_iter().map(|event_id| (
            event_id, EventOrigin::UserEdit, Some("LiveTabulator-Editor".into()),
            Some("Lyrics-Wort manuell korrigiert".into()),
        )).collect();
        next.record_provenance_batch(provenance)?;
        next.validate()?;
        self.push_edit_snapshot(&project)?;
        *project = next;
        drop(project);
        self.rebuild_playback()?;
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
        let previous_revision = current.revision;
        let mut next = current.clone();
        next.replace_lyrics(result.words.clone(), result.lines.clone());
        let language = result.language.as_deref().unwrap_or("unbekannt");
        let provenance = result.words.iter().map(|word| (
            word.id(), EventOrigin::LocalTranscription,
            Some("lokales Lyrics-Backend".into()),
            Some(format!("Lokale Worttranskription; Sprache {language}")),
        )).collect::<Vec<_>>();
        if !provenance.is_empty() {
            next.record_provenance_batch(provenance)?;
        }
        next.revision = previous_revision;
        next.touch();
        next.validate()?;
        *current = next;
        drop(current);
        self.rebuild_playback()?;
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

    /// Zweck: Springt in Sekunden und veröffentlicht den an dieser Position aktiven musikalischen Zustand.
    /// Eingaben: Zielposition in Songsekunden; sie muss innerhalb Songdauer und aktivem Loop liegen.
    /// Ausgabe: Synchron rekonstruierter PlaybackTick oder Validierungs-/Schedulerfehler.
    pub fn seek_playback(&self, seconds: f64) -> Result<PlaybackTick, String> {
        self.ensure_playback_scheduler()?;
        let tick = self.playback.lock()
            .map_err(|_| "Playbackstatus konnte nicht gesperrt werden.".to_string())?
            .as_mut()
            .ok_or_else(|| "Playback-Scheduler wurde noch nicht aufgebaut.".to_string())?
            .seek(seconds)?;
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
        let timeline = project.timeline_map()?;
        let scheduler = PlaybackScheduler::new_with_timeline(
            project.duration_seconds,
            project.bpm,
            project.time_signature,
            project.notes,
            project.lyrics_words,
            timeline,
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

    /// Zweck: Prüft Rolling-Beatgrid-Übernahme mit stabilen IDs, geschütztem Nutzerbeat und LiveProvenance.
    /// Eingaben: Drei vorhandene Beats, davon einer manuell geschützt, sowie ein synthetisches Vier-Beat-Grid.
    /// Ausgabe: Keine; Assertions sichern Zeitaktualisierung, ID-Erhalt, Nutzerkorrektur und Eventherkunft.
    #[test]
    fn rolling_live_beatgrid_preserves_ids_and_user_edits() {
        let controller = AppController::new("Test", "DBZS");
        let automatic_first = BeatEvent::new(0.0, true, 0.7).unwrap();
        let user_beat = BeatEvent::new(0.5, false, 0.8).unwrap();
        let automatic_second = BeatEvent::new(1.0, false, 0.7).unwrap();
        {
            let mut project = controller.project.lock().unwrap();
            project.duration_seconds = 2.0;
            project.beats = vec![automatic_first, user_beat, automatic_second];
            project.record_provenance(
                user_beat.id, EventOrigin::UserEdit,
                Some("test-editor".into()), Some("geschützter Downbeat-Test".into()),
            ).unwrap();
        }
        let grid = BeatGrid {
            bpm: 120.0,
            offset_seconds: 0.02,
            confidence: 0.9,
            time_signature: crate::domain::TimeSignature::default(),
            beats: vec![
                BeatEvent::new(0.02, true, 0.9).unwrap(),
                BeatEvent::new(0.52, false, 0.9).unwrap(),
                BeatEvent::new(1.02, false, 0.9).unwrap(),
                BeatEvent::new(1.52, false, 0.9).unwrap(),
            ],
        };
        let applied = controller.apply_live_beat_grid(&grid, 0.0, 2.0).unwrap().unwrap();
        let project = controller.snapshot().unwrap();
        assert_eq!(applied.beats.len(), 4);
        assert_eq!(project.beats[0].id, automatic_first.id);
        assert_eq!(project.beats[0].time_seconds, 0.02);
        let protected = project.beats.iter().find(|beat| beat.id == user_beat.id).unwrap();
        assert_eq!(protected.time_seconds, 0.5);
        assert!(!protected.is_downbeat);
        assert_eq!(project.beats[2].id, automatic_second.id);
        let inserted = project.beats.iter().find(|beat| (beat.time_seconds - 1.52).abs() < 1e-9).unwrap();
        assert!(project.event_provenance.iter().any(|entry| {
            entry.event_id == inserted.id && entry.origin == EventOrigin::LiveDsp
        }));
        assert!(project.event_provenance.iter().any(|entry| {
            entry.event_id == user_beat.id && entry.origin == EventOrigin::UserEdit
        }));
    }

    /// Zweck: Verhindert Änderungen am kanonischen Projekt, wenn die Rollingfensterzeit ungültig ist.
    /// Eingaben: Controller-Snapshot und Beatgrid mit NaN-Startzeit.
    /// Ausgabe: Keine; Assertions sichern einen Fehler ohne Änderung an Revision oder Beatliste.
    #[test]
    fn live_beatgrid_rejects_invalid_time_without_mutation() {
        let controller = AppController::new("Test", "DBZS");
        let grid = BeatGrid {
            bpm: 120.0,
            offset_seconds: 0.0,
            confidence: 0.8,
            time_signature: crate::domain::TimeSignature::default(),
            beats: vec![BeatEvent::new(0.0, true, 0.8).unwrap()],
        };
        let before = controller.snapshot().unwrap();
        assert!(controller.apply_live_beat_grid(&grid, f64::NAN, 1.0).is_err());
        let after = controller.snapshot().unwrap();
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.beats, before.beats);
    }

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

    /// Zweck: Prüft, dass ein kooperativ abgebrochener Offlinepass keine Projektteile übernimmt.
    /// Eingaben: Synthetische lokale WAV-Datei, bestehende Projekt-Audioreferenz und sofortiger Abbruch.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Audiopfad, Dauer, Akkord-ID und Revision.
    #[test]
    fn cancelled_analysis_preserves_the_existing_project() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("cancelled.wav");
        let samples = (0..16_000).map(|index| {
            (std::f32::consts::TAU * 220.0 * index as f32 / 8_000.0).sin() * 0.2
        }).collect::<Vec<_>>();
        crate::audio::write_wav(&input, &samples, 8_000, 1).unwrap();
        let controller = AppController::new("Test", "DBZS");
        let old_audio = dir.path().join("old.wav");
        let existing = ChordEvent::new(
            Chord { root: NoteName::A, quality: ChordQuality::Minor }, 0.0, 1.0,
        ).unwrap();
        {
            let mut project = controller.project.lock().unwrap();
            project.audio_path = Some(old_audio.clone());
            project.duration_seconds = 5.0;
            project.chords.push(existing);
            project.touch();
        }
        let before = controller.snapshot().unwrap();
        assert!(controller.analyze_audio_file_with_cancel(&input, || true).is_err());
        let after = controller.snapshot().unwrap();
        assert_eq!(after.audio_path, Some(old_audio));
        assert_eq!(after.duration_seconds, before.duration_seconds);
        assert_eq!(after.chords[0].id(), before.chords[0].id());
        assert_eq!(after.revision, before.revision);
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

    /// Zweck: Prüft Import von Mock-Workerlyrics einschließlich Wort-IDs, Provenienz und Scheduler-Rebuild.
    /// Eingaben: Zwei synthetische lokale Transkriptionssegmente und ein Projekt mit lokaler Audioreferenz.
    /// Ausgabe: Keine; Assertions sichern transaktionshafte Wortübernahme und `LocalTranscription`-Provenienz.
    #[test]
    fn local_transcription_records_provenance_for_canonical_words() {
        struct FixedBackend;
        impl LyricsBackend for FixedBackend {
            /// Zweck: Liefert feste Testwörter ohne Prozess, Datei- oder Audiozugriff.
            /// Eingaben: Ignorierter Testpfad und optionale Sprache.
            /// Ausgabe: Zwei synthetische Wortsegmente samt deutscher Sprachkennung.
            fn transcribe(
                &self,
                _: &Path,
                _: Option<&str>,
            ) -> Result<crate::lyrics::TranscriptionOutput, String> {
                Ok(crate::lyrics::TranscriptionOutput {
                    words: vec![
                        crate::lyrics::TranscribedWord {
                            text: "Guten".into(), start_seconds: 0.1, end_seconds: 0.4, confidence: 0.9,
                        },
                        crate::lyrics::TranscribedWord {
                            text: "Morgen".into(), start_seconds: 0.5, end_seconds: 0.9, confidence: 0.8,
                        },
                    ],
                    language: Some("de".into()),
                })
            }
        }
        let controller = AppController::new("Test", "DBZS");
        {
            let mut project = controller.project.lock().unwrap();
            project.audio_path = Some(PathBuf::from("mock-local-audio.wav"));
            project.duration_seconds = 1.0;
        }
        let result = controller.transcribe_lyrics(&FixedBackend, None).unwrap();
        let project = controller.snapshot().unwrap();
        assert_eq!(project.lyrics_words.len(), 2);
        assert_eq!(result.words.len(), 2);
        for word in &project.lyrics_words {
            assert!(project.event_provenance.iter().any(|entry| {
                entry.event_id == word.id() && entry.origin == EventOrigin::LocalTranscription
            }));
        }
    }
}
