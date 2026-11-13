/*
===============================================================================
Division By Zeros (DBZS) - ASIO-Captureadapter
Modul: asio_capture
Zweck: Listet ASIO-Treiber auf und erfasst Interface-Eingänge über azo, ohne
       das Steinberg-ASIO-SDK oder den WASAPI-Systemmix zu verwenden.
Sprache: Rust
Hinweise:
- ASIO-Treiberinstanzen und COM-Aufrufe bleiben auf dem Capture-Workerthread.
- Der Echtzeitcallback schreibt nur einen Bufferindex in einen begrenzten,
  nicht blockierenden Kanal; Samplekonvertierung und DSP laufen außerhalb.
- Der erste Eingangskanal (mono) beziehungsweise das erste Eingangskanalpaar
  wird in einen Mono-Stream für die bestehende Analyse umgewandelt.
===============================================================================
*/
use azo::driver::{self, Driver};
use azo::dto::ChannelId;
use azo::sys::{self, Callbacks, SampleType};
use crossbeam_channel::{Receiver, Sender};
use std::collections::BTreeMap;
use std::ffi::{c_long, c_void};
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const CALLBACK_QUEUE_CAPACITY: usize = 32;
const MAX_INPUT_CHANNELS: c_long = 2;

static ASIO_DRIVER_CATALOG: OnceLock<Mutex<BTreeMap<String, azo::windows_core::GUID>>> =
    OnceLock::new();
static BUFFER_NOTIFICATIONS: OnceLock<(Sender<c_long>, Receiver<c_long>)> = OnceLock::new();
static DROPPED_CALLBACK_NOTIFICATIONS: AtomicU64 = AtomicU64::new(0);
static SAMPLE_RATE_CHANGED: AtomicBool = AtomicBool::new(false);
static ASIO_CALLBACKS: Callbacks = Callbacks {
    buffer_switch: asio_buffer_switch,
    sample_rate_did_change: asio_sample_rate_did_change,
    asio_message: asio_message,
    buffer_switch_time_info: asio_buffer_switch_time_info,
};

/// Zweck: Listet registrierte ASIO-Treiber und speichert ihre Anzeigenamen für die spätere Auswahl.
/// Eingaben: Keine; die Windows-Registry wird durch `azo` auf dem aufrufenden Scanworker gelesen.
/// Ausgabe: Eindeutige, mit `ASIO ·` markierte Anzeigenamen oder ein verständlicher Registryfehler.
pub fn available_driver_names() -> Result<Vec<String>, String> {
    let metadata = driver::Metadata::enumerate()
        .map_err(|error| format!("ASIO-Treiber konnten nicht aufgelistet werden: {error}"))?;
    let catalog = ASIO_DRIVER_CATALOG.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut entries = catalog
        .lock()
        .map_err(|_| "ASIO-Treiberliste ist gesperrt.".to_string())?;
    entries.clear();

    let mut name_counts = BTreeMap::<String, usize>::new();
    let mut labels = Vec::with_capacity(metadata.len());
    for item in metadata {
        let base = format!("ASIO · {}", item.description.to_string_lossy());
        let count = name_counts.entry(base.clone()).or_default();
        *count += 1;
        let label = if *count == 1 {
            base
        } else {
            format!("{base} ({count})")
        };
        entries.insert(label.clone(), item.clsid);
        labels.push(label);
    }
    Ok(labels)
}

/// Zweck: Prüft, ob ein angezeigter ASIO-Treibername aus dem letzten erfolgreichen Scan stammt.
/// Eingaben: `label` – vollständiger Anzeigename aus `available_driver_names`.
/// Ausgabe: `Ok(())` bei bekannter Treiberkennung oder ein Hinweis auf einen neuen Gerätescan.
pub fn validate_driver_label(label: &str) -> Result<(), String> {
    lookup_driver_clsid(label).map(|_| ())
}

