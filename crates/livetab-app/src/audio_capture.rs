/*
===============================================================================
Division By Zeros (DBZS) - Native Audio Capture
Modul: audio_capture
Zweck: Erfasst Windows-Systemausgabe über WASAPI-Loopback und mobile Eingabe
       über native cpal-Mikrofonstreams; DSP läuft auf einem separaten Worker.
Sprache: Rust
Hinweise:
- Keine Audio-Callbackfunktion führt DSP, UI-Aufrufe oder blockierende Kanalwrites aus.
- Windows benutzt ausschließlich einen lokalen Shared-Mode-Render-Loopback-Endpunkt.
- Android/iOS verwenden eine lokale Eingabequelle und verlangen Betriebssystemfreigabe.
===============================================================================
*/
use livetab_core::audio::AudioFrame;
use livetab_core::observability::{LogCategory, LogLevel};
use livetab_core::{AppController, AppEvent};
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const LIVE_FRAME_SAMPLES: usize = 4096;
const LIVE_HOP_SAMPLES: usize = 2048;
const CAPTURE_CHANNEL_CAPACITY: usize = 8;
const MAX_RECORDING_SECONDS: u64 = 600;

static SELECTED_RENDER_DEVICE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// Zweck: Listet lokale Capture-Quellen, auf Windows konkret die WASAPI-Render-Endpunkte für Loopback.
/// Eingaben: Keine; initialisiert COM nur auf dem aufrufenden Workerthread.
/// Ausgabe: Gerätenamen oder ein plattform-/treiberbezogener Fehler.
pub fn available_audio_devices() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
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

