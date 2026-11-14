/*
===============================================================================
Division By Zeros (DBZS) - Native Audio Capture
Modul: audio_capture
Zweck: Erfasst Windows-Systemausgabe über WASAPI-Loopback oder ASIO-Interface-
       eingänge sowie mobile Eingabe über native cpal-Mikrofonstreams; DSP bleibt getrennt.
Sprache: Rust
Hinweise:
- Keine Audio-Callbackfunktion führt DSP, UI-Aufrufe oder blockierende Kanalwrites aus.
- Windows trennt Shared-Mode-WASAPI-Systemmix von ASIO-Interfaceeingängen.
- Android/iOS verwenden eine lokale Eingabequelle und verlangen Betriebssystemfreigabe.
===============================================================================
*/
use livetab_core::audio::{AudioFrame, InterleavedAudioRingBuffer};
use livetab_core::observability::{LogCategory, LogLevel};
use livetab_core::{AppController, AppEvent};
use std::collections::BTreeMap;
use std::collections::VecDeque;
#[cfg(windows)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const LIVE_FRAME_SAMPLES: usize = 4096;
const LIVE_HOP_SAMPLES: usize = 2048;
const CAPTURE_CHANNEL_CAPACITY: usize = 8;
const MAX_RECORDING_SECONDS: u64 = 600;
const MAX_RECORDING_BYTES: usize = 256 * 1024 * 1024;

/// Gemeinsame Capturemetadaten samt vorallokiertem Transport zum unabhängigen Analyseworker.
#[derive(Clone)]
struct CaptureConfiguration {
    sample_rate: u32,
    channels: u16,
    transport: Arc<InterleavedAudioRingBuffer>,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
enum WindowsAudioDevice {
    Wasapi(String),
    Asio(String),
}

#[cfg(windows)]
static SELECTED_WINDOWS_AUDIO_DEVICE: OnceLock<Mutex<Option<WindowsAudioDevice>>> = OnceLock::new();

/// Zweck: Listet lokale Capture-Quellen, auf Windows getrennt markierte WASAPI-Systemmix- und ASIO-Interfacequellen.
/// Eingaben: Keine; Windows-Geräte-/Treiberabfragen laufen auf dem aufrufenden Scanworker.
/// Ausgabe: Auswählbare Quellnamen oder ein verständlicher Plattform-/Treiberfehler.
pub fn available_audio_devices() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        let mut names = Vec::new();
        let mut errors = Vec::new();
        match available_wasapi_render_devices() {
            Ok(devices) => names.extend(devices),
            Err(error) => errors.push(error),
        }
        match crate::asio_capture::available_driver_names() {
            Ok(drivers) => names.extend(drivers),
            Err(error) => errors.push(error),
        }
        if names.is_empty() {
            return Err(if errors.is_empty() {
                "Windows meldet weder aktive WASAPI-Ausgabeendpunkte noch registrierte ASIO-Treiber.".into()
            } else {
                errors.join(" · ")
            });
        }
        return Ok(names);
    }
    #[cfg(all(not(windows), any(target_os = "android", target_os = "ios")))]
    {
        return Ok(vec!["Standardmikrofon · lokale Eingabe".into()]);
    }
    #[cfg(all(not(windows), not(any(target_os = "android", target_os = "ios"))))]
    {
        Err("Für dieses Plattformprofil ist keine lokale Audioquelle integriert.".into())
    }
}

/// Zweck: Listet WASAPI-Renderendpunkte mit einem expliziten Hinweis auf ihre Systemmix-Loopback-Funktion.
/// Eingaben: Keine; initialisiert COM auf dem aufrufenden Scanworker.
/// Ausgabe: Mit `WASAPI ·` markierte Gerätenamen oder ein verständlicher Windows-Audiofehler.
#[cfg(windows)]
fn available_wasapi_render_devices() -> Result<Vec<String>, String> {
    use wasapi::{DeviceEnumerator, Direction, initialize_mta};
    initialize_mta()
        .ok()
        .map_err(|error| format!("WASAPI-COM konnte nicht initialisiert werden: {error}"))?;
    let enumerator = DeviceEnumerator::new()
        .map_err(|error| format!("WASAPI-Geräteauflistung fehlgeschlagen: {error}"))?;
    let collection = enumerator
        .get_device_collection(&Direction::Render)
        .map_err(|error| {
            format!("Windows-Audioendpunkte konnten nicht aufgelistet werden: {error}")
        })?;
    let mut names = Vec::new();
    if let Ok(default_device) = enumerator.get_default_device(&Direction::Render) {
        if let Ok(default_name) = default_device.get_friendlyname() {
            names.push(default_name);
        }
    }
    for index in 0..collection
        .get_nbr_devices()
        .map_err(|error| error.to_string())?
    {
        let device = collection
            .get_device_at_index(index)
            .map_err(|error| error.to_string())?;
        if let Ok(name) = device.get_friendlyname() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        return Err("Windows meldet keine aktiven Render-Endpunkte.".into());
    }
    Ok(names
        .into_iter()
        .map(|name| format!("WASAPI · {name}"))
        .collect())
}

