/*
===============================================================================
Division By Zeros (DBZS) - Slint-Anwendungsworkflows
Modul: app_workflows
Zweck: Bindet die fachlogikfreie GUI an AppController, JobManager, lokale
       Audioquellen, Modellregistry, Projektpersistenz und Observer.
Sprache: Rust
Hinweise:
- Keine langen Operationen laufen im Slint-Ereignisloop.
- Audio, Lyrics, Projekte und Modelle bleiben auf dem Gerät.
- Die UI erhält Statusereignisse über schwache Handles und typisierte Kanäle.
===============================================================================
*/
use crate::audio_capture::{self, CaptureSession};
use crate::playback_output::PlaybackOutputSession;
use crate::stem_workflow::{self, StemRunContext};
use crate::{MainWindow, ModelCatalog, application_data_directory};
use livetab_core::audio::{read_wav, write_wav};
use livetab_core::event_bus::AppEvent;
use livetab_core::export::load_dbztab;
use livetab_core::jobs::{JobEvent, JobHandle, JobKind, JobState};
use livetab_core::lyrics::PythonWorkerLyricsBackend;
use livetab_core::model_registry::{ModelRole, ModelStatus};
use livetab_core::project_model::{GuitarTuning, StemKind};
use livetab_core::tab_engine::{describe_tuning, format_ascii_tab};
use livetab_core::{AppController, SongProject};
#[cfg(windows)]
use rfd::FileDialog;
use slint::{ComponentHandle, Weak};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Obergrenze für den gemeinsamen WAV-Dateibestand bei Original plus synchronisierten Stems.
const MAX_SYNC_PLAYBACK_FILE_BYTES: u64 = 384 * 1024 * 1024;

/// Besitz der langlaufenden Workerflags, die nach dem Schließen kontrolliert gestoppt werden.
pub struct AppLifecycle {
    shutdown: Arc<AtomicBool>,
    capture_requested: Arc<AtomicBool>,
    capture: Arc<Mutex<Option<CaptureSession>>>,
    playback: Arc<Mutex<Option<PlaybackOutputSession>>>,
    stem_job: Arc<Mutex<Option<JobHandle>>>,
}

impl AppLifecycle {
    /// Zweck: Fordert den Stopp von Playback-/Eventpumpen an und beendet eine aktive Capture-Session.
    /// Eingaben: `self` – Lifecycle der geschlossenen App.
    /// Ausgabe: Keine; Audio-Worker erhalten das atomare Stoppsignal.
    pub fn stop(self) {
        self.shutdown.store(true, Ordering::Release);
        self.capture_requested.store(false, Ordering::Release);
        if let Ok(mut active) = self.stem_job.lock() {
            if let Some(job) = active.take() {
                job.cancellation.cancel();
            }
        }
        if let Ok(mut capture) = self.capture.lock() {
            if let Some(session) = capture.take() {
                thread::spawn(move || session.stop());
            }
        }
        if let Ok(mut playback) = self.playback.lock() {
            if let Some(session) = playback.take() {
                thread::spawn(move || drop(session));
            }
        }
    }
}