/// Zweck: Öffnet einen ausgewählten ASIO-Treiber und speist seine ersten ein bis zwei Eingänge in den Live-Capturekanal.
/// Eingaben: Bounded Sample-Sender, Stoppsignal, Start-Handshake, EventBus und ASIO-Anzeigename.
/// Ausgabe: `Ok(())` nach Stopp oder eine verständliche Treiber-, Format- oder Streamingfehlermeldung.
pub fn run_capture(
    sender: SyncSender<Vec<f32>>,
    stop: Arc<AtomicBool>,
    startup: SyncSender<Result<u32, String>>,
    events: livetab_core::EventBus,
    driver_label: &str,
) -> Result<(), String> {
    let clsid = match lookup_driver_clsid(driver_label) {
        Ok(clsid) => clsid,
        Err(message) => {
            let _ = startup.send(Err(message.clone()));
            return Err(message);
        }
    };
    let driver = match driver::SafeHandle::new(&clsid) {
        Ok(driver) => driver,
        Err(error) => {
            let message =
                format!("ASIO-Treiber ‚{driver_label}‘ konnte nicht geöffnet werden: {error}");
            let _ = startup.send(Err(message.clone()));
            return Err(message);
        }
    };
    if !driver.init(None) {
        let message = format!(
            "ASIO-Treiber ‚{driver_label}‘ konnte nicht initialisiert werden: {}",
            driver.last_error().to_string_lossy()
        );
        let _ = startup.send(Err(message.clone()));
        return Err(message);
    }

    let (_, buffer_receiver) =
        BUFFER_NOTIFICATIONS.get_or_init(|| crossbeam_channel::bounded(CALLBACK_QUEUE_CAPACITY));
    while buffer_receiver.try_recv().is_ok() {}
    DROPPED_CALLBACK_NOTIFICATIONS.store(0, Ordering::Release);
    SAMPLE_RATE_CHANGED.store(false, Ordering::Release);

    let mut buffers_created = false;
    let mut stream_started = false;
    let mut startup_succeeded = false;
    let mut capture_source = driver_label.to_string();
    let result = (|| -> Result<(), String> {
        let channel_counts = driver.channel_counts().map_err(|error| {
            format!("ASIO-Eingangskanäle konnten nicht gelesen werden: {error}")
        })?;
        if channel_counts.in_ <= 0 {
            return Err(format!(
                "ASIO-Treiber ‚{driver_label}‘ meldet keine Audioeingänge."
            ));
        }
        let channel_count = channel_counts.in_.min(MAX_INPUT_CHANNELS) as usize;
        let channel_ids = (0..channel_count)
            .map(|index| ChannelId {
                input: true,
                index: index as c_long,
            })
            .collect::<Vec<_>>();
        let channel_formats = channel_ids
            .iter()
            .map(|channel| {
                driver
                    .channel_info(*channel)
                    .map(|info| info.sample_type)
                    .map_err(|error| {
                        format!(
                            "ASIO-Eingangsformat für Kanal {} konnte nicht gelesen werden: {error}",
                            channel.index + 1
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for format in &channel_formats {
            if sample_width(*format).is_none() {
                return Err(format!(
                    "ASIO-Sampleformat {format:?} wird nicht als PCM unterstützt; bitte im Treiber ein PCM-Format wählen."
                ));
            }
        }

        let sample_rate = driver
            .get_sample_rate()
            .map_err(|error| format!("ASIO-Samplerate konnte nicht gelesen werden: {error}"))?;
        if !sample_rate.is_finite() || sample_rate <= 0.0 || sample_rate > u32::MAX as f64 {
            return Err(format!(
                "ASIO-Treiber meldet eine ungültige Samplerate: {sample_rate}"
            ));
        }
        let sample_rate = sample_rate.round() as u32;
        let buffer_size = driver
            .buffer_size()
            .map_err(|error| format!("ASIO-Buffergröße konnte nicht gelesen werden: {error}"))?
            .preferred;
        if buffer_size <= 0 {
            return Err(format!(
                "ASIO-Treiber meldet eine ungültige Buffergröße: {buffer_size}"
            ));
        }

        // SAFETY: Die Callbackstruktur ist statisch und lebt länger als die vom Treiber erzeugten Buffer.
        let buffer_channels = unsafe {
            driver.create_buffers(
                channel_ids,
                buffer_size as c_long,
                &raw const ASIO_CALLBACKS,
            )
        }
        .map_err(|error| format!("ASIO-Eingangsbuffer konnten nicht erstellt werden: {error}"))?
        .collect::<Vec<_>>();
        buffers_created = true;
        if buffer_channels.len() != channel_count {
            return Err(
                "ASIO-Treiber lieferte eine unerwartete Zahl an Eingangsbufferpaaren.".into(),
            );
        }
        if buffer_channels
            .iter()
            .flat_map(|pair| pair.iter())
            .any(|pointer| pointer.is_null())
        {
            return Err("ASIO-Treiber lieferte einen ungültigen Eingangsbuffer.".into());
        }

        driver.start().map_err(|error| {
            format!("ASIO-Aufnahmestream konnte nicht gestartet werden: {error}")
        })?;
        stream_started = true;
        let driver_name = driver_label.strip_prefix("ASIO · ").unwrap_or(driver_label);
        capture_source = if channel_count == 1 {
            format!("ASIO · {driver_name} · Eingang 1 · {sample_rate} Hz")
        } else {
            format!("ASIO · {driver_name} · Eingänge 1–2 · {sample_rate} Hz")
        };
        startup
            .send(Ok(sample_rate))
            .map_err(|_| "ASIO-Capture-Start wurde vom Aufrufer abgebrochen.".to_string())?;
        startup_succeeded = true;
        events.publish(livetab_core::AppEvent::AudioCaptureChanged {
            running: true,
            source: capture_source.clone(),
        });

        let frame_count = buffer_size as usize;
        let mut dropped_audio_packets = 0u64;
        while !stop.load(Ordering::Acquire) {
            if SAMPLE_RATE_CHANGED.swap(false, Ordering::AcqRel) {
                return Err("Die ASIO-Samplerate wurde während der Aufnahme geändert; Live-Capture wurde sicher beendet.".into());
            }
            let first_index = match buffer_receiver.recv_timeout(Duration::from_millis(8)) {
                Ok(index) => index,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return Err("ASIO-Bufferbenachrichtigungskanal wurde geschlossen.".into());
                }
            };
            for buffer_index in std::iter::once(first_index)
                .chain(std::iter::from_fn(|| buffer_receiver.try_recv().ok()))
            {
                let buffer_index = usize::try_from(buffer_index)
                    .ok()
                    .filter(|index| *index < 2)
                    .ok_or_else(|| {
                        "ASIO-Treiber meldete einen ungültigen Double-Bufferindex.".to_string()
                    })?;
                let mono = convert_input_buffers(
                    &buffer_channels,
                    &channel_formats,
                    buffer_index,
                    frame_count,
                )?;
                match sender.try_send(mono) {
                    Ok(()) => {}
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        dropped_audio_packets = dropped_audio_packets.saturating_add(1);
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return Ok(()),
                }
            }
        }
        if dropped_audio_packets > 0 {
            events.publish(livetab_core::AppEvent::Error {
                category: "ASIO-Audiopuffer".into(),
                message: format!("{dropped_audio_packets} ASIO-Audiopakete wurden verworfen, weil die Live-DSP-Pipeline ausgelastet war."),
            });
        }
        Ok(())
    })();

    let stop_result = if stream_started {
        driver
            .stop()
            .map_err(|error| format!("ASIO-Stream konnte nicht sauber gestoppt werden: {error}"))
    } else {
        Ok(())
    };
    let dispose_result = if buffers_created {
        driver.dispose_buffers().map_err(|error| {
            format!("ASIO-Eingangsbuffer konnten nicht freigegeben werden: {error}")
        })
    } else {
        Ok(())
    };
    while buffer_receiver.try_recv().is_ok() {}
    if stream_started {
        events.publish(livetab_core::AppEvent::AudioCaptureChanged {
            running: false,
            source: capture_source,
        });
    }
    if !startup_succeeded {
        if let Err(message) = &result {
            let _ = startup.send(Err(message.clone()));
        }
    }

    result?;
    stop_result?;
    dispose_result?;
    let missed = DROPPED_CALLBACK_NOTIFICATIONS.swap(0, Ordering::AcqRel);
    if missed > 0 {
        events.publish(livetab_core::AppEvent::Error {
            category: "ASIO-Callbackpuffer".into(),
            message: format!("{missed} ASIO-Bufferbenachrichtigungen konnten bei Überlast nicht angenommen werden."),
        });
    }
    Ok(())
}

/// Zweck: Sucht die COM-Klassenkennung zum exakten ASIO-Anzeigenamen aus dem letzten Scan.
/// Eingaben: `label` – markierter ASIO-Treibereintrag der Geräteauswahl.
/// Ausgabe: Kopierte Windows-GUID oder eine verständliche Meldung bei veraltetem/ungültigem Eintrag.
fn lookup_driver_clsid(label: &str) -> Result<azo::windows_core::GUID, String> {
    let catalog = ASIO_DRIVER_CATALOG.get().ok_or_else(|| {
        "ASIO-Treiberliste ist veraltet; bitte Audiogeräte erneut scannen.".to_string()
    })?;
    catalog
        .lock()
        .map_err(|_| "ASIO-Treiberliste ist gesperrt.".to_string())?
        .get(label)
        .copied()
        .ok_or_else(|| {
            "ASIO-Treiber ist nicht mehr verfügbar; bitte Audiogeräte erneut scannen.".to_string()
        })
}

/// Zweck: Meldet die Bytegröße unterstützter linearer PCM-Sampleformate.
/// Eingaben: `format` – ASIO-Sampleformat aus der Kanalmetadatenabfrage.
/// Ausgabe: Bytes je Sample oder `None` für nicht unterstützte DSD-/unbekannte Formate.
fn sample_width(format: SampleType) -> Option<usize> {
    match format {
        SampleType::PCM_I16_MSB | SampleType::PCM_I16_LSB => Some(2),
        SampleType::PCM_I24_MSB | SampleType::PCM_I24_LSB => Some(3),
        SampleType::PCM_I32_MSB
        | SampleType::PCM_I32_MSB_16
        | SampleType::PCM_I32_MSB_18
        | SampleType::PCM_I32_MSB_20
        | SampleType::PCM_I32_MSB_24
        | SampleType::PCM_F32_MSB
        | SampleType::PCM_F64_MSB
        | SampleType::PCM_I32_LSB
        | SampleType::PCM_I32_LSB_16
        | SampleType::PCM_I32_LSB_18
        | SampleType::PCM_I32_LSB_20
        | SampleType::PCM_I32_LSB_24
        | SampleType::PCM_F32_LSB
        | SampleType::PCM_F64_LSB => match format {
            SampleType::PCM_F64_MSB | SampleType::PCM_F64_LSB => Some(8),
            _ => Some(4),
        },
        _ => None,
    }
}

/// Zweck: Kopiert ein Double-Bufferpaket, konvertiert bis zu zwei getrennte ASIO-Eingänge und mischt sie zu Mono.
/// Eingaben: Kanalbufferpaare, zugehörige Sampleformate, Double-Bufferindex und Framezahl.
/// Ausgabe: Mono-Floatframes oder ein Fehler bei ungültigem Pointer/Format.
fn convert_input_buffers(
    buffers: &[[*mut c_void; 2]],
    formats: &[SampleType],
    buffer_index: usize,
    frame_count: usize,
) -> Result<Vec<f32>, String> {
    if buffers.is_empty() || buffers.len() != formats.len() || buffer_index >= 2 {
        return Err("ASIO-Eingangsbuffer und Kanalformate sind inkonsistent.".into());
    }
    let mut channel_data = Vec::with_capacity(buffers.len());
    for (channel_index, (pair, format)) in buffers.iter().zip(formats).enumerate() {
        let width = sample_width(*format)
            .ok_or_else(|| format!("ASIO-Sampleformat {format:?} wird nicht unterstützt."))?;
        let pointer = pair[buffer_index];
        if pointer.is_null() {
            return Err(format!(
                "ASIO-Eingangsbuffer für Kanal {} ist ungültig.",
                channel_index + 1
            ));
        }
        // SAFETY: `create_buffers` vergibt pro Kanal und Double-Buffer mindestens `frame_count * width` Bytes.
        let bytes = unsafe { slice::from_raw_parts(pointer.cast::<u8>(), frame_count * width) };
        channel_data.push((bytes, *format));
    }

    let mut mono = Vec::with_capacity(frame_count);
    for frame_index in 0..frame_count {
        let sum = channel_data
            .iter()
            .map(|(bytes, format)| decode_sample(bytes, frame_index, *format))
            .sum::<f32>();
        mono.push((sum / channel_data.len() as f32).clamp(-1.0, 1.0));
    }
    Ok(mono)
}

/// Zweck: Dekodiert ein einzelnes ASIO-PCM-Sample einschließlich Endianness und 32-Bit-Ausrichtungsformaten.
/// Eingaben: Vollständiger Kanalbuffer, Frameindex und ASIO-Sampleformat.
/// Ausgabe: Endlicher, auf den Normalbereich begrenzter Floatwert; fehlerhafte/NaN-Samples werden null.
fn decode_sample(bytes: &[u8], sample_index: usize, format: SampleType) -> f32 {
    let Some(width) = sample_width(format) else {
        return 0.0;
    };
    let Some(offset) = sample_index.checked_mul(width) else {
        return 0.0;
    };
    let Some(sample) = bytes.get(offset..offset.saturating_add(width)) else {
        return 0.0;
    };
    let value = match format {
        SampleType::PCM_I16_MSB => i16::from_be_bytes([sample[0], sample[1]]) as f32 / 32_768.0,
        SampleType::PCM_I16_LSB => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0,
        SampleType::PCM_I24_MSB => decode_i24(sample, true),
        SampleType::PCM_I24_LSB => decode_i24(sample, false),
        SampleType::PCM_I32_MSB => {
            (i32::from_be_bytes([sample[0], sample[1], sample[2], sample[3]]) as f64
                / 2_147_483_648.0) as f32
        }
        SampleType::PCM_I32_LSB => {
            (i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f64
                / 2_147_483_648.0) as f32
        }
        SampleType::PCM_I32_MSB_16 => decode_aligned_i32(sample, true, 16),
        SampleType::PCM_I32_MSB_18 => decode_aligned_i32(sample, true, 18),
        SampleType::PCM_I32_MSB_20 => decode_aligned_i32(sample, true, 20),
        SampleType::PCM_I32_MSB_24 => decode_aligned_i32(sample, true, 24),
        SampleType::PCM_I32_LSB_16 => decode_aligned_i32(sample, false, 16),
        SampleType::PCM_I32_LSB_18 => decode_aligned_i32(sample, false, 18),
        SampleType::PCM_I32_LSB_20 => decode_aligned_i32(sample, false, 20),
        SampleType::PCM_I32_LSB_24 => decode_aligned_i32(sample, false, 24),
        SampleType::PCM_F32_MSB => f32::from_be_bytes([sample[0], sample[1], sample[2], sample[3]]),
        SampleType::PCM_F32_LSB => f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
        SampleType::PCM_F64_MSB => f64::from_be_bytes([
            sample[0], sample[1], sample[2], sample[3], sample[4], sample[5], sample[6], sample[7],
        ]) as f32,
        SampleType::PCM_F64_LSB => f64::from_le_bytes([
            sample[0], sample[1], sample[2], sample[3], sample[4], sample[5], sample[6], sample[7],
        ]) as f32,
        _ => 0.0,
    };
    if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Zweck: Dekodiert vorzeichenbehaftetes 24-Bit-PCM aus einem gepackten Big- oder Little-Endian-Buffer.
/// Eingaben: Drei Bytes und Endianness-Schalter.
/// Ausgabe: Auf ±1 normalisiertes Float-Sample.
fn decode_i24(bytes: &[u8], big_endian: bool) -> f32 {
    let raw = if big_endian {
        (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2])
    } else {
        (u32::from(bytes[2]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[0])
    };
    let signed = if raw & 0x0080_0000 != 0 {
        (raw | 0xFF00_0000) as i32
    } else {
        raw as i32
    };
    (signed as f64 / 8_388_608.0) as f32
}

/// Zweck: Dekodiert linksbündig ausgerichtetes Signed-PCM in einem 32-Bit-ASIO-Container.
/// Eingaben: Vier Containerbytes, Endianness und gültige Präzision von 16, 18, 20 oder 24 Bit.
/// Ausgabe: Auf den Float-Normalbereich begrenzter und skalierter Samplewert.
fn decode_aligned_i32(bytes: &[u8], big_endian: bool, valid_bits: u32) -> f32 {
    let raw = if big_endian {
        i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    } else {
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    };
    let shift = 32u32.saturating_sub(valid_bits);
    let sample = raw >> shift;
    (sample as f64 / 2_f64.powi(valid_bits.saturating_sub(1) as i32)) as f32
}

/// Zweck: Meldet einen Double-Bufferwechsel ohne Sperre, Allokation oder Audioverarbeitung an den Captureworker.
/// Eingaben: ASIO-Bufferindex und Treiberflag; `direct_process` wird bewusst nicht für DSP verwendet.
/// Ausgabe: Keine; bei voller Queue wird der begrenzte Überlastzähler erhöht.
unsafe extern "system" fn asio_buffer_switch(buffer_index: c_long, _direct_process: sys::Bool) {
    notify_buffer_worker(buffer_index);
}

/// Zweck: Schreibt einen gültigen Double-Bufferindex in die begrenzte lockfreie Captureworker-Queue.
/// Eingaben: ASIO-Bufferindex von einem der Treibercallbacks.
/// Ausgabe: Keine; ungültige Indizes werden verworfen und Queueüberlast wird gezählt.
fn notify_buffer_worker(buffer_index: c_long) {
    if !(0..=1).contains(&buffer_index) {
        return;
    }
    if let Some((sender, _)) = BUFFER_NOTIFICATIONS.get() {
        if sender.try_send(buffer_index).is_err() {
            DROPPED_CALLBACK_NOTIFICATIONS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Zweck: Markiert eine vom Treiber gemeldete Sampleratenänderung für den kontrollierten Capture-Abbruch.
/// Eingaben: Neue ASIO-Samplerate; die Verarbeitung wird bewusst in den Worker verlagert.
/// Ausgabe: Keine; setzt ein atomares Signal für den Capturethread.
unsafe extern "system" fn asio_sample_rate_did_change(_sample_rate: sys::SampleRate) {
    SAMPLE_RATE_CHANGED.store(true, Ordering::Release);
}

/// Zweck: Lehnt optionale ASIO-Hostnachrichten ab, die diese Eingabeintegration nicht implementiert.
/// Eingaben: Selector und ASIO-Parameter, die für die hier unterstützten Capturefunktionen ungenutzt sind.
/// Ausgabe: ASIO-FALSE gemäß Callbackvertrag.
unsafe extern "system" fn asio_message(
    _selector: sys::MessageSelector,
    _value: c_long,
    _message: *const c_void,
    _opt: *const f64,
) -> c_long {
    sys::Bool::FALSE.0
}

/// Zweck: Meldet auch beim ASIO-Timingcallback den aktiven Buffer und erhält den Treiberzeitzeiger.
/// Eingaben: ASIO-Timezeiger, Double-Bufferindex und Direct-Process-Flag.
/// Ausgabe: Derselbe Timezeiger; Timinginformationen selbst werden nicht ausgewertet.
unsafe extern "system" fn asio_buffer_switch_time_info(
    time: *mut sys::Time,
    buffer_index: c_long,
    _direct_process: sys::Bool,
) -> *mut sys::Time {
    notify_buffer_worker(buffer_index);
    time
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft die Konvertierung gepackter 24-Bit-Werte in beiden Byteordnungen.
    /// Eingaben: Positive 24-Bit-Maximalwerte und negative Vollaussteuerung.
    /// Ausgabe: Keine; Assertions sichern Vorzeichen, Endianness und Begrenzungsbereich.
    #[test]
    fn converts_packed_24_bit_pcm_in_both_endiannesses() {
        assert!((decode_i24(&[0x7f, 0xff, 0xff], true) - 0.999_999_9).abs() < 0.000_001);
        assert!((decode_i24(&[0xff, 0xff, 0x7f], false) - 0.999_999_9).abs() < 0.000_001);
        assert_eq!(decode_i24(&[0x80, 0, 0], true), -1.0);
        assert_eq!(decode_i24(&[0, 0, 0x80], false), -1.0);
    }

    /// Zweck: Prüft ASIO-Float- und ausgerichtete 24-Bit-PCM-Samples ohne Gerätezugriff.
    /// Eingaben: Little-Endian Float32 und zwei 32-Bit-Container mit 24-Bit-Präzision.
    /// Ausgabe: Keine; Assertions sichern Formatdekodierung und Normierung.
    #[test]
    fn decodes_float_and_aligned_integer_samples() {
        assert_eq!(
            decode_sample(&0.25f32.to_le_bytes(), 0, SampleType::PCM_F32_LSB),
            0.25
        );
        assert!(
            (decode_sample(&0x4000_0000i32.to_le_bytes(), 0, SampleType::PCM_I32_LSB_24) - 0.5)
                .abs()
                < 0.000_001
        );
        assert!(
            (decode_sample(&0xC000_0000u32.to_le_bytes(), 0, SampleType::PCM_I32_LSB_24) + 0.5)
                .abs()
                < 0.000_001
        );
    }
}