/// Zweck: Speichert die gewählte WASAPI- oder ASIO-Quelle ohne einen aktiven Audio-Thread zu verändern.
/// Eingaben: Markierter Eintrag aus `available_audio_devices` oder `None` für den WASAPI-Standardendpunkt.
/// Ausgabe: `Ok(())` oder ein Hinweis auf eine veraltete ASIO-Treiberliste/fehlende Plattformauswahl.
pub fn select_audio_device(name: Option<String>) -> Result<(), String> {
    #[cfg(windows)]
    {
        let selection = match name {
            Some(label) if label.starts_with("ASIO · ") => {
                crate::asio_capture::validate_driver_label(&label)?;
                Some(WindowsAudioDevice::Asio(label))
            }
            Some(label) => Some(WindowsAudioDevice::Wasapi(
                label
                    .strip_prefix("WASAPI · ")
                    .unwrap_or(&label)
                    .to_string(),
            )),
            None => None,
        };
        let selected = SELECTED_WINDOWS_AUDIO_DEVICE.get_or_init(|| Mutex::new(None));
        *selected
            .lock()
            .map_err(|_| "Audiogeräteauswahl ist gesperrt.".to_string())? = selection;
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        if name.is_none() || name.as_deref() == Some("Standardmikrofon · lokale Eingabe") {
            return Ok(());
        }
        Err(
            "Eine explizite Audioendpunktauswahl ist in diesem Plattformprofil nicht verfügbar."
                .into(),
        )
    }
}

/// Ein laufender lokaler Capture- und Liveanalyse-Workflow.
pub struct CaptureSession {
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    recording: Arc<Mutex<Option<Vec<f32>>>>,
    recording_limit_reached: Arc<AtomicBool>,
    sample_rate: u32,
    channels: u16,
    maximum_recording_samples: usize,
}

impl CaptureSession {
    /// Zweck: Beginnt eine neue begrenzte Aufnahme und erhält alle Quellkanäle für WAV-/MP3-Export.
    /// Eingaben: `self` – aktive Quelle; maximale Länge ist auf zehn Minuten begrenzt.
    /// Ausgabe: `Ok(())` oder ein verständlicher Fehler bei doppelter/leerer Session.
    pub fn begin_recording(&self) -> Result<(), String> {
        let mut recording = self
            .recording
            .lock()
            .map_err(|_| "Audio-Aufnahmepuffer ist gesperrt.".to_string())?;
        if recording.is_some() {
            return Err("Eine Audioaufnahme läuft bereits.".into());
        }
        let initial_capacity = (self.sample_rate as usize)
            .saturating_mul(usize::from(self.channels))
            .saturating_mul(5)
            .min(self.maximum_recording_samples);
        *recording = Some(Vec::with_capacity(initial_capacity));
        self.recording_limit_reached.store(false, Ordering::Release);
        Ok(())
    }

    /// Zweck: Beendet die laufende Pufferaufnahme und übergibt die Samples an den WAV-/MP3-Writer.
    /// Eingaben: `self` – aktive Capture-Session.
    /// Ausgabe: Interleaved Originalkanäle, Capture-Rate und Kanalzahl oder eine verständliche Meldung.
    pub fn finish_recording(&self) -> Result<(Vec<f32>, u32, u16), String> {
        let mut recording = self
            .recording
            .lock()
            .map_err(|_| "Audio-Aufnahmepuffer ist gesperrt.".to_string())?;
        let samples = recording
            .take()
            .ok_or_else(|| "Es läuft gerade keine Audioaufnahme.".to_string())?;
        if samples.is_empty() {
            return Err("Die Audioaufnahme enthält keine Samples.".into());
        }
        Ok((samples, self.sample_rate, self.channels))
    }