/// Zweck: Registriert sämtliche Workflowcallbacks und startet Event-/Jobdispatcher.
/// Eingaben: Slint-Fenster, geteilten `AppController` und rekursiv befüllten Modellkatalog.
/// Ausgabe: Lifecyclehandle, das nach `ui.run()` die laufenden Hintergrundworker stoppt.
pub fn wire(ui: &MainWindow, controller: AppController, models: ModelCatalog) -> AppLifecycle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let capture_requested = Arc::new(AtomicBool::new(false));
    let capture = Arc::new(Mutex::new(None));
    let playback: Arc<Mutex<Option<PlaybackOutputSession>>> = Arc::new(Mutex::new(None));
    let deep_job: Arc<Mutex<Option<JobHandle>>> = Arc::new(Mutex::new(None));
    let stem_job: Arc<Mutex<Option<JobHandle>>> = Arc::new(Mutex::new(None));
    let recording_wav_saved = Arc::new(AtomicBool::new(false));
    let pending_stem_download: Arc<Mutex<Option<StemRunContext>>> = Arc::new(Mutex::new(None));
    let active_audio_source_id = Arc::new(Mutex::new("source".to_string()));
    let playback_clock_started = Arc::new(AtomicBool::new(false));
    start_event_dispatcher(
        ui.as_weak(),
        controller.clone(),
        Arc::clone(&shutdown),
        Arc::clone(&capture_requested),
        Arc::clone(&capture),
    );
    start_job_dispatcher(
        ui.as_weak(),
        controller.clone(),
        Arc::clone(&shutdown),
        Arc::clone(&deep_job),
        Arc::clone(&stem_job),
        Arc::clone(&pending_stem_download),
        Arc::clone(&recording_wav_saved),
    );

    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_open_audio_requested(move || {
            let Some(handle) = weak.upgrade() else { return };
            let mut path = PathBuf::from(handle.get_audio_path_text().to_string());
            if path.as_os_str().is_empty() {
                path = match choose_audio_path() {
                    Some(path) => path,
                    None => { handle.set_audio_status_text("Audiodateiauswahl abgebrochen oder auf dieser Plattform nicht verfügbar.".into()); return; }
                };
                handle.set_audio_path_text(path.to_string_lossy().into_owned().into());
            }
            open_audio_async(weak.clone(), controller.clone(), path);
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_select_audio_requested(move || {
            if let Some(path) = choose_audio_path() {
                if let Some(handle) = weak.upgrade() {
                    handle.set_audio_path_text(path.to_string_lossy().into_owned().into());
                }
                open_audio_async(weak.clone(), controller.clone(), path);
            } else if let Some(handle) = weak.upgrade() {
                handle.set_audio_status_text(
                    "Audiodateiauswahl abgebrochen oder auf dieser Plattform nicht verfügbar."
                        .into(),
                );
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_scan_audio_devices_requested(move || {
            if let Some(handle) = weak.upgrade() { handle.set_audio_device_status_text("Lokale Audioendpunkte werden aufgelistet …".into()); }
            let weak = weak.clone();
            thread::spawn(move || {
                let result = audio_capture::available_audio_devices();
                let _ = weak.upgrade_in_event_loop(move |handle| match result {
                    Ok(names) => {
                        let model = slint::ModelRc::new(std::rc::Rc::new(slint::VecModel::from(names.iter().cloned().map(slint::SharedString::from).collect::<Vec<_>>())));
                        handle.set_audio_device_names(model);
                        let status = if cfg!(windows) {
                            let wasapi_count = names.iter().filter(|name| name.starts_with("WASAPI · ")).count();
                            let asio_count = names.iter().filter(|name| name.starts_with("ASIO · ")).count();
                            format!("{wasapi_count} WASAPI-Systemmixquelle(n) · {asio_count} ASIO-Treiber; Auswahl gilt beim nächsten Live-Start.")
                        } else {
                            format!("{} lokale Quelle(n) verfügbar; Auswahl gilt beim nächsten Live-Start.", names.len())
                        };
                        handle.set_audio_device_status_text(status.into());
                    }
                    Err(error) => handle.set_audio_device_status_text(error.into()),
                });
            });
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_audio_device_selected(move |name| {
            let value = name.to_string();
            match audio_capture::select_audio_device(Some(value.clone())) {
                Ok(()) => {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_audio_device_status_text(
                            format!("Ausgewählt: {value} · gilt ab dem nächsten Start").into(),
                        );
                    }
                }
                Err(error) => {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_audio_device_status_text(error.into());
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        let capture = Arc::clone(&capture);
        let requested = Arc::clone(&capture_requested);
        let shutdown = Arc::clone(&shutdown);
        ui.on_start_live_requested(move || {
            if shutdown.load(Ordering::Acquire) || requested.swap(true, Ordering::AcqRel) {
                return;
            }
            if let Some(handle) = weak.upgrade() {
                handle.set_live_running(true);
                handle.set_audio_status_text("Lokale Audioquelle wird geöffnet …".into());
                handle.set_worker_status_text("Liveaufnahme wird initialisiert".into());
            }
            let weak_for_worker = weak.clone();
            let controller = controller.clone();
            let capture = Arc::clone(&capture);
            let requested = Arc::clone(&requested);
            thread::spawn(move || match audio_capture::start_capture(controller) {
                Ok(session) => {
                    if requested.load(Ordering::Acquire) {
                        if let Ok(mut active) = capture.lock() {
                            *active = Some(session);
                        }
                    } else {
                        session.stop();
                    }
                }
                Err(message) => {
                    requested.store(false, Ordering::Release);
                    let _ = weak_for_worker.upgrade_in_event_loop(move |handle| {
                        handle.set_live_running(false);
                        handle.set_audio_status_text(message.clone().into());
                        handle.set_status_text(message.into());
                    });
                }
            });
        });
    }
    {
        let weak = ui.as_weak();
        let capture = Arc::clone(&capture);
        let requested = Arc::clone(&capture_requested);
        ui.on_stop_live_requested(move || {
            requested.store(false, Ordering::Release);
            if let Some(handle) = weak.upgrade() {
                handle.set_live_running(false);
                handle.set_audio_status_text("Liveaufnahme wird gestoppt …".into());
            }
            if let Ok(mut active) = capture.lock() {
                if let Some(session) = active.take() {
                    thread::spawn(move || session.stop());
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let capture = Arc::clone(&capture);
        let controller = controller.clone();
        let deep_job = Arc::clone(&deep_job);
        let recording_wav_saved = Arc::clone(&recording_wav_saved);
        ui.on_toggle_recording_requested(move || {
            let Some(handle) = weak.upgrade() else { return };
            if !handle.get_recording() {
                let result = capture
                    .lock()
                    .map_err(|_| "Audio-Capture-Session ist gesperrt.".to_string())
                    .and_then(|active| {
                        active
                            .as_ref()
                            .ok_or_else(|| "Bitte zuerst die Live-Aufnahme starten.".to_string())?
                            .begin_recording()
                    });
                match result {
                    Ok(()) => {
                        handle.set_recording(true);
                        handle.set_audio_status_text(
                            "WAV-Aufnahme läuft · maximal zehn Minuten".into(),
                        );
                    }
                    Err(error) => handle.set_status_text(error.into()),
                }
                return;
            }
            let Some(path) = choose_recording_save_path() else {
                handle.set_audio_status_text(
                    "WAV-Speichern abgebrochen; die Aufnahme läuft weiter.".into(),
                );
                return;
            };
            let recording = capture
                .lock()
                .map_err(|_| "Audio-Capture-Session ist gesperrt.".to_string())
                .and_then(|active| {
                    active
                        .as_ref()
                        .ok_or_else(|| "Live-Capture wurde beendet.".to_string())?
                        .finish_recording()
                });
            let (samples, sample_rate) = match recording {
                Ok(recording) => recording,
                Err(error) => {
                    handle.set_status_text(error.into());
                    return;
                }
            };
            handle.set_recording(false);
            handle.set_deep_running(true);
            handle.set_busy(true);
            handle.set_worker_status_text(
                "Liveaufnahme wird lokal gespeichert und anschließend vollständig analysiert …"
                    .into(),
            );
            recording_wav_saved.store(false, Ordering::Release);
            let manager = controller.job_manager();
            let analysis_controller = controller.clone();
            let weak_after_save = weak.clone();
            let weak_after_analysis = weak.clone();
            let path_for_ui = path.clone();
            let recording_wav_saved = Arc::clone(&recording_wav_saved);
            let job = manager.start(JobKind::DeepAnalysis, move |reporter, cancellation| {
                reporter.progress(0.05, "Live-Audio wird als lokale Mono-WAV gespeichert");
                let saved_path_for_ui = path.clone();
                let save_reporter = reporter.clone();
                let summary = save_and_analyze_recording(
                    &analysis_controller,
                    &path,
                    samples,
                    sample_rate,
                    || cancellation.is_cancelled(),
                    move |waveform, duration_seconds, rate| {
                        recording_wav_saved.store(true, Ordering::Release);
                        save_reporter.progress(
                            0.2,
                            "WAV gesichert · vollständige lokale DSP-Analyse läuft",
                        );
                        let _ = weak_after_save.upgrade_in_event_loop(move |handle| {
                            handle.set_audio_path_text(
                                saved_path_for_ui.to_string_lossy().into_owned().into(),
                            );
                            handle.set_audio_status_text(
                                format!(
                                    "Live-Audio-WAV gesichert · {:.2}s · {rate} Hz · Offline-Analyse läuft",
                                    duration_seconds
                                )
                                .into(),
                            );
                            handle.set_waveform_text(waveform.into());
                        });
                    },
                )?;
                cancellation.check()?;
                reporter.progress(0.95, "Analyseergebnis wird in das Songprojekt übernommen");
                let message = format!(
                    "Aufnahme gespeichert und analysiert: {} Akkorde · {} Noten · {} Frames · {}",
                    summary.chord_count,
                    summary.note_count,
                    summary.analyzed_frames,
                    path.display()
                );
                let completed_path = path_for_ui.clone();
                let _ = weak_after_analysis.upgrade_in_event_loop(move |handle| {
                    handle.set_audio_path_text(
                        completed_path.to_string_lossy().into_owned().into(),
                    );
                    handle.set_audio_status_text(
                        format!(
                            "Live-Audio analysiert · {:.2}s · {} Hz · {} Akkorde · {} Noten",
                            summary.duration_seconds,
                            summary.sample_rate,
                            summary.chord_count,
                            summary.note_count
                        )
                        .into(),
                    );
                    update_project_view(&handle, &analysis_controller);
                });
                reporter.progress(1.0, "Live-Audio-WAV gespeichert und offline analysiert");
                Ok(message)
            });
            if let Ok(mut active) = deep_job.lock() {
                *active = Some(job);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        let deep_job = Arc::clone(&deep_job);
        ui.on_run_deep_analysis_requested(move || {
            let Some(handle) = weak.upgrade() else { return };
            let snapshot = match controller.snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    handle.set_status_text(error.into());
                    return;
                }
            };
            let Some(path) = snapshot.audio_path.clone() else {
                handle.set_status_text("Bitte zuerst eine lokale WAV-Datei öffnen.".into());
                return;
            };
            handle.set_deep_running(true);
            handle.set_busy(true);
            handle.set_worker_status_text("Abbrechbare lokale DSP-Analyse läuft …".into());
            let controller = controller.clone();
            let manager = controller.job_manager();
            let job = manager.start(JobKind::DeepAnalysis, move |reporter, cancellation| {
                cancellation.check()?;
                reporter.progress(0.02, "WAV wird dekodiert");
                let result = controller
                    .analyze_audio_file_with_cancel(&path, || cancellation.is_cancelled())?;
                cancellation.check()?;
                reporter.progress(0.95, "Projektmodell und Playback werden aktualisiert");
                Ok(format!(
                    "{} Akkorde · {} Noten · {} analysierte Frames",
                    result.chords.len(),
                    result.notes.len(),
                    result.analyzed_frames
                ))
            });
            if let Ok(mut active) = deep_job.lock() {
                *active = Some(job);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let deep_job = Arc::clone(&deep_job);
        ui.on_cancel_deep_analysis_requested(move || {
            if let Ok(active) = deep_job.lock() {
                if let Some(job) = active.as_ref() {
                    job.cancellation.cancel();
                }
            }
            if let Some(handle) = weak.upgrade() {
                handle.set_worker_status_text(
                    "Abbruch angefordert; DSP stoppt am nächsten Frame.".into(),
                );
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        let playback = Arc::clone(&playback);
        let active_audio_source_id = Arc::clone(&active_audio_source_id);
        let playback_clock_started = Arc::clone(&playback_clock_started);
        let shutdown = Arc::clone(&shutdown);
        {
            let weak = ui.as_weak();
            let controller = controller.clone();
            let stem_job = Arc::clone(&stem_job);
            let pending_stem_download = Arc::clone(&pending_stem_download);
            ui.on_run_stem_separation_requested(move || {
                let Some(handle) = weak.upgrade() else { return };
                if handle.get_busy() || handle.get_stem_running() || handle.get_playback_running() {
                    return;
                }
                let snapshot = match controller.snapshot() {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        handle.set_stem_status_text(error.into());
                        return;
                    }
                };
                let context = match stem_workflow::prepare_context(&snapshot) {
                    Ok(context) => context,
                    Err(error) => {
                        handle.set_stem_status_text(error.clone().into());
                        handle.set_status_text(error.into());
                        return;
                    }
                };
                handle.set_busy(true);
                handle.set_stem_running(true);
                handle.set_stem_status_text("Prüfe lokale Modelldateien ohne Download …".into());
                let job = stem_workflow::start_job(
                    controller.clone(),
                    context,
                    false,
                    Arc::clone(&pending_stem_download),
                );
                if let Ok(mut active) = stem_job.lock() {
                    *active = Some(job);
                }
            });
        }
        {
            let weak = ui.as_weak();
            let controller = controller.clone();
            let stem_job = Arc::clone(&stem_job);
            let pending_stem_download = Arc::clone(&pending_stem_download);
            ui.on_confirm_stem_model_download_requested(move || {
                let Some(handle) = weak.upgrade() else { return };
                let context = pending_stem_download
                    .lock()
                    .ok()
                    .and_then(|mut pending| pending.take());
                let Some(context) = context else {
                    handle.set_stem_status_text(
                        "Downloadfreigabe ist abgelaufen; bitte Trennung erneut starten.".into(),
                    );
                    return;
                };
                handle.set_busy(true);
                handle.set_stem_running(true);
                handle.set_stem_status_text(
                    "Modellabruf ausdrücklich freigegeben; lokale Stem-Trennung läuft …".into(),
                );
                let job = stem_workflow::start_job(
                    controller.clone(),
                    context,
                    true,
                    Arc::clone(&pending_stem_download),
                );
                if let Ok(mut active) = stem_job.lock() {
                    *active = Some(job);
                }
            });
        }
        {
            let weak = ui.as_weak();
            let pending_stem_download = Arc::clone(&pending_stem_download);
            ui.on_decline_stem_model_download_requested(move || {
                if let Ok(mut pending) = pending_stem_download.lock() {
                    *pending = None;
                }
                if let Some(handle) = weak.upgrade() {
                    handle.set_stem_download_prompt_open(false);
                    handle.set_stem_status_text(
                        "Modell nicht heruntergeladen; WAV und DSP-Ergebnis bleiben erhalten, bisherige Stem-Spuren bleiben unverändert."
                            .into(),
                    );
                    handle.set_status_text(
                        "Modell-Download abgelehnt; Modellbestand und vorhandene Stem-Spuren wurden nicht geändert."
                            .into(),
                    );
                }
            });
        }
        {
            let weak = ui.as_weak();
            let stem_job = Arc::clone(&stem_job);
            ui.on_cancel_stem_separation_requested(move || {
                if let Ok(active) = stem_job.lock() {
                    if let Some(job) = active.as_ref() {
                        job.cancellation.cancel();
                    }
                }
                if let Some(handle) = weak.upgrade() {
                    handle.set_stem_status_text(
                        "Abbruch angefordert; lokaler Separator-Prozess wird beendet …".into(),
                    );
                }
            });
        }
        {
            let weak = ui.as_weak();
            let controller = controller.clone();
            let playback = Arc::clone(&playback);
            let active_audio_source_id = Arc::clone(&active_audio_source_id);
            ui.on_select_audio_source_requested(move |source_id| {
                let Some(handle) = weak.upgrade() else { return };
                let source_id = source_id.to_string();
                let project = match controller.snapshot() {
                    Ok(project) => project,
                    Err(error) => {
                        handle.set_status_text(error.into());
                        return;
                    }
                };
                let available = if source_id == "source" {
                    project.audio_path.is_some()
                } else {
                    project
                        .stem_tracks
                        .iter()
                        .any(|track| format!("stem-{}", stem_kind_key(track.kind)) == source_id)
                };
                if !available {
                    handle.set_status_text(
                        "Diese lokale Audiospur ist in diesem Projekt nicht vorhanden.".into(),
                    );
                    return;
                }
                if let Ok(active) = playback.lock() {
                    if let Some(session) = active.as_ref() {
                        if let Err(error) = session.select_source(&source_id) {
                            handle.set_status_text(error.into());
                            return;
                        }
                    }
                }
                if let Ok(mut active) = active_audio_source_id.lock() {
                    *active = source_id.clone();
                }
                handle.set_active_audio_source_id(source_id.clone().into());
                handle.set_active_audio_source_label(audio_source_label(&source_id).into());
                handle.set_worker_status_text(
                    format!(
                        "Solo-Ausgabe: {} · gemeinsamer Playhead",
                        audio_source_label(&source_id)
                    )
                    .into(),
                );
            });
        }
        ui.on_toggle_playback_requested(move || {
            let Some(handle) = weak.upgrade() else { return };
            if handle.get_playback_running() && !handle.get_playback_paused() {
                controller.pause_playback();
                if let Ok(active) = playback.lock() {
                    if let Some(session) = active.as_ref() {
                        session.pause();
                    }
                }
                handle.set_playback_paused(true);
                handle.set_worker_status_text("Playback pausiert".into());
            } else if handle.get_playback_paused() {
                match controller.start_playback() {
                    Ok(()) => {
                        if let Ok(active) = playback.lock() {
                            if let Some(session) = active.as_ref() {
                                session.resume();
                            }
                        }
                        handle.set_playback_running(true);
                        handle.set_playback_paused(false);
                        handle.set_worker_status_text("Playback läuft".into());
                        ensure_playback_clock(
                            controller.clone(),
                            weak.clone(),
                            Arc::clone(&shutdown),
                            Arc::clone(&playback_clock_started),
                        );
                    }
                    Err(error) => handle.set_status_text(error.into()),
                }
            } else {
                let tempo = handle.get_tempo_percent();
                let loop_enabled = handle.get_loop_enabled();
                start_playback_async(
                    weak.clone(),
                    controller.clone(),
                    Arc::clone(&playback),
                    Arc::clone(&active_audio_source_id),
                    Arc::clone(&shutdown),
                    Arc::clone(&playback_clock_started),
                    tempo,
                    loop_enabled,
                );
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        let playback = Arc::clone(&playback);
        ui.on_stop_playback_requested(move || {
            controller.stop_playback();
            if let Ok(active) = playback.lock() {
                if let Some(session) = active.as_ref() {
                    session.stop();
                }
            }
            if let Some(handle) = weak.upgrade() {
                handle.set_playback_running(false);
                handle.set_playback_paused(false);
                handle.set_playback_position_text("00:00.0 / 00:00.0".into());
                handle.set_worker_status_text("Playback gestoppt".into());
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        let playback = Arc::clone(&playback);
        ui.on_loop_changed(move |enabled| {
            let range = if enabled {
                controller
                    .snapshot()
                    .ok()
                    .filter(|project| project.duration_seconds > 0.0)
                    .map(|project| (0.0, project.duration_seconds))
            } else {
                None
            };
            if let Err(error) = controller.set_playback_loop(range) {
                if let Some(handle) = weak.upgrade() {
                    handle.set_loop_enabled(!enabled);
                    handle.set_status_text(error.into());
                }
            } else if let Ok(active) = playback.lock() {
                if let Some(session) = active.as_ref() {
                    if let Err(error) = session.set_loop(range) {
                        if let Some(handle) = weak.upgrade() {
                            handle.set_status_text(error.into());
                        }
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        let playback = Arc::clone(&playback);
        ui.on_tempo_changed(move |percent| {
            if !(50..=150).contains(&percent) {
                if let Some(handle) = weak.upgrade() {
                    handle.set_status_text(
                        "Playbacktempo muss zwischen 50 und 150 Prozent liegen.".into(),
                    );
                }
                return;
            }
            if let Err(error) = controller.set_playback_tempo(percent as u16) {
                if let Some(handle) = weak.upgrade() {
                    handle.set_status_text(error.into());
                }
            } else if let Ok(active) = playback.lock() {
                if let Some(session) = active.as_ref() {
                    if let Err(error) = session.set_tempo_percent(percent as u16) {
                        if let Some(handle) = weak.upgrade() {
                            handle.set_status_text(error.into());
                        }
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_generate_tabs_requested(move || {
            match controller.generate_tabs(GuitarTuning::standard(), "Gitarre") {
                Ok(track) => {
                    if let Some(handle) = weak.upgrade() {
                        let project = controller.snapshot().ok();
                        let bpm = project.as_ref().map(|p| p.bpm).unwrap_or(120.0);
                        let meter = project
                            .as_ref()
                            .map(|p| p.time_signature)
                            .unwrap_or_default();
                        handle.set_tab_text(format_ascii_tab(&track, bpm, meter, 128).into());
                        handle.set_fretboard_text(
                            format!(
                                "{} · Capo {} · {} Tabnoten",
                                describe_tuning(&track.tuning),
                                track.tuning.capo,
                                track.notes.len()
                            )
                            .into(),
                        );
                        handle.set_status_text(
                            format!("{} Tabnoten erzeugt.", track.notes.len()).into(),
                        );
                        update_project_view(&handle, &controller);
                    }
                }
                Err(error) => {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_status_text(error.into());
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        let models = Arc::clone(&models);
        ui.on_start_lyrics_requested(move || {
            let model_directory = models.lock().ok().and_then(|catalog| catalog.as_ref().and_then(|snapshot| snapshot.entries.iter().find(|entry| entry.role == ModelRole::SpeechRecognition && entry.status == ModelStatus::Validated && entry.path.is_dir()).map(|entry| entry.path.clone())));
            let Some(model_directory) = model_directory else {
                if let Some(handle) = weak.upgrade() { handle.set_status_text("Kein strukturell gültiges lokales faster-whisper-Modell gefunden; Modellscan ausführen und ein lokales Modell bereitstellen.".into()); }
                return;
            };
            let worker_script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../livetab-core/workers/lyrics_worker.py");
            let python = std::env::var_os("LIVETAB_PYTHON").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("python"));
            let backend = PythonWorkerLyricsBackend { python_executable: python, worker_script, model_directory };
            if let Some(handle) = weak.upgrade() { handle.set_worker_status_text("Lokale faster-whisper-Transkription läuft …".into()); }
            let manager = controller.job_manager();
            let job_controller = controller.clone();
            manager.start(JobKind::Lyrics, move |reporter, cancellation| {
                cancellation.check()?;
                reporter.progress(0.05, "Lokales ASR wird gestartet");
                let result = job_controller.transcribe_lyrics(&backend, None)?;
                cancellation.check()?;
                reporter.progress(0.95, "Wortzeiten werden am Beatgrid ausgerichtet");
                Ok(format!("{} Wörter in {} Lyrics-Zeilen", result.words.len(), result.lines.len()))
            });
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_save_project_requested(move || {
            let Some(handle) = weak.upgrade() else { return };
            let typed = handle.get_project_path_text().to_string();
            let path = if typed.trim().is_empty() {
                choose_project_save_path(&controller)
            } else {
                Some(PathBuf::from(typed))
            };
            let Some(path) = path else {
                handle.set_project_status_text(
                    "Projektspeicherung abgebrochen oder Dialog nicht verfügbar.".into(),
                );
                return;
            };
            start_project_save(weak.clone(), controller.clone(), path);
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_open_project_requested(move || {
            let Some(handle) = weak.upgrade() else { return };
            let typed = handle.get_project_path_text().to_string();
            let path = if typed.trim().is_empty() {
                choose_project_open_path()
            } else {
                Some(PathBuf::from(typed))
            };
            let Some(path) = path else {
                handle.set_project_status_text(
                    "Projektdateiauswahl abgebrochen oder Dialog nicht verfügbar.".into(),
                );
                return;
            };
            handle.set_project_busy(true);
            handle
                .set_project_status_text("Projektarchiv wird geprüft und lokal geöffnet …".into());
            let weak = weak.clone();
            let controller = controller.clone();
            thread::spawn(move || {
                let result = application_data_directory().and_then(|root| {
                    let unique = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let project =
                        load_dbztab(&path, root.join("opened").join(format!("project-{unique}")))?;
                    controller.replace_project(project.clone())?;
                    Ok::<SongProject, String>(project)
                });
                let _ = weak.upgrade_in_event_loop(move |handle| {
                    handle.set_project_busy(false);
                    match result {
                        Ok(project) => {
                            handle
                                .set_project_path_text(path.to_string_lossy().into_owned().into());
                            handle.set_project_status_text(
                                format!("Projekt geöffnet: {} · {}", project.title, path.display())
                                    .into(),
                            );
                            update_project_view(&handle, &controller);
                        }
                        Err(error) => handle.set_project_status_text(
                            format!("Projekt konnte nicht geöffnet werden: {error}").into(),
                        ),
                    }
                });
            });
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_export_requested(move |format| {
            let Some(handle) = weak.upgrade() else { return };
            let format = format.to_string();
            let typed = handle.get_export_path_text().to_string();
            let path = if typed.trim().is_empty() {
                choose_export_path(&controller, &format)
            } else {
                Some(PathBuf::from(typed))
            };
            let Some(path) = path else {
                handle.set_status_text(
                    "Exportdateiauswahl abgebrochen oder Dialog nicht verfügbar.".into(),
                );
                return;
            };
            let controller = controller.clone();
            let manager = controller.job_manager();
            handle.set_worker_status_text(format!("Lokaler {format}-Export läuft …").into());
            let format_for_job = format.clone();
            manager.start(JobKind::Export, move |reporter, cancellation| {
                cancellation.check()?;
                reporter.progress(0.1, "Projektzustand wird validiert");
                let saved = controller.export(&format_for_job, &path)?;
                reporter.progress(1.0, "Datei lokal geschrieben");
                Ok(format!("Export gespeichert: {}", saved.display()))
            });
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_correct_chord_requested(move |time, chord| {
            match controller.correct_chord(f64::from(time), chord.as_str()) {
                Ok(corrected) => {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_chord_correction_text(corrected.chord().name().into());
                        handle.set_status_text(
                            format!("Akkordkorrektur gespeichert: {}", corrected.chord().name())
                                .into(),
                        );
                        update_project_view(&handle, &controller);
                    }
                }
                Err(error) => {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_status_text(error.into());
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_correct_downbeat_requested(move |offset| {
            let parsed = offset.as_str().trim().replace(',', ".").parse::<f64>();
            match parsed {
                Ok(seconds) => match controller.correct_downbeat(seconds) {
                    Ok(beats) => {
                        if let Some(handle) = weak.upgrade() {
                            if let Some(first) = beats.first() {
                                handle.set_downbeat_offset_text(
                                    format!("{:.3}", first.time_seconds).into(),
                                );
                            }
                            handle.set_status_text("Downbeat und Beatgrid korrigiert.".into());
                            update_project_view(&handle, &controller);
                        }
                    }
                    Err(error) => {
                        if let Some(handle) = weak.upgrade() {
                            handle.set_status_text(error.into());
                        }
                    }
                },
                Err(_) => {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_status_text(
                            "Downbeat-Offset muss eine Zahl in Sekunden sein.".into(),
                        );
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let controller = controller.clone();
        ui.on_edit_lyrics_requested(move |index, text| {
            if index < 0 {
                if let Some(handle) = weak.upgrade() {
                    handle.set_status_text("Lyrics-Zeilenindex darf nicht negativ sein.".into());
                }
                return;
            }
            match controller.edit_lyrics_line(index as usize, text.as_str()) {
                Ok(()) => {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_status_text(
                            "Lyrics-Zeile gespeichert; Wortzeiten wurden neu verteilt.".into(),
                        );
                        update_project_view(&handle, &controller);
                    }
                }
                Err(error) => {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_status_text(error.into());
                    }
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_select_path_requested(move |kind| match kind.as_str() {
            "project" => {
                if let Some(path) = choose_project_open_path() {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_project_path_text(path.to_string_lossy().into_owned().into());
                    }
                }
            }
            "export" => {
                if let Some(path) = choose_export_path_without_project("txt") {
                    if let Some(handle) = weak.upgrade() {
                        handle.set_export_path_text(path.to_string_lossy().into_owned().into());
                    }
                }
            }
            _ => {
                if let Some(handle) = weak.upgrade() {
                    handle.set_status_text("Unbekannter Dateidialogtyp.".into());
                }
            }
        });
    }

    AppLifecycle {
        shutdown,
        capture_requested,
        capture,
        playback,
        stem_job,
    }
}

/// Zweck: Dekodiert eine WAV-Datei auf einem Worker und aktualisiert das Projekt-/Waveform-UI.
/// Eingaben: Schwacher Fensterhandle, Controller und lokale Audiodatei.
/// Ausgabe: Keine; Status, Dauer und Waveform erscheinen nach Abschluss im UI-Thread.
fn open_audio_async(weak: Weak<MainWindow>, controller: AppController, path: PathBuf) {
    if let Some(handle) = weak.upgrade() {
        if handle.get_busy() {
            return;
        }
        handle.set_busy(true);
        handle.set_audio_status_text("WAV wird lokal dekodiert …".into());
    }
    thread::spawn(move || {
        let result = controller.open_audio(&path).map(|wave| {
            (
                wave.duration_seconds(),
                wave.format.sample_rate,
                wave.format.channels,
                livetab_core::dsp::waveform_bars(&wave.samples, 72),
            )
        });
        let _ = weak.upgrade_in_event_loop(move |handle| {
            handle.set_busy(false);
            match result {
                Ok((duration, rate, channels, waveform)) => {
                    handle.set_audio_status_text(
                        format!(
                            "{} · {:.2}s · {} Hz · {} Kanal/Kanäle",
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("Audio"),
                            duration,
                            rate,
                            channels
                        )
                        .into(),
                    );
                    handle.set_waveform_text(waveform.into());
                    handle.set_worker_status_text(
                        "Audioquelle bereit · Deep-Analyse kann separat gestartet werden".into(),
                    );
                    handle.set_status_text("Lokale WAV-Datei geöffnet.".into());
                    update_project_view(&handle, &controller);
                }
                Err(error) => {
                    handle.set_audio_status_text(error.clone().into());
                    handle.set_status_text(error.into());
                }
            }
        });
    });
}

/// Ergebnis der lokalen Vollanalyse einer zuvor gespeicherten Liveaufnahme.
#[derive(Debug)]
struct RecordingAnalysisSummary {
    sample_rate: u32,
    duration_seconds: f64,
    chord_count: usize,
    note_count: usize,
    analyzed_frames: usize,
}

/// Zweck: Speichert erfasste Live-Audiosamples beliebiger lokaler Quellen als WAV und startet die vollständige Rust-DSP-Analyse.
/// Eingaben: Controller, WAV-Ziel, besessene Mono-Samples, Sample-Rate, kooperative Abfrage und Callback nach erfolgreichem Schreiben.
/// Ausgabe: Analysekennzahlen oder ein Schreib-, Kurzsignal-, Datei- oder Abbruchfehler; gespeicherte WAV bleibt bei Analysefehlern erhalten.
fn save_and_analyze_recording<F, S>(
    controller: &AppController,
    path: &std::path::Path,
    samples: Vec<f32>,
    sample_rate: u32,
    mut is_cancelled: F,
    after_save: S,
) -> Result<RecordingAnalysisSummary, String>
where
    F: FnMut() -> bool,
    S: FnOnce(String, f64, u32),
{
    if sample_rate == 0 || samples.is_empty() {
        return Err("Die Liveaufnahme enthält keine gültigen Samples.".into());
    }
    if is_cancelled() {
        return Err("Aufnahme-Analyse wurde vor dem Speichern abgebrochen.".into());
    }
    write_wav(path, &samples, sample_rate, 1).map_err(|error| error.to_string())?;
    let duration_seconds = samples.len() as f64 / f64::from(sample_rate);
    let waveform = livetab_core::dsp::waveform_bars(&samples, 72);
    after_save(waveform, duration_seconds, sample_rate);
    drop(samples);
    let result = controller.analyze_audio_file_with_cancel(path, is_cancelled)?;
    Ok(RecordingAnalysisSummary {
        sample_rate,
        duration_seconds: result.duration_seconds,
        chord_count: result.chords.len(),
        note_count: result.notes.len(),
        analyzed_frames: result.analyzed_frames,
    })
}

/// Zweck: Öffnet eine lokale WAV-Quelle und startet die synchrone native Audio-/Schedulerwiedergabe.
/// Eingaben: Fenster, Controller, geteiltes Ausgabehandle, Shutdown-/Clockflags, Tempo und Loopzustand.
/// Ausgabe: Keine; Gerätefehler und Startstatus werden asynchron an die UI zurückgemeldet.
fn start_playback_async(
    weak: Weak<MainWindow>,
    controller: AppController,
    playback: Arc<Mutex<Option<PlaybackOutputSession>>>,
    active_audio_source_id: Arc<Mutex<String>>,
    shutdown: Arc<AtomicBool>,
    playback_clock_started: Arc<AtomicBool>,
    tempo_percent: i32,
    loop_enabled: bool,
) {
    let Some(handle) = weak.upgrade() else { return };
    if handle.get_busy() {
        return;
    }
    let project = match controller.snapshot() {
        Ok(project) => project,
        Err(error) => {
            handle.set_status_text(error.into());
            return;
        }
    };
    let Some(path) = project.audio_path.clone() else {
        handle.set_status_text(
            "Bitte zuerst eine lokale WAV-Datei öffnen; Playback benötigt eine Audioquelle.".into(),
        );
        return;
    };
    let mut source_paths = vec![("source".to_string(), path)];
    source_paths.extend(
        project
            .stem_tracks
            .iter()
            .map(|track| (track.id.clone(), track.file_path.clone())),
    );
    let mut selected_source = active_audio_source_id
        .lock()
        .map(|source| source.clone())
        .unwrap_or_else(|_| "source".into());
    if !source_paths.iter().any(|(id, _)| id == &selected_source) {
        selected_source = "source".into();
        if let Ok(mut current) = active_audio_source_id.lock() {
            *current = selected_source.clone();
        }
        handle.set_active_audio_source_id("source".into());
        handle.set_active_audio_source_label("Original".into());
    }
    handle.set_busy(true);
    handle.set_worker_status_text("Original und synchronisierte Stem-WAVs werden geladen …".into());
    let bpm = project.bpm;
    let meter_numerator = project.time_signature.numerator();
    let project_duration = project.duration_seconds;
    thread::spawn(move || {
        let result = (|| {
            if source_paths.len() > 1 {
                let total_bytes = source_paths.iter().try_fold(
                    0u64,
                    |total, (_, path)| -> Result<u64, String> {
                        let file_bytes = std::fs::metadata(path)
                            .map_err(|error| format!("{}: {error}", path.display()))?
                            .len();
                        total.checked_add(file_bytes).ok_or_else(|| {
                            "Gesamtgröße der Playback-WAVs ist nicht darstellbar.".into()
                        })
                    },
                )?;
                if total_bytes > MAX_SYNC_PLAYBACK_FILE_BYTES {
                    return Err(format!(
                        "Original und Stem-WAVs belegen zusammen mehr als 384 MiB; das synchronisierte Laden ist zum Schutz des Arbeitsspeichers begrenzt."
                    ));
                }
            }
            let sources = source_paths
                .into_iter()
                .map(|(id, path)| {
                    read_wav(&path)
                        .map(|wave| (id, wave))
                        .map_err(|error| format!("{}: {error}", path.display()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let duration = sources
                .first()
                .map(|(_, wave)| wave.duration_seconds())
                .unwrap_or_default();
            PlaybackOutputSession::start_sources(sources, &selected_source)
                .map(|session| (session, duration))
        })();
        let _ = weak.upgrade_in_event_loop(move |handle| {
            handle.set_busy(false);
            match result {
                Ok((session, audio_duration)) => {
                    let _ = session.set_tempo_percent(tempo_percent.clamp(50, 150) as u16);
                    session.configure_metronome(true, bpm, meter_numerator);
                    let loop_range = if loop_enabled {
                        Some((0.0, project_duration.min(audio_duration)))
                    } else {
                        None
                    };
                    if let Err(error) = session.set_loop(loop_range) {
                        handle.set_status_text(error.into());
                        return;
                    }
                    controller.stop_playback();
                    if let Err(error) = controller
                        .set_playback_loop(loop_range)
                        .and_then(|_| {
                            controller.set_playback_tempo(tempo_percent.clamp(50, 150) as u16)
                        })
                        .and_then(|_| controller.start_playback())
                    {
                        handle.set_status_text(error.into());
                        return;
                    }
                    session.resume();
                    match playback.lock() {
                        Ok(mut active) => *active = Some(session),
                        Err(_) => {
                            handle.set_status_text(
                                "Playbackhandle ist gesperrt; Audioausgabe wurde verworfen.".into(),
                            );
                            return;
                        }
                    }
                    handle.set_playback_running(true);
                    handle.set_playback_paused(false);
                    handle.set_worker_status_text("Lokale WAV-Wiedergabe läuft".into());
                    ensure_playback_clock(
                        controller.clone(),
                        handle.as_weak(),
                        Arc::clone(&shutdown),
                        Arc::clone(&playback_clock_started),
                    );
                }
                Err(error) => {
                    handle.set_playback_running(false);
                    handle.set_status_text(error.clone().into());
                    handle.set_worker_status_text(
                        format!("Playback nicht gestartet: {error}").into(),
                    );
                }
            }
        });
    });
}

/// Zweck: Startet den Playback-Ticker höchstens einmal und veröffentlicht Cursorereignisse in festen kurzen Intervallen.
/// Eingaben: Controller, Slint-Handle, App-Stoppsignal und Einmalstartflag.
/// Ausgabe: Keine; der Controller verwaltet Tempo/Loop, der UI-Thread empfängt Cursor- und Lyricsereignisse.
fn ensure_playback_clock(
    controller: AppController,
    weak: Weak<MainWindow>,
    shutdown: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
) {
    if started.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::Builder::new()
        .name("livetab-playback-clock".into())
        .spawn(move || {
            let mut last = Instant::now();
            while !shutdown.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(33));
                let now = Instant::now();
                let elapsed = now.duration_since(last).as_secs_f64();
                last = now;
                if controller.playback_tick(elapsed).is_err() {
                    continue;
                }
                if weak.upgrade().is_none() {
                    break;
                }
            }
        })
        .ok();
}

/// Zweck: Verteilt typsichere Audio-, Akkord-, Beat-, Playback- und Fehlerereignisse an Slint.
/// Eingaben: Slint-Weak-Handle, Controller für Snapshot/Observer und App-Stoppsignal.
/// Ausgabe: Keine; Worker verlassen den Loop beim Appende ohne starke UI-Referenz.
fn start_event_dispatcher(
    weak: Weak<MainWindow>,
    controller: AppController,
    shutdown: Arc<AtomicBool>,
    capture_requested: Arc<AtomicBool>,
    capture: Arc<Mutex<Option<CaptureSession>>>,
) {
    let receiver = controller.event_bus().subscribe();
    thread::Builder::new()
        .name("livetab-ui-events".into())
        .spawn(move || {
            while !shutdown.load(Ordering::Acquire) {
                let event = match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => event,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let controller = controller.clone();
                let capture_requested = capture_requested.clone();
                let capture = capture.clone();
                let _ = weak.clone().upgrade_in_event_loop(move |handle| {
                    match event {
                        AppEvent::AudioCaptureChanged { running, source } => {
                            controller.observer().set_audio_source(source.clone());
                            handle.set_live_running(running);
                            handle.set_audio_status_text(source.into());
                            if !running {
                                capture_requested.store(false, Ordering::Release);
                                handle.set_recording(false);
                                if let Ok(mut active) = capture.lock() {
                                    if let Some(session) = active.take() {
                                        thread::spawn(move || session.stop());
                                    }
                                }
                            }
                        }
                        AppEvent::AudioLevel {
                            rms,
                            peak,
                            waveform,
                        } => {
                            handle.set_waveform_text(waveform.into());
                            handle.set_audio_status_text(
                                format!("RMS {:.3} · Peak {:.3}", rms, peak).into(),
                            );
                        }
                        AppEvent::ChordDetected(event) => {
                            handle.set_current_chord_text(event.chord().name().into());
                            handle.set_chord_correction_text(event.chord().name().into());
                            handle.set_correction_time_seconds(event.start_seconds() as f32);
                            handle.set_confidence_text(
                                format!(
                                    "Confidence {:.0}% · {:.2}s",
                                    event.confidence() * 100.0,
                                    event.start_seconds()
                                )
                                .into(),
                            );
                            update_project_view(&handle, &controller);
                        }
                        AppEvent::BeatGridUpdated {
                            bpm,
                            beats,
                            confidence,
                        } => {
                            handle.set_bpm_text(format!("{bpm:.1} BPM").into());
                            handle.set_beatgrid_text(
                                format!(
                                    "{} Beats · {:.0}% Konfidenz",
                                    beats.len(),
                                    confidence * 100.0
                                )
                                .into(),
                            );
                            update_project_view(&handle, &controller);
                        }
                        AppEvent::JobProgress {
                            kind,
                            progress,
                            message,
                            ..
                        } => {
                            handle.set_worker_status_text(
                                format!("{kind}: {:.0}% · {message}", progress * 100.0).into(),
                            );
                        }
                        AppEvent::JobCompleted { message, .. } => {
                            handle.set_worker_status_text(message.into());
                            update_project_view(&handle, &controller);
                        }
                        AppEvent::Error { category, message } => {
                            handle.set_worker_status_text(format!("{category}: {message}").into());
                            handle.set_status_text(message.into());
                        }
                        AppEvent::PlaybackPosition {
                            seconds,
                            bar,
                            beat,
                            lyric,
                        } => {
                            let duration = controller
                                .snapshot()
                                .map(|project| project.duration_seconds)
                                .unwrap_or_default();
                            handle.set_playback_position_text(
                                format!("{} / {}", format_clock(seconds), format_clock(duration))
                                    .into(),
                            );
                            handle.set_selection_status_text(
                                match lyric {
                                    Some(word) => format!("Takt {bar} · Beat {beat:.2} · {word}"),
                                    None => format!("Takt {bar} · Beat {beat:.2}"),
                                }
                                .into(),
                            );
                            if duration > 0.0 && seconds >= duration && !handle.get_loop_enabled() {
                                handle.set_playback_running(false);
                                handle.set_playback_paused(false);
                                handle.set_worker_status_text("Playback abgeschlossen".into());
                                controller.stop_playback();
                            }
                        }
                    }
                    handle.set_observer_text(controller.observer().panel_text().into());
                });
            }
        })
        .ok();
}

/// Zweck: Verarbeitet den JobManager-Eventkanal und hält Fortschritt, Ergebnis und Abbruch sichtbar.
/// Eingaben: Weak-Handle, Controller, Shutdown-Flag und zugehöriges Deep-Analyse-Jobhandle.
/// Ausgabe: Keine; beendet den Dispatcher mit der App und räumt terminale Handles auf.
fn start_job_dispatcher(
    weak: Weak<MainWindow>,
    controller: AppController,
    shutdown: Arc<AtomicBool>,
    deep_job: Arc<Mutex<Option<JobHandle>>>,
    stem_job: Arc<Mutex<Option<JobHandle>>>,
    pending_stem_download: Arc<Mutex<Option<StemRunContext>>>,
    recording_wav_saved: Arc<AtomicBool>,
) {
    let Some(receiver) = controller.job_manager().take_event_receiver() else {
        return;
    };
    thread::Builder::new()
        .name("livetab-job-events".into())
        .spawn(move || {
            while !shutdown.load(Ordering::Acquire) {
                let event = match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => event,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let weak = weak.clone();
                let deep_job = Arc::clone(&deep_job);
                let stem_job = Arc::clone(&stem_job);
                let pending_stem_download = Arc::clone(&pending_stem_download);
                let recording_wav_saved = Arc::clone(&recording_wav_saved);
                let shutdown = Arc::clone(&shutdown);
                let controller = controller.clone();
                let _ = weak.clone().upgrade_in_event_loop(move |handle| {
                    let (status, terminal, kind, progress, message) = match event {
                        JobEvent::Started(status) | JobEvent::Progress(status) => (
                            status.state.clone(),
                            false,
                            status.kind,
                            status.progress,
                            status.message,
                        ),
                        JobEvent::Completed(status) => (
                            status.state.clone(),
                            true,
                            status.kind,
                            status.progress,
                            status.message,
                        ),
                        JobEvent::Failed(status) => (
                            status.state.clone(),
                            true,
                            status.kind,
                            status.progress,
                            status.message,
                        ),
                        JobEvent::Cancelled(status) => (
                            status.state.clone(),
                            true,
                            status.kind,
                            status.progress,
                            status.message,
                        ),
                    };
                    let recording_followup = recording_followup_after_terminal(
                        kind,
                        terminal,
                        &status,
                        &recording_wav_saved,
                    );
                    if kind == JobKind::DeepAnalysis {
                        handle.set_deep_running(!terminal);
                        handle.set_busy(!terminal);
                    }
                    if kind == JobKind::StemSeparation {
                        handle.set_stem_running(!terminal);
                    }
                    handle.set_worker_status_text(
                        format!("{:?} · {:.0}% · {message}", status, progress * 100.0).into(),
                    );
                    if terminal {
                        if let Ok(mut active) = deep_job.lock() {
                            if kind == JobKind::DeepAnalysis {
                                *active = None;
                            }
                        }
                        if kind == JobKind::StemSeparation {
                            if let Ok(mut active) = stem_job.lock() {
                                *active = None;
                            }
                            handle.set_busy(false);
                            if status == JobState::Succeeded && message == "STEM_DOWNLOAD_REQUIRED" {
                                let prompt = pending_stem_download
                                    .lock()
                                    .ok()
                                    .and_then(|pending| pending.as_ref().and_then(|context| context.download_prompt.clone()));
                                if let Some(prompt) = prompt {
                                    handle.set_stem_download_prompt_text(prompt.into());
                                    handle.set_stem_download_prompt_open(true);
                                    handle.set_stem_status_text("Modell fehlt; ohne Ihre Bestätigung wird nichts heruntergeladen.".into());
                                    handle.set_status_text("Für diese lokale Trennung fehlen Modelldateien. Bitte bestätigen oder ablehnen.".into());
                                } else {
                                    handle.set_stem_status_text("Worker meldete fehlende Modellressourcen, aber kein Freigabedialog war verfügbar.".into());
                                }
                            } else if status == JobState::Succeeded {
                                handle.set_stem_status_text(message.clone().into());
                            } else {
                                handle.set_stem_status_text(format!("Stem-Job {status:?}: {message}").into());
                            }
                        }
                        if status == JobState::Succeeded && !(kind == JobKind::StemSeparation && message == "STEM_DOWNLOAD_REQUIRED") {
                            handle.set_status_text(message.into());
                        } else {
                            if kind != JobKind::StemSeparation {
                                handle.set_status_text(format!("Job beendet: {message}").into());
                            }
                        }
                        match recording_followup {
                            RecordingFollowup::StartStems if cfg!(windows) => {
                                start_recording_stem_followup(
                                    &handle,
                                    controller.clone(),
                                    Arc::clone(&stem_job),
                                    Arc::clone(&pending_stem_download),
                                    Arc::clone(&shutdown),
                                );
                            }
                            RecordingFollowup::StartStems => {
                                handle.set_stem_status_text(
                                    "WAV gespeichert; automatische Vier-Stem-Trennung ist im Windows-Build verfügbar."
                                        .into(),
                                );
                            }
                            RecordingFollowup::CancelledAfterSave => {
                                handle.set_stem_status_text(
                                    "DSP-Auftrag abgebrochen; die gespeicherte WAV bleibt erhalten, Vier-Stem-Trennung nicht gestartet."
                                        .into(),
                                );
                            }
                            RecordingFollowup::None => {}
                        }
                        update_project_view(&handle, &controller);
                    }
                });
            }
        })
        .ok();
}

/// Zweck: Entscheidet einmalig, ob eine gespeicherte Live-WAV nach Abschluss der DSP-Aufnahmeverarbeitung den Stem-Folgejob auslöst.
/// Eingaben: Jobtyp, Terminalstatus, Ergebniszustand und atomarer Merker für eine erfolgreich geschriebene WAV.
/// Ausgabe: Folgejobstart, bewusster Nichtstart nach Abbruch oder keine aufnahmebezogene Aktion.
fn recording_followup_after_terminal(
    kind: JobKind,
    terminal: bool,
    status: &JobState,
    recording_wav_saved: &AtomicBool,
) -> RecordingFollowup {
    if kind != JobKind::DeepAnalysis || !terminal {
        return RecordingFollowup::None;
    }
    if !recording_wav_saved.swap(false, Ordering::AcqRel) {
        return RecordingFollowup::None;
    }
    match status {
        JobState::Succeeded | JobState::Failed => RecordingFollowup::StartStems,
        JobState::Cancelled => RecordingFollowup::CancelledAfterSave,
        JobState::Queued | JobState::Running => RecordingFollowup::None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RecordingFollowup {
    StartStems,
    CancelledAfterSave,
    None,
}

/// Zweck: Bereitet im Hintergrund den lokalen Vier-Stem-Folgejob vor und startet zunächst ohne Modell-Downloadfreigabe.
/// Eingaben: UI-Handle, AppController, aktiver Jobpuffer und auftragsgebundener Modellfreigabepuffer.
/// Ausgabe: Keine; der Job prüft lokale Ressourcen und kann nur über den vorhandenen Bestätigungsdialog Downloads erlauben.
fn start_recording_stem_followup(
    handle: &MainWindow,
    controller: AppController,
    stem_job: Arc<Mutex<Option<JobHandle>>>,
    pending_stem_download: Arc<Mutex<Option<StemRunContext>>>,
    shutdown: Arc<AtomicBool>,
) {
    handle.set_busy(true);
    handle.set_stem_running(true);
    handle.set_stem_status_text(
        "Live-WAV gespeichert; prüfe lokale Modellressourcen ohne Downloadfreigabe …".into(),
    );
    let weak = handle.as_weak();
    let worker = thread::Builder::new()
        .name("livetab-recording-stem-setup".into())
        .spawn(move || {
            if shutdown.load(Ordering::Acquire) {
                return;
            }
            let context = controller
                .snapshot()
                .and_then(|project| stem_workflow::prepare_context(&project));
            match context {
                Ok(context) => {
                    if shutdown.load(Ordering::Acquire) {
                        return;
                    }
                    let job =
                        stem_workflow::start_job(controller, context, false, pending_stem_download);
                    if let Ok(mut active) = stem_job.lock() {
                        *active = Some(job);
                    } else {
                        job.cancellation.cancel();
                    }
                }
                Err(error) => {
                    let _ = weak.upgrade_in_event_loop(move |handle| {
                        handle.set_busy(false);
                        handle.set_stem_running(false);
                        handle.set_stem_status_text(
                            format!(
                                "Aufnahme gespeichert; Stem-Vorbereitung fehlgeschlagen: {error}"
                            )
                            .into(),
                        );
                    });
                }
            }
        });
    if let Err(error) = worker {
        handle.set_busy(false);
        handle.set_stem_running(false);
        handle.set_stem_status_text(
            format!("Aufnahme gespeichert; Stem-Vorbereitungsworker konnte nicht starten: {error}")
                .into(),
        );
    }
}

/// Zweck: Aktualisiert alle Analyseanzeigen aus einer konsistenten Songprojektkopie.
/// Eingaben: UI-Handle und Controller-Snapshotquelle.
/// Ausgabe: Keine; Timeline, Beats, Lyrics, Tabs, Skala und Trackübersicht werden dargestellt.
fn update_project_view(handle: &MainWindow, controller: &AppController) {
    let Ok(project) = controller.snapshot() else {
        return;
    };
    handle.set_bpm_text(format!("{:.1} BPM", project.bpm).into());
    handle.set_current_key_text(
        project
            .key_signature
            .map(|key| key.name())
            .unwrap_or_else(|| "Tonart offen".into())
            .into(),
    );
    let scales = project
        .key_signature
        .map(|key| format!("{} · Dur-/Mollskala", key.name()))
        .unwrap_or_else(|| "Skalenempfehlungen erscheinen nach der Tonart-Erkennung.".into());
    let alternatives = controller.key_alternatives();
    let scales = if alternatives.is_empty() {
        scales
    } else {
        format!("{scales}\nAlternativen: {}", alternatives.join(" · "))
    };
    handle.set_scales_text(scales.into());
    if let Some(first) = project.beats.first() {
        handle.set_downbeat_offset_text(format!("{:.3}", first.time_seconds).into());
    }
    if let Some(chord) = project.chords.last() {
        handle.set_current_chord_text(chord.chord().name().into());
        handle.set_confidence_text(
            format!(
                "Confidence {:.0}% · {:.2}s",
                chord.confidence() * 100.0,
                chord.start_seconds()
            )
            .into(),
        );
    }
    let timeline = if project.chords.is_empty() {
        "Noch keine Akkordereignisse.".to_string()
    } else {
        project
            .chords
            .iter()
            .take(80)
            .map(|event| {
                format!(
                    "{:>6.2}s  {:<8}  {:.0}%",
                    event.start_seconds(),
                    event.chord().name(),
                    event.confidence() * 100.0
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    handle.set_chord_timeline_text(timeline.into());
    handle.set_beatgrid_text(if project.beats.is_empty() {
        "Beatgrid noch nicht berechnet.".into()
    } else {
        format!("{} Beats · {:.1} BPM", project.beats.len(), project.bpm).into()
    });
    if let Some(track) = project.tab_tracks.last() {
        handle
            .set_tab_text(format_ascii_tab(track, project.bpm, project.time_signature, 128).into());
        handle.set_fretboard_text(
            format!(
                "{} · Capo {} · {} Tabnoten",
                describe_tuning(&track.tuning),
                track.tuning.capo,
                track.notes.len()
            )
            .into(),
        );
    }
    let lyrics = if project.lyrics_lines.is_empty() {
        "Noch keine Lyrics. Starte einen lokalen Lyrics-Job, sofern ein passendes Modell verfügbar ist.".to_string()
    } else {
        project
            .lyrics_lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                format!(
                    "{:>2}. [{:>6.2}s] {}",
                    index + 1,
                    line.start_seconds,
                    line.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    handle.set_lyrics_text(lyrics.into());
    if handle.get_lyrics_edit_text().is_empty() {
        if let Some(line) = project.lyrics_lines.first() {
            handle.set_lyrics_edit_text(line.text.clone().into());
        }
    }
    let original = if project.audio_path.is_some() {
        "Original  Quelle bereit"
    } else {
        "Original  keine Audioquelle"
    };
    let mut track_lines = vec![original.to_string()];
    for track in &project.stem_tracks {
        track_lines.push(format!(
            "{}  {:.2}s · {} Hz",
            track.name, track.duration_seconds, track.sample_rate
        ));
    }
    track_lines.extend([
        format!("Akkorde   {}", project.chords.len()),
        format!("Noten     {}", project.notes.len()),
        format!("Beats     {}", project.beats.len()),
        format!("Lyrics    {} Wörter", project.lyrics_words.len()),
        format!(
            "Gitarre   {} Tabnoten",
            project
                .tab_tracks
                .iter()
                .map(|track| track.notes.len())
                .sum::<usize>()
        ),
        "Metronom  bereit".into(),
    ]);
    handle.set_tracks_text(track_lines.join("\n").into());
    handle.set_stem_vocals_available(
        project
            .stem_tracks
            .iter()
            .any(|track| track.kind == StemKind::Vocals),
    );
    handle.set_stem_drums_available(
        project
            .stem_tracks
            .iter()
            .any(|track| track.kind == StemKind::Drums),
    );
    handle.set_stem_bass_available(
        project
            .stem_tracks
            .iter()
            .any(|track| track.kind == StemKind::Bass),
    );
    handle.set_stem_other_available(
        project
            .stem_tracks
            .iter()
            .any(|track| track.kind == StemKind::Other),
    );
    let active_source = handle.get_active_audio_source_id().to_string();
    if active_source != "source"
        && !project
            .stem_tracks
            .iter()
            .any(|track| format!("stem-{}", stem_kind_key(track.kind)) == active_source)
    {
        handle.set_active_audio_source_id("source".into());
        handle.set_active_audio_source_label("Original".into());
    }
    handle.set_project_status_text(
        format!(
            "{} · {} · Revision {}",
            project.title, project.artist, project.revision
        )
        .into(),
    );
    handle.set_observer_text(controller.observer().panel_text().into());
}

/// Zweck: Erzeugt die stabile ID-Komponente für eine Stem-Klasse.
/// Eingaben: `kind` – Kernklassifikation der Spur.
/// Ausgabe: Kleingeschriebener Quellen-ID-Schlüssel für UI und Playback.
fn stem_kind_key(kind: StemKind) -> &'static str {
    match kind {
        StemKind::Vocals => "vocals",
        StemKind::Drums => "drums",
        StemKind::Bass => "bass",
        StemKind::Other => "other",
    }
}

/// Zweck: Liefert die kurze deutsche Beschriftung einer ausgewählten Playbackquelle.
/// Eingaben: `source_id` – `source` oder eine stabile `stem-*`-Kennung.
/// Ausgabe: Menschenlesbarer Anzeigename; unbekannte IDs bleiben sichtbar.
fn audio_source_label(source_id: &str) -> &'static str {
    match source_id {
        "source" => "Original",
        "stem-vocals" => "Gesang",
        "stem-drums" => "Drums",
        "stem-bass" => "Bass",
        "stem-other" => "Übrige Instrumente",
        _ => "Unbekannte Quelle",
    }
}

/// Zweck: Startet einen validierten .dbztab-Projektexport außerhalb des UI-Threads.
/// Eingaben: Weak-Handle, Controller und geprüfter Zielpfad.
/// Ausgabe: Keine; JobManager meldet terminalen Status und bleibt abbrechbar.
fn start_project_save(weak: Weak<MainWindow>, controller: AppController, path: PathBuf) {
    if let Some(handle) = weak.upgrade() {
        if handle.get_project_busy() {
            return;
        }
        handle.set_project_busy(true);
        handle.set_project_path_text(path.to_string_lossy().into_owned().into());
        handle.set_project_status_text("Projekt wird lokal als .dbztab gespeichert …".into());
    }
    let manager = controller.job_manager();
    manager.start(JobKind::Export, move |reporter, cancellation| {
        cancellation.check()?;
        reporter.progress(0.1, "Projektmodell wird validiert");
        match controller.save_project(&path) {
            Ok(saved) => {
                reporter.progress(1.0, "Projektarchiv geschrieben");
                let message = format!("Projekt gespeichert: {}", saved.display());
                let message_for_ui = message.clone();
                let _ = weak.upgrade_in_event_loop(move |handle| {
                    handle.set_project_busy(false);
                    handle.set_project_status_text(message_for_ui.into());
                });
                Ok(message)
            }
            Err(error) => {
                let error_for_ui = error.clone();
                let _ = weak.upgrade_in_event_loop(move |handle| {
                    handle.set_project_busy(false);
                    handle.set_project_status_text(
                        format!("Speichern fehlgeschlagen: {error_for_ui}").into(),
                    );
                });
                Err(error)
            }
        }
    });
}

/// Zweck: Öffnet einen nativen lokalen Audiodateidialog oder meldet die fehlende Plattformintegration.
/// Eingaben: Keine.
/// Ausgabe: Ausgewählte WAV-Datei oder `None` bei Abbruch/nicht unterstützter Plattform.
fn choose_audio_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        return FileDialog::new()
            .set_title("Lokale WAV-Datei auswählen")
            .add_filter("WAV-Audio", &["wav"])
            .pick_file();
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Zweck: Öffnet den Dialog zum Speichern einer lokalen Mono-PCM-WAV-Aufnahme.
/// Eingaben: Keine; der Nutzer wählt den endgültigen lokalen Dateipfad.
/// Ausgabe: Zielpfad oder `None` bei Abbruch/nicht unterstützter Plattform.
fn choose_recording_save_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        return FileDialog::new()
            .set_title("Live-Aufnahme als WAV speichern")
            .set_file_name("Live-Aufnahme.wav")
            .add_filter("PCM-WAV", &["wav"])
            .save_file();
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Zweck: Öffnet den Dialog zum Speichern eines portablen .dbztab-Projekts.
/// Eingaben: Controller für den aktuellen Songtitel.
/// Ausgabe: Zielpfad oder `None` bei Abbruch/nicht unterstützter Plattform.
fn choose_project_save_path(controller: &AppController) -> Option<PathBuf> {
    let name = controller
        .snapshot()
        .ok()
        .map(|project| format!("{}.dbztab", safe_filename(&project.title)))
        .unwrap_or_else(|| "Songprojekt.dbztab".into());
    #[cfg(windows)]
    {
        return FileDialog::new()
            .set_title("LiveTab-Projekt speichern")
            .set_file_name(name)
            .add_filter("LiveTab-Projekt", &["dbztab"])
            .save_file();
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        None
    }
}

/// Zweck: Öffnet den Dialog zum Lesen eines bestehenden portablen .dbztab-Projekts.
/// Eingaben: Keine.
/// Ausgabe: Lokaler Pfad oder `None` bei Abbruch/nicht unterstützter Plattform.
fn choose_project_open_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        return FileDialog::new()
            .set_title("LiveTab-Projekt öffnen")
            .add_filter("LiveTab-Projekt", &["dbztab"])
            .pick_file();
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Zweck: Öffnet den Exportdialog und schlägt einen songbezogenen lokalen Dateinamen vor.
/// Eingaben: Controller für Songtitel und unterstütztes Exportformat.
/// Ausgabe: Exportziel oder `None` bei Abbruch/nicht unterstützter Plattform.
fn choose_export_path(controller: &AppController, format: &str) -> Option<PathBuf> {
    let title = controller
        .snapshot()
        .ok()
        .map(|project| safe_filename(&project.title))
        .unwrap_or_else(|| "Song".into());
    choose_export_path_for_name(&title, format)
}

/// Zweck: Öffnet den Exportdialog ohne einen Controller-Snapshot.
/// Eingaben: Unterstütztes Austauschformat.
/// Ausgabe: Exportziel oder `None`.
fn choose_export_path_without_project(format: &str) -> Option<PathBuf> {
    choose_export_path_for_name("Song", format)
}

/// Zweck: Wählt einen Dateinamen und Filter für ein konkretes Exportformat.
/// Eingaben: Sicherer Basename und gewünschte Endung.
/// Ausgabe: Lokaler Exportpfad oder `None`.
fn choose_export_path_for_name(name: &str, format: &str) -> Option<PathBuf> {
    let filename = format!("{name}.{}", format.trim_start_matches('.'));
    #[cfg(windows)]
    {
        return FileDialog::new()
            .set_title("Song lokal exportieren")
            .set_file_name(filename)
            .add_filter("Songexport", &[format.trim_start_matches('.')])
            .save_file();
    }
    #[cfg(not(windows))]
    {
        let _ = filename;
        None
    }
}

/// Zweck: Ersetzt für vorgeschlagene Dateinamen Windows-/Unix-Pfadtrennzeichen und Steuerzeichen.
/// Eingaben: Nutzerdefinierter Songtitel.
/// Ausgabe: Nichtleerer Dateiname ohne Pfadkomponenten.
fn safe_filename(value: &str) -> String {
    let filtered = value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
        .collect::<String>();
    let trimmed = filtered.trim().trim_matches('.');
    if trimmed.is_empty() {
        "Song".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// Zweck: Formatiert eine endliche nichtnegative Playbackposition als `MM:SS.t`.
/// Eingaben: Songzeit in Sekunden.
/// Ausgabe: Kurzzeitzeichenkette für Cursor und Gesamtdauer.
fn format_clock(seconds: f64) -> String {
    let safe = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let minutes = (safe / 60.0).floor() as u64;
    let remainder = safe - minutes as f64 * 60.0;
    format!("{minutes:02}:{remainder:04.1}")
}

#[cfg(test)]
mod recording_workflow_tests {
    use super::*;

    /// Zweck: Startet Stem-Verarbeitung genau einmal bei terminalem Analyseabschluss einer erfolgreich gespeicherten Aufnahme.
    /// Eingaben: Keine; prüft fremde/zwischenzeitliche Jobs sowie einen fehlgeschlagenen DSP-Pass.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Trigger, Merkerverbrauch und Wiederholschutz.
    #[test]
    fn recording_stem_followup_is_once_per_saved_terminal_analysis() {
        let saved = AtomicBool::new(true);
        assert_eq!(
            recording_followup_after_terminal(
                JobKind::StemSeparation,
                true,
                &JobState::Succeeded,
                &saved,
            ),
            RecordingFollowup::None
        );
        assert!(saved.load(Ordering::Acquire));
        assert_eq!(
            recording_followup_after_terminal(
                JobKind::DeepAnalysis,
                false,
                &JobState::Running,
                &saved,
            ),
            RecordingFollowup::None
        );
        assert_eq!(
            recording_followup_after_terminal(
                JobKind::DeepAnalysis,
                true,
                &JobState::Failed,
                &saved,
            ),
            RecordingFollowup::StartStems
        );
        assert!(!saved.load(Ordering::Acquire));
        assert_eq!(
            recording_followup_after_terminal(
                JobKind::DeepAnalysis,
                true,
                &JobState::Succeeded,
                &saved,
            ),
            RecordingFollowup::None
        );
    }

    /// Zweck: Unterdrückt den automatischen Stem-Folgejob, wenn der Nutzer die Nachanalyse abbricht.
    /// Eingaben: Keine; kennzeichnet eine bereits gespeicherte WAV und beendet den zugehörigen DSP-Job als abgebrochen.
    /// Ausgabe: Keine Rückgabe; der Abbruch wird gemeldet und der Einmalmerker zurückgesetzt.
    #[test]
    fn recording_stem_followup_respects_cancel_after_wav_save() {
        let saved = AtomicBool::new(true);
        assert_eq!(
            recording_followup_after_terminal(
                JobKind::DeepAnalysis,
                true,
                &JobState::Cancelled,
                &saved,
            ),
            RecordingFollowup::CancelledAfterSave
        );
        assert!(!saved.load(Ordering::Acquire));
    }

    /// Zweck: Prüft den echten Save-then-analyze-Pfad mit synthetischem Mono-Audio.
    /// Eingaben: Keine; erzeugt eine temporäre Ein-Sekunden-WAV und einen Controller.
    /// Ausgabe: Keine Rückgabe; bestätigt WAV, Analysekennzahlen, sichtbaren Nach-Speichern-Hook und Projektquelle.
    #[test]
    fn saved_live_recording_is_analyzed_and_attached_to_project() {
        let controller = AppController::new("Testaufnahme", "DBZS");
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "livetab-live-recording-{}-{unique}.wav",
            std::process::id()
        ));
        let sample_rate = 48_000;
        let samples = (0..sample_rate)
            .map(|index| {
                (std::f32::consts::TAU * 440.0 * index as f32 / sample_rate as f32).sin() * 0.3
            })
            .collect::<Vec<_>>();
        let mut saved_info = None;
        let summary = save_and_analyze_recording(
            &controller,
            &path,
            samples,
            sample_rate,
            || false,
            |waveform, duration, rate| saved_info = Some((waveform, duration, rate)),
        )
        .unwrap();

        assert!(path.is_file());
        assert!(summary.analyzed_frames > 0);
        assert_eq!(summary.sample_rate, sample_rate);
        assert!((summary.duration_seconds - 1.0).abs() < 0.01);
        let (waveform, saved_duration, saved_rate) = saved_info.unwrap();
        assert!(!waveform.is_empty());
        assert!((saved_duration - 1.0).abs() < 0.01);
        assert_eq!(saved_rate, sample_rate);
        let project = controller.snapshot().unwrap();
        assert_eq!(project.audio_path.as_deref(), Some(path.as_path()));
        assert!((project.duration_seconds - 1.0).abs() < 0.01);
        std::fs::remove_file(path).unwrap();
    }

    /// Zweck: Verhindert Dateierzeugung und UI-Erfolgsmeldung, wenn der Nutzer vor dem WAV-Schreiben abbricht.
    /// Eingaben: Keine; nutzt kurze Samples, ein temporäres Ziel und ein bereits gesetztes Abbruchsignal.
    /// Ausgabe: Keine Rückgabe; erwartet Abbruchfehler, kein WAV und keinen Nach-Speichern-Callback.
    #[test]
    fn cancelled_recording_does_not_create_a_wav() {
        let controller = AppController::new("Abbruchtest", "DBZS");
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "livetab-cancelled-recording-{}-{unique}.wav",
            std::process::id()
        ));
        let mut after_save_called = false;
        let result = save_and_analyze_recording(
            &controller,
            &path,
            vec![0.0; 4096],
            48_000,
            || true,
            |_, _, _| after_save_called = true,
        );

        assert!(result.unwrap_err().contains("abgebrochen"));
        assert!(!path.exists());
        assert!(!after_save_called);
    }
}