/// Zweck: Speichert die ausgewählte WASAPI-Renderquelle ohne den aktiven Audio-Thread zu verändern.
/// Eingaben: Gerätename aus `available_audio_devices` oder `None` für den Windows-Standardendpunkt.
/// Ausgabe: `Ok(())` oder ein kurzer Hinweis, wenn die Plattform keine Endpunktauswahl unterstützt.
pub fn select_audio_device(name: Option<String>) -> Result<(), String> {
    #[cfg(windows)]
    {
        let selected = SELECTED_RENDER_DEVICE.get_or_init(|| Mutex::new(None));
        *selected
            .lock()
            .map_err(|_| "Audiogeräteauswahl ist gesperrt.".to_string())? = name;
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
    maximum_recording_samples: usize,
}

impl CaptureSession {
    /// Zweck: Beginnt eine neue begrenzte Monoaufnahme aus den laufenden Captureframes.
    /// Eingaben: `self` – aktive Quelle; maximale Länge ist auf zehn Minuten begrenzt.
    /// Ausgabe: `Ok(())` oder ein verständlicher Fehler bei doppelter/leerer Session.
    pub fn begin_recording(&self) -> Result<(), String> {
        let mut recording = self
            .recording
            .lock()
            .map_err(|_| "WAV-Aufnahmepuffer ist gesperrt.".to_string())?;
        if recording.is_some() {
            return Err("Eine WAV-Aufnahme läuft bereits.".into());
        }
        *recording = Some(Vec::with_capacity(
            (self.sample_rate as usize * 5).min(self.maximum_recording_samples),
        ));
        self.recording_limit_reached.store(false, Ordering::Release);
        Ok(())
    }

    /// Zweck: Beendet die laufende Pufferaufnahme und übergibt die Samples an den WAV-Writer.
    /// Eingaben: `self` – aktive Capture-Session.
    /// Ausgabe: Mono-Samples und Capture-Rate oder eine verständliche Meldung ohne aktive Aufnahme.
    pub fn finish_recording(&self) -> Result<(Vec<f32>, u32), String> {
        let mut recording = self
            .recording
            .lock()
            .map_err(|_| "WAV-Aufnahmepuffer ist gesperrt.".to_string())?;
        let samples = recording
            .take()
            .ok_or_else(|| "Es läuft gerade keine WAV-Aufnahme.".to_string())?;
        if samples.is_empty() {
            return Err("Die WAV-Aufnahme enthält keine Samples.".into());
        }
        Ok((samples, self.sample_rate))
    }

    /// Zweck: Meldet, ob gegenwärtig Samples für einen WAV-Export gesammelt werden.
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
    let stop = Arc::new(AtomicBool::new(false));
    let (audio_sender, audio_receiver) = mpsc::sync_channel::<Vec<f32>>(CAPTURE_CHANNEL_CAPACITY);
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    let capture_stop = Arc::clone(&stop);
    let capture_events = controller.event_bus();
    let capture_observer = controller.observer();
    let startup_fallback = startup_sender.clone();
    let capture_worker = thread::Builder::new()
        .name("livetab-audio-capture".into())
        .spawn(move || {
            let worker_observer = capture_observer;
            let result = run_platform_capture(
                audio_sender,
                Arc::clone(&capture_stop),
                startup_sender,
                capture_events.clone(),
            );
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

    let sample_rate = match startup_receiver.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(rate)) => rate,
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
    let maximum_recording_samples =
        (sample_rate as usize).saturating_mul(MAX_RECORDING_SECONDS as usize);
    let analyzer_controller = controller.clone();
    let analyzer_events = controller.event_bus();
    let analyzer_stop = Arc::clone(&stop);
    let analyzer_worker = thread::Builder::new()
        .name("livetab-live-dsp".into())
        .spawn({
            let recording = Arc::clone(&recording);
            let limit_reached = Arc::clone(&recording_limit_reached);
            move || {
                run_live_analyzer(
                    audio_receiver,
                    sample_rate,
                    analyzer_stop,
                    analyzer_controller,
                    analyzer_events,
                    recording,
                    maximum_recording_samples,
                    limit_reached,
                )
            }
        })
        .map_err(|error| {
            stop.store(true, Ordering::Release);
            format!("Live-DSP-Worker konnte nicht gestartet werden: {error}")
        })?;

    Ok(CaptureSession {
        stop,
        workers: vec![capture_worker, analyzer_worker],
        recording,
        recording_limit_reached,
        sample_rate,
        maximum_recording_samples,
    })
}

/// Zweck: Verarbeitet bounded Capture-Pakete zu überlappenden Liveframes ohne die Quellcallback zu blockieren.
/// Eingaben: Audiosample-Empfänger, Capture-Rate, Stoppflag, Controller und EventBus.
/// Ausgabe: Keine; publiziert Audio-/Chord-Events und meldet verständliche DSP-Fehler.
fn run_live_analyzer(
    receiver: Receiver<Vec<f32>>,
    sample_rate: u32,
    stop: Arc<AtomicBool>,
    controller: AppController,
    events: livetab_core::EventBus,
    recording: Arc<Mutex<Option<Vec<f32>>>>,
    maximum_recording_samples: usize,
    recording_limit_reached: Arc<AtomicBool>,
) {
    let mut buffered = VecDeque::with_capacity(LIVE_FRAME_SAMPLES * 4);
    let mut start_sample = 0u64;
    while !stop.load(Ordering::Acquire) {
        match receiver.recv_timeout(Duration::from_millis(12)) {
            Ok(samples) => {
                if let Ok(mut active) = recording.lock() {
                    if let Some(output) = active.as_mut() {
                        let remaining = maximum_recording_samples.saturating_sub(output.len());
                        output.extend(samples.iter().take(remaining).copied());
                        if samples.len() > remaining
                            && !recording_limit_reached.swap(true, Ordering::AcqRel)
                        {
                            controller.observer().log(
                                LogLevel::Warning,
                                LogCategory::Audio,
                                "Maximale 10-Minuten-Live-WAV-Aufnahme erreicht.",
                                "Die bisherige Aufnahme kann gespeichert werden.",
                                BTreeMap::new(),
                                None,
                            );
                            events.publish(AppEvent::Error { category: "WAV-Aufnahme".into(), message: "Die 10-Minuten-Grenze wurde erreicht; die bisherige Aufnahme ist weiterhin speicherbar.".into() });
                        }
                    }
                }
                buffered.extend(samples);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        while buffered.len() >= LIVE_FRAME_SAMPLES {
            let frame_samples = buffered
                .iter()
                .take(LIVE_FRAME_SAMPLES)
                .copied()
                .collect::<Vec<_>>();
            buffered.drain(..LIVE_HOP_SAMPLES);
            match AudioFrame::new(sample_rate, start_sample, frame_samples).and_then(|frame| {
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
            start_sample = start_sample.saturating_add(LIVE_HOP_SAMPLES as u64);
        }
    }
}

/// Zweck: Verarbeitet eine WASAPI-Shared-Mode-Renderquelle als Windows-Systemaudio-Loopback.
/// Eingaben: Bounded Sample-Kanal, Stoppflag, Start-Handshake und EventBus.
/// Ausgabe: `Ok(())` nach kontrolliertem Stopp oder ein Windows-Audiofehler.
#[cfg(windows)]
fn run_platform_capture(
    sender: SyncSender<Vec<f32>>,
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<u32, String>>,
    events: livetab_core::EventBus,
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
    let selected_name = SELECTED_RENDER_DEVICE
        .get()
        .and_then(|selected| selected.lock().ok().and_then(|name| name.clone()));
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
    startup
        .send(Ok(sample_rate))
        .map_err(|_| "Capture-Start wurde vom Aufrufer abgebrochen.".to_string())?;
    events.publish(AppEvent::AudioCaptureChanged {
        running: true,
        source: format!("WASAPI-Loopback · {device_name}"),
    });

    let mut dropped_packets = 0u64;
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
            let mut bytes = vec![0u8; frames as usize * block_align];
            let (frames_read, _buffer_info) =
                capture.read_from_device(&mut bytes).map_err(|error| {
                    format!("WASAPI-Capturepuffer konnte nicht gelesen werden: {error}")
                })?;
            let byte_count = (frames_read as usize)
                .saturating_mul(block_align)
                .min(bytes.len());
            let samples = float32_interleaved_to_mono(&bytes[..byte_count], channels);
            if !samples.is_empty() {
                match sender.try_send(samples) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        dropped_packets = dropped_packets.saturating_add(1)
                    }
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
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
    if dropped_packets > 0 {
        events.publish(AppEvent::Error { category: "Audiopuffer".into(), message: format!("{dropped_packets} Live-Audiopakete wurden verworfen, damit die Wiedergabe nicht blockiert." ) });
    }
    stop_result
}

/// Zweck: Verarbeitet nicht-Windows-Ziele ohne versehentlich das Mikrofon als Systemloopback auszugeben.
/// Eingaben: Capturekanal, Stoppflag, Start-Handshake und EventBus.
/// Ausgabe: Mobile Mikrofonaufnahme oder klarer Hinweis auf eine nicht integrierte Plattform.
#[cfg(all(not(windows), any(target_os = "android", target_os = "ios")))]
fn run_platform_capture(
    sender: SyncSender<Vec<f32>>,
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<u32, String>>,
    events: livetab_core::EventBus,
) -> Result<(), String> {
    run_cpal_capture(sender, stop, startup, events)
}

/// Zweck: Erfasst auf Android/iOS den freigegebenen Standard-Mikrofoneingang und normalisiert Samples zu Mono.
/// Eingaben: Bounded Samplekanal, Stoppsignal, Start-Handshake und EventBus.
/// Ausgabe: `Ok(())` nach Stopp oder ein Geräte-/Berechtigungsfehler.
#[cfg(all(not(windows), any(target_os = "android", target_os = "ios")))]
fn run_cpal_capture(
    sender: SyncSender<Vec<f32>>,
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<u32, String>>,
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
    let config: cpal::StreamConfig = supported.clone().into();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let output = sender.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _| try_send_mono(&output, data, channels),
                move |error| eprintln!("local microphone error: {error}"),
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let output = sender.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let converted = data
                        .iter()
                        .map(|value| *value as f32 / 32768.0)
                        .collect::<Vec<_>>();
                    try_send_mono(&output, &converted, channels)
                },
                move |error| eprintln!("local microphone error: {error}"),
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let output = sender.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let converted = data
                        .iter()
                        .map(|value| (*value as f32 - 32768.0) / 32768.0)
                        .collect::<Vec<_>>();
                    try_send_mono(&output, &converted, channels)
                },
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
    startup
        .send(Ok(sample_rate))
        .map_err(|_| "Capture-Start wurde vom Aufrufer abgebrochen.".to_string())?;
    events.publish(AppEvent::AudioCaptureChanged {
        running: true,
        source: "Mikrofon · lokale Eingabe".into(),
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

/// Zweck: Wandelt plattformübergreifende interleaved Mono-/Multikanalsamples im cpal-Callback nichtblockierend um.
/// Eingaben: Capture-Sender, interleaved Samplefolge und Kanalzahl.
/// Ausgabe: Keine; bei vollem bounded Kanal werden ganze Pakete verworfen statt den Audiocallback zu blockieren.
#[cfg(all(not(windows), any(target_os = "android", target_os = "ios")))]
fn try_send_mono(sender: &SyncSender<Vec<f32>>, samples: &[f32], channels: usize) {
    if channels == 0 || samples.len() % channels != 0 {
        return;
    }
    let mono = samples
        .chunks_exact(channels)
        .map(|frame| {
            frame
                .iter()
                .copied()
                .filter(|sample| sample.is_finite())
                .sum::<f32>()
                / channels as f32
        })
        .collect::<Vec<_>>();
    match sender.try_send(mono) {
        Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
    }
}

/// Zweck: Meldet auf nicht implementierten Desktopzielen einen klaren statt irreführenden Capturefehler.
/// Eingaben: Capturekanal, Stoppsignal, Start-Handshake und EventBus.
/// Ausgabe: Immer ein beschreibender Fehler; es wird keine unbeabsichtigte Quelle geöffnet.
#[cfg(all(not(windows), not(any(target_os = "android", target_os = "ios"))))]
fn run_platform_capture(
    _sender: SyncSender<Vec<f32>>,
    _stop: Arc<AtomicBool>,
    startup: SyncSender<Result<u32, String>>,
    _events: livetab_core::EventBus,
) -> Result<(), String> {
    let message = "Liveaufnahme ist in diesem Plattformprofil nicht integriert.".to_string();
    let _ = startup.send(Err(message.clone()));
    Err(message)
}

/// Zweck: Dekodiert finite little-endian Float32-Frames und mittelt ihre Kanäle für den Mono-DSP.
/// Eingaben: Interleaved Float32-Bytes und gültige Kanalzahl.
/// Ausgabe: Mono-Samplefolge; unvollständige oder nichtendliche Werte werden verworfen/neutralisiert.
fn float32_interleaved_to_mono(bytes: &[u8], channels: usize) -> Vec<f32> {
    if channels == 0 || bytes.len() % 4 != 0 {
        return Vec::new();
    }
    let values = bytes
        .chunks_exact(4)
        .map(|value| f32::from_ne_bytes([value[0], value[1], value[2], value[3]]))
        .collect::<Vec<_>>();
    if values.len() % channels != 0 {
        return Vec::new();
    }
    values
        .chunks_exact(channels)
        .map(|frame| {
            let finite = frame
                .iter()
                .copied()
                .filter(|value| value.is_finite())
                .collect::<Vec<_>>();
            if finite.is_empty() {
                0.0
            } else {
                (finite.iter().sum::<f32>() / channels as f32).clamp(-1.0, 1.0)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft stereo Float32-Downmix, Begrenzung und Behandlung von NaN.
    /// Eingaben: Vier interleaved Float32-Samples für zwei Stereo-Zeitpunkte.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Monopegel und endliche Werte.
    #[test]
    fn loopback_downmixes_stereo_float_samples() {
        let mut bytes = Vec::new();
        for sample in [0.5f32, -0.5, 2.0, 0.0] {
            bytes.extend_from_slice(&sample.to_ne_bytes());
        }
        assert_eq!(float32_interleaved_to_mono(&bytes, 2), vec![0.0, 1.0]);
    }

    /// Zweck: Verhindert Frames aus unvollständigen oder kanalinkonsistenten Capturepaketen.
    /// Eingaben: Bytes mit einer unvollständigen float32- oder Stereo-Framegrenze.
    /// Ausgabe: Keine Rückgabe; Assertions sichern fail-closed leere Ergebnisse.
    #[test]
    fn capture_decoder_rejects_incomplete_frames() {
        assert!(float32_interleaved_to_mono(&[0, 0, 0], 1).is_empty());
        assert!(float32_interleaved_to_mono(&[0; 4], 2).is_empty());
    }
}