    /// Zweck: Liefert für die UI einen Snapshot der tatsächlich gepufferten Aufnahmesamples.
    /// Eingaben: `self` – aktive Capture-Session; die Obergrenze entspricht dem Zehn-Minuten-Limit.
    /// Ausgabe: Vollständige aufgenommene Frames, maximale Frames und Sample-Rate oder `None` ohne Aufnahme.
    pub fn recording_progress_snapshot(&self) -> Option<(usize, usize, u32)> {
        let recording = self.recording.lock().ok()?;
        let channels = usize::from(self.channels);
        Some((
            recording.as_ref()?.len() / channels,
            self.maximum_recording_samples / channels,
            self.sample_rate,
        ))
    }

    /// Zweck: Meldet, ob gegenwärtig Samples für einen WAV- oder MP3-Export gesammelt werden.
    /// Eingaben: `self` – aktive Capture-Session.
    /// Ausgabe: `true`, solange ein Aufnahmepuffer aktiv ist.
    pub fn is_recording(&self) -> bool {
        self.recording
            .lock()
            .map(|recording| recording.is_some())
            .unwrap_or(false)
    }

    /// Zweck: Signalisiert den kooperativen Stopp und wartet außerhalb der UI auf Capture-/Analyse-Worker.
    /// Eingaben: `self` – der zu beendende Aufnahmeworkflow.
    /// Ausgabe: Keine; alle Worker werden bis zu ihrem nächsten sicheren Stopppunkt beendet.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for CaptureSession {
    /// Zweck: Fordert beim Verwerfen einer Session sicher den Worker-Stopp an.
    /// Eingaben: `self` – die freizugebende Session.
    /// Ausgabe: Keine; Worker werden nicht im Slint-Thread gejoint.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Zweck: Startet die plattformnative Audioquelle und eine entkoppelte DSP-Frame-Pipeline.
/// Eingaben: Klonbarer `AppController`; Aufnahmefehler werden im EventBus veröffentlicht.
/// Ausgabe: Laufende `CaptureSession` oder verständliche Geräte-/Berechtigungs-/Workerfehlermeldung.
pub fn start_capture(controller: AppController) -> Result<CaptureSession, String> {
    controller.reset_live_analysis();
    let stop = Arc::new(AtomicBool::new(false));
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    let capture_stop = Arc::clone(&stop);
    let capture_events = controller.event_bus();
    let capture_observer = controller.observer();
    let startup_fallback = startup_sender.clone();
    let capture_worker = thread::Builder::new()
        .name("livetab-audio-capture".into())
        .spawn(move || {
            let worker_observer = capture_observer;
            let result = run_platform_capture(Arc::clone(&capture_stop), startup_sender, capture_events.clone());
            if let Err(message) = result {
                worker_observer.log(
                    LogLevel::Error,
                    LogCategory::Audio,
                    message.clone(),
                    "Lokale Audioaufnahme wurde beendet.",
                    BTreeMap::new(),
                    None,
                );
                let _ = startup_fallback.try_send(Err(message.clone()));
                capture_events.publish(AppEvent::AudioCaptureChanged {
                    running: false,
                    source: "Audioquelle gestoppt".into(),
                });
                capture_events.publish(AppEvent::Error {
                    category: "Audioaufnahme".into(),
                    message,
                });
            }
        })
        .map_err(|error| format!("Capture-Worker konnte nicht gestartet werden: {error}"))?;

    let configuration = match startup_receiver.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(configuration)) => configuration,
        Ok(Err(message)) => {
            stop.store(true, Ordering::Release);
            let _ = capture_worker.join();
            return Err(message);
        }
        Err(error) => {
            stop.store(true, Ordering::Release);
            let _ = capture_worker.join();
            return Err(format!(
                "Audioquelle meldete sich nicht rechtzeitig: {error}"
            ));
        }
    };

    let recording = Arc::new(Mutex::new(None));
    let recording_limit_reached = Arc::new(AtomicBool::new(false));
    let samples_per_second = (configuration.sample_rate as usize)
        .saturating_mul(usize::from(configuration.channels));
    let maximum_recording_samples = samples_per_second
        .saturating_mul(MAX_RECORDING_SECONDS as usize)
        .min(MAX_RECORDING_BYTES / std::mem::size_of::<f32>());
    let analyzer_controller = controller.clone();
    let analyzer_events = controller.event_bus();
    let analyzer_stop = Arc::clone(&stop);
    let analyzer_configuration = configuration.clone();
    let analyzer_worker = match thread::Builder::new()
        .name("livetab-live-dsp".into())
        .spawn({
            let recording = Arc::clone(&recording);
            let limit_reached = Arc::clone(&recording_limit_reached);
            move || {
                run_live_analyzer(
                    analyzer_configuration,
                    analyzer_stop,
                    analyzer_controller,
                    analyzer_events,
                    recording,
                    maximum_recording_samples,
                    limit_reached,
                )
            }
        })
    {
        Ok(worker) => worker,
        Err(error) => {
            stop.store(true, Ordering::Release);
            let _ = capture_worker.join();
            return Err(format!("Live-DSP-Worker konnte nicht gestartet werden: {error}"));
        }
    };

    Ok(CaptureSession {
        stop,
        workers: vec![capture_worker, analyzer_worker],
        recording,
        recording_limit_reached,
        sample_rate: configuration.sample_rate,
        channels: configuration.channels,
        maximum_recording_samples,
    })
}

/// Zweck: Verarbeitet bounded Interleaved-Captureblöcke in getrennte Live-DSP- und Mehrkanal-Recorderpfade.
/// Eingaben: Captureformat/Ring, Stoppflag, Controller/Eventbus und begrenzter Recorder-Samplepuffer.
/// Ausgabe: Keine; publiziert DSP-/Transportmetriken und meldet echte Überläufe oder Framefehler.
fn run_live_analyzer(
    configuration: CaptureConfiguration,
    stop: Arc<AtomicBool>,
    controller: AppController,
    events: livetab_core::EventBus,
    recording: Arc<Mutex<Option<Vec<f32>>>>,
    maximum_recording_samples: usize,
    recording_limit_reached: Arc<AtomicBool>,
) {
    let mut buffered = VecDeque::with_capacity(LIVE_FRAME_SAMPLES * 4);
    let mut buffer_start_sample = 0u64;
    let mut expected_input_sample = None;
    let mut last_transport_report = std::time::Instant::now();
    let sample_rate = configuration.sample_rate;
    let channels = usize::from(configuration.channels);
    while !stop.load(Ordering::Acquire) {
        let block = configuration.transport.read_available(4_096);
        if block.samples.is_empty() {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        let frame_count = block.frame_count();
        if expected_input_sample.is_some_and(|expected| expected != block.first_frame_index) {
            buffered.clear();
            buffer_start_sample = block.first_frame_index;
        } else if buffered.is_empty() {
            buffer_start_sample = block.first_frame_index;
        }
        if let Ok(mut active) = recording.lock() {
            if let Some(output) = active.as_mut() {
                let remaining = maximum_recording_samples.saturating_sub(output.len());
                let accepted = (remaining / channels * channels).min(block.samples.len());
                output.extend(block.samples.iter().take(accepted).copied());
                if accepted < block.samples.len()
                    && !recording_limit_reached.swap(true, Ordering::AcqRel)
                {
                    controller.observer().log(
                        LogLevel::Warning,
                        LogCategory::Audio,
                        "Die begrenzte Liveaufnahme hat ihre Speicher-/Zeitgrenze erreicht.",
                        "Die bisherige Mehrkanalaufnahme kann gespeichert werden.",
                        BTreeMap::new(),
                        None,
                    );
                    events.publish(AppEvent::Error {
                        category: "Audioaufnahme".into(),
                        message: "Die Aufnahmegrenze wurde erreicht; die bisherige Aufnahme bleibt als WAV oder MP3 speicherbar.".into(),
                    });
                }
            }
        }
        buffered.extend(block.samples.chunks_exact(channels).map(|frame| {
            let (sum, count) = frame.iter().copied().fold((0.0_f32, 0_usize), |(sum, count), sample| {
                if sample.is_finite() { (sum + sample, count + 1) } else { (sum, count) }
            });
            if count == 0 {
                0.0
            } else {
                (sum / count as f32).clamp(-1.0, 1.0)
            }
        }));
        expected_input_sample = Some(block.first_frame_index.saturating_add(frame_count as u64));
        while buffered.len() >= LIVE_FRAME_SAMPLES {
            let frame_samples = buffered
                .iter()
                .take(LIVE_FRAME_SAMPLES)
                .copied()
                .collect::<Vec<_>>();
            let frame_start = buffer_start_sample;
            buffered.drain(..LIVE_HOP_SAMPLES);
            buffer_start_sample = buffer_start_sample.saturating_add(LIVE_HOP_SAMPLES as u64);
            match AudioFrame::new(sample_rate, frame_start, frame_samples).and_then(|frame| {
                controller
                    .process_live_frame(&frame)
                    .map(|_| frame)
                    .map_err(|message| livetab_core::audio::AudioError::InvalidFormat(message))
            }) {
                Ok(_) => {}
                Err(error) => {
                    let message = error.to_string();
                    controller.observer().log(
                        LogLevel::Error,
                        LogCategory::Audio,
                        message.clone(),
                        "Live-Audioframe konnte nicht analysiert werden.",
                        BTreeMap::new(),
                        None,
                    );
                    events.publish(AppEvent::Error {
                        category: "Live-DSP".into(),
                        message,
                    });
                }
            }
        }
        if last_transport_report.elapsed() >= Duration::from_millis(100) {
            events.publish(AppEvent::AudioTransportMetrics {
                sample_rate,
                channels: configuration.channels,
                queue_frames: configuration.transport.queued_frames(),
                dropped_frames: configuration.transport.dropped_frames(),
                input_frames_seen: configuration.transport.input_frames_seen(),
            });
            last_transport_report = std::time::Instant::now();
        }
    }
}

/// Zweck: Routet die aktive Windows-Quelle zum separaten ASIO-Eingang oder zum WASAPI-Systemmix.
/// Eingaben: Bounded Sample-Kanal, Stoppflag, Start-Handshake und EventBus; Quelle stammt aus der Geräteauswahl.
/// Ausgabe: `Ok(())` nach kontrolliertem Stopp oder ein verständlicher Treiber-/Capturefehler.
#[cfg(windows)]
fn run_platform_capture(
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<CaptureConfiguration, String>>,
    events: livetab_core::EventBus,
) -> Result<(), String> {
    let selected = SELECTED_WINDOWS_AUDIO_DEVICE
        .get()
        .and_then(|selected| selected.lock().ok().and_then(|device| device.clone()));
    match selected {
        Some(WindowsAudioDevice::Asio(label)) => {
            run_asio_capture_compat(stop, startup, events, &label)
        }
        Some(WindowsAudioDevice::Wasapi(name)) => {
            run_wasapi_capture(stop, startup, events, Some(name))
        }
        None => run_wasapi_capture(stop, startup, events, None),
    }
}

/// Zweck: Überführt den bestehenden ASIO-Mono-Kanalvertrag in den neuen vorallokierten Ringpuffer.
/// Eingaben: Stoppsignal, Capture-Handshake, Eventbus und validierter ASIO-Treibername.
/// Ausgabe: Kontrollierter Streamstopp oder ein ASIO-/Transportfehler; der Adapter `asio_capture.rs` bleibt unverändert.
#[cfg(windows)]
fn run_asio_capture_compat(
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<CaptureConfiguration, String>>,
    events: livetab_core::EventBus,
    driver_label: &str,
) -> Result<(), String> {
    let (sample_sender, sample_receiver) = mpsc::sync_channel(CAPTURE_CHANNEL_CAPACITY);
    let (asio_startup_sender, asio_startup_receiver) = mpsc::sync_channel(1);
    let asio_stop = Arc::clone(&stop);
    let asio_events = events.clone();
    let label = driver_label.to_string();
    let asio_worker = thread::Builder::new()
        .name("livetab-asio-compat".into())
        .spawn(move || {
            crate::asio_capture::run_capture(
                sample_sender,
                asio_stop,
                asio_startup_sender,
                asio_events,
                &label,
            )
        })
        .map_err(|error| format!("ASIO-Kompatibilitätsworker konnte nicht gestartet werden: {error}"))?;
    let sample_rate = match asio_startup_receiver.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(sample_rate)) => sample_rate,
        Ok(Err(message)) => {
            let _ = asio_worker.join();
            return Err(message);
        }
        Err(error) => {
            stop.store(true, Ordering::Release);
            let worker_result = asio_worker.join();
            let worker_detail = match worker_result {
                Ok(Ok(())) => String::new(),
                Ok(Err(message)) => format!("; ASIO-Worker-Abschluss: {message}"),
                Err(_) => "; ASIO-Worker ist beim Abbruch unerwartet beendet worden.".into(),
            };
            return Err(format!("ASIO-Quelle meldete sich nicht rechtzeitig: {error}{worker_detail}"));
        }
    };
    let transport = match InterleavedAudioRingBuffer::new(sample_rate as usize * 2, 1) {
        Ok(transport) => Arc::new(transport),
        Err(error) => {
            stop.store(true, Ordering::Release);
            let _ = asio_worker.join();
            return Err(format!("ASIO-Capturepuffer konnte nicht angelegt werden: {error}"));
        }
    };
    let configuration = CaptureConfiguration {
        sample_rate,
        channels: 1,
        transport,
    };
    if startup.send(Ok(configuration.clone())).is_err() {
        stop.store(true, Ordering::Release);
        let _ = asio_worker.join();
        return Err("Capture-Start wurde vom Aufrufer abgebrochen.".into());
    }

    let mut transport_result = Ok(());
    while !stop.load(Ordering::Acquire) {
        match sample_receiver.recv_timeout(Duration::from_millis(8)) {
            Ok(samples) => {
                // Ein voller Ring verwirft das Paket und erhöht seine Drop-Metrik; der ASIO-Stream bleibt nichtblockierend.
                if let Err(error) = configuration.transport.push_f32_interleaved(&samples) {
                    transport_result = Err(format!("ASIO-Captureframe konnte nicht übertragen werden: {error}"));
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) if stop.load(Ordering::Acquire) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                transport_result = Err("ASIO-Audiokanal wurde unerwartet geschlossen.".into());
                break;
            }
        }
    }
    stop.store(true, Ordering::Release);
    let asio_result = asio_worker
        .join()
        .map_err(|_| "ASIO-Captureworker wurde unerwartet beendet.".to_string())?;
    transport_result?;
    asio_result
}

/// Zweck: Verarbeitet ausschließlich einen WASAPI-Render-Endpunkt als Windows-Systemaudio-Loopback.
/// Eingaben: Stoppflag, Capture-Handshake, EventBus und optionaler Render-Endpunktname; Samples gehen in den Interleaved-Ring.
/// Ausgabe: `Ok(())` nach kontrolliertem Stopp oder ein Windows-Audiofehler.
#[cfg(windows)]
fn run_wasapi_capture(
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<CaptureConfiguration, String>>,
    events: livetab_core::EventBus,
    selected_name: Option<String>,
) -> Result<(), String> {
    use wasapi::{
        AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat,
        initialize_mta,
    };

    initialize_mta()
        .ok()
        .map_err(|error| format!("WASAPI-COM konnte nicht initialisiert werden: {error}"))?;
    let enumerator = DeviceEnumerator::new()
        .map_err(|error| format!("WASAPI-Geräteauflistung fehlgeschlagen: {error}"))?;
    let device = if let Some(name) = selected_name.as_deref() {
        let collection = enumerator
            .get_device_collection(&Direction::Render)
            .map_err(|error| {
                format!("Windows-Audioendpunkte konnten nicht aufgelistet werden: {error}")
            })?;
        collection.get_device_with_name(name).map_err(|error| {
            format!("Ausgewählter Audioendpunkt ‚{name}‘ ist nicht verfügbar: {error}")
        })?
    } else {
        enumerator
            .get_default_device(&Direction::Render)
            .map_err(|error| format!("Kein aktiver Windows-Ausgabeendpunkt: {error}"))?
    };
    let device_name = device
        .get_friendlyname()
        .unwrap_or_else(|_| "Windows-Ausgabe".into());
    let mut client: AudioClient = device
        .get_iaudioclient()
        .map_err(|error| format!("WASAPI-AudioClient konnte nicht geöffnet werden: {error}"))?;
    let format = WaveFormat::new(32, 32, &SampleType::Float, 48_000, 2, None);
    let buffer_duration_hns = client
        .get_device_period()
        .map(|(_, minimum)| minimum)
        .unwrap_or(200_000)
        .max(200_000);
    let mode = StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns,
    };
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|error| format!("WASAPI-Loopback konnte nicht initialisiert werden: {error}"))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|error| format!("WASAPI-CaptureClient konnte nicht geöffnet werden: {error}"))?;
    client
        .start_stream()
        .map_err(|error| format!("WASAPI-Loopback konnte nicht starten: {error}"))?;
    let sample_rate = format.get_samplespersec();
    let channels = usize::from(format.get_nchannels());
    let block_align = format.get_blockalign() as usize;
    let channel_count = u16::try_from(channels)
        .map_err(|_| "WASAPI meldet eine ungültige Kanalzahl.".to_string())?;
    let transport = Arc::new(
        InterleavedAudioRingBuffer::new(sample_rate as usize * 2, channel_count)
            .map_err(|error| format!("WASAPI-Capturepuffer konnte nicht angelegt werden: {error}"))?,
    );
    let configuration = CaptureConfiguration {
        sample_rate,
        channels: channel_count,
        transport,
    };
    startup
        .send(Ok(configuration.clone()))
        .map_err(|_| "Capture-Start wurde vom Aufrufer abgebrochen.".to_string())?;
    events.publish(AppEvent::AudioCaptureChanged {
        running: true,
        source: format!("WASAPI-Loopback · {device_name} · {sample_rate} Hz · {channels} Kanal/Kanäle"),
    });

    let packet_buffer_frames = (sample_rate as usize / 4).max(1);
    let packet_buffer_bytes = packet_buffer_frames
        .checked_mul(block_align)
        .ok_or_else(|| "WASAPI-Paketpuffergröße läuft über.".to_string())?;
    let mut packet_bytes = vec![0_u8; packet_buffer_bytes];
    while !stop.load(Ordering::Acquire) {
        let mut saw_packet = false;
        loop {
            let Some(frames) = capture.get_next_packet_size().map_err(|error| {
                format!("WASAPI-Paketgröße konnte nicht gelesen werden: {error}")
            })?
            else {
                break;
            };
            if frames == 0 {
                break;
            }
            let required_bytes = (frames as usize)
                .checked_mul(block_align)
                .ok_or_else(|| "WASAPI-Paketgröße läuft über.".to_string())?;
            if required_bytes > packet_bytes.len() {
                return Err(format!(
                    "WASAPI-Paket mit {} Frames überschreitet den vorallokierten Capturepuffer.",
                    frames
                ));
            }
            let (frames_read, _buffer_info) =
                capture.read_from_device(&mut packet_bytes[..required_bytes]).map_err(|error| {
                    format!("WASAPI-Capturepuffer konnte nicht gelesen werden: {error}")
                })?;
            let byte_count = (frames_read as usize)
                .saturating_mul(block_align)
                .min(required_bytes);
            configuration
                .transport
                .push_f32_le_bytes(&packet_bytes[..byte_count])
                .map_err(|error| format!("WASAPI-Captureframe konnte nicht übertragen werden: {error}"))?;
            saw_packet = true;
        }
        if !saw_packet {
            thread::sleep(Duration::from_millis(4));
        }
    }
    let stop_result = client
        .stop_stream()
        .map_err(|error| format!("WASAPI-Loopback konnte nicht sauber gestoppt werden: {error}"));
    events.publish(AppEvent::AudioCaptureChanged {
        running: false,
        source: "WASAPI-Loopback".into(),
    });
    stop_result
}

/// Zweck: Verarbeitet Android/iOS-Ziele ohne versehentlich das Mikrofon als Systemloopback auszugeben.
/// Eingaben: Stoppflag, Start-Handshake und EventBus; Samples gehen über einen vorallokierten Interleaved-Ring.
/// Ausgabe: Mobile Mikrofonaufnahme oder klarer Hinweis auf eine nicht integrierte Plattform.
#[cfg(all(not(windows), any(target_os = "android", target_os = "ios")))]
fn run_platform_capture(
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<CaptureConfiguration, String>>,
    events: livetab_core::EventBus,
) -> Result<(), String> {
    run_cpal_capture(stop, startup, events)
}

/// Zweck: Erfasst auf Android/iOS den freigegebenen Standard-Mikrofoneingang und erhält dessen Quellkanäle.
/// Eingaben: Stoppsignal, Start-Handshake und EventBus; Callback schreibt normalisierte Frames nichtblockierend in den Ring.
/// Ausgabe: `Ok(())` nach Stopp oder ein Geräte-/Berechtigungsfehler.
#[cfg(all(not(windows), any(target_os = "android", target_os = "ios")))]
fn run_cpal_capture(
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<CaptureConfiguration, String>>,
    events: livetab_core::EventBus,
) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or_else(|| {
        "Kein Standard-Mikrofon verfügbar; die App benötigt eine Audioaufnahmefreigabe.".to_string()
    })?;
    let supported = device
        .default_input_config()
        .map_err(|error| format!("Mikrofonformat konnte nicht gelesen werden: {error}"))?;
    let sample_rate = supported.sample_rate().0;
    let channels = usize::from(supported.channels());
    let channel_count = u16::try_from(channels)
        .map_err(|_| "Das native Mikrofon meldet zu viele Kanäle.".to_string())?;
    let transport = Arc::new(
        InterleavedAudioRingBuffer::new(sample_rate as usize * 2, channel_count)
            .map_err(|error| format!("Mikrofon-Capturepuffer konnte nicht angelegt werden: {error}"))?,
    );
    let config: cpal::StreamConfig = supported.clone().into();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let output = Arc::clone(&transport);
            device.build_input_stream(
                &config,
                move |data: &[f32], _| { let _ = output.push_f32_interleaved(data); },
                move |error| eprintln!("local microphone error: {error}"),
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let output = Arc::clone(&transport);
            device.build_input_stream(
                &config,
                move |data: &[i16], _| { let _ = output.push_i16_interleaved(data); },
                move |error| eprintln!("local microphone error: {error}"),
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let output = Arc::clone(&transport);
            device.build_input_stream(
                &config,
                move |data: &[u16], _| { let _ = output.push_u16_interleaved(data); },
                move |error| eprintln!("local microphone error: {error}"),
                None,
            )
        }
        format => {
            return Err(format!(
                "Das native Mikrofonformat {format:?} wird nicht unterstützt."
            ));
        }
    }
    .map_err(|error| format!("Mikrofonstream konnte nicht erstellt werden: {error}"))?;
    stream
        .play()
        .map_err(|error| format!("Mikrofonstream konnte nicht gestartet werden: {error}"))?;
    let configuration = CaptureConfiguration {
        sample_rate,
        channels: channel_count,
        transport,
    };
    startup
        .send(Ok(configuration))
        .map_err(|_| "Capture-Start wurde vom Aufrufer abgebrochen.".to_string())?;
    events.publish(AppEvent::AudioCaptureChanged {
        running: true,
        source: format!("Mikrofon · lokale Eingabe · {sample_rate} Hz · {channels} Kanal/Kanäle"),
    });
    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    events.publish(AppEvent::AudioCaptureChanged {
        running: false,
        source: "Mikrofon".into(),
    });
    Ok(())
}

/// Zweck: Meldet auf nicht implementierten Desktopzielen einen klaren statt irreführenden Capturefehler.
/// Eingaben: Stoppsignal und Start-Handshake; es wird keine Quelle geöffnet.
/// Ausgabe: Immer ein beschreibender Fehler; es wird keine unbeabsichtigte Quelle geöffnet.
#[cfg(all(not(windows), not(any(target_os = "android", target_os = "ios"))))]
fn run_platform_capture(
    _stop: Arc<AtomicBool>,
    startup: SyncSender<Result<CaptureConfiguration, String>>,
    _events: livetab_core::EventBus,
) -> Result<(), String> {
    let message = "Liveaufnahme ist in diesem Plattformprofil nicht integriert.".to_string();
    let _ = startup.send(Err(message.clone()));
    Err(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft den Capturefortschritt anhand vorhandener Frames statt einer Zeitschätzung.
    /// Eingaben: Synthetische Stereo-Session mit 125 von 500 Frames; es werden keine Geräte gestartet.
    /// Ausgabe: Keine Rückgabe; der Snapshot muss Frames, Grenze und Rate exakt wiedergeben.
    #[test]
    fn recording_progress_snapshot_reports_captured_frames() {
        let session = CaptureSession {
            stop: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
            recording: Arc::new(Mutex::new(Some(vec![0.0; 250]))),
            recording_limit_reached: Arc::new(AtomicBool::new(false)),
            sample_rate: 100,
            channels: 2,
            maximum_recording_samples: 1_000,
        };

        assert_eq!(
            session.recording_progress_snapshot(),
            Some((125, 500, 100))
        );
    }

    /// Zweck: Prüft, dass der Recorder die Originalkanäle beim Abschluss erhält.
    /// Eingaben: Synthetische Stereo-Samples, Abtastrate und Kanalzahl; kein Gerät wird geöffnet.
    /// Ausgabe: Keine Rückgabe; der Abschluss liefert unveränderte Interleaved-Samples und Metadaten.
    #[test]
    fn finishing_capture_preserves_original_interleaved_channels() {
        let samples = vec![0.5, -0.5, 0.25, 0.75];
        let session = CaptureSession {
            stop: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
            recording: Arc::new(Mutex::new(Some(samples.clone()))),
            recording_limit_reached: Arc::new(AtomicBool::new(false)),
            sample_rate: 48_000,
            channels: 2,
            maximum_recording_samples: 8,
        };

        assert_eq!(
            session.finish_recording().unwrap(),
            (samples, 48_000, 2)
        );
    }

    /// Zweck: Weist einen leeren Recorderpuffer kontrolliert zurück.
    /// Eingaben: Synthetische Session mit leerem Mehrkanalpuffer; es werden keine Geräte gestartet.
    /// Ausgabe: Keine Rückgabe; der Abschluss muss einen verständlichen Fehler liefern.
    #[test]
    fn finishing_capture_rejects_empty_recording() {
        let session = CaptureSession {
            stop: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
            recording: Arc::new(Mutex::new(Some(Vec::new()))),
            recording_limit_reached: Arc::new(AtomicBool::new(false)),
            sample_rate: 48_000,
            channels: 2,
            maximum_recording_samples: 8,
        };

        assert!(session.finish_recording().is_err());
    }
}
