/*
===============================================================================
Division By Zeros (DBZS) - Rust-Core-Audio
Modul: audio
Zweck: Sichere WAV-/MP3-Ein-/Ausgabe, Framevalidierung, Pegelmessung und
       atomarer Single-Producer/Single-Consumer-Ringbuffer.
Sprache: Rust
Hinweise:
- Der Audio-Schreibpfad verwendet atomare Slots, niemals eine UI-Sperre.
- Überläufe verwerfen alte Samples nachvollziehbar zugunsten aktueller Live-Audioframes.
===============================================================================
*/
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const MAX_WAV_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MP3_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MP3_SAMPLES: usize = 512 * 1024 * 1024;

/// Audiofehler beim Dekodieren, Erzeugen oder Puffern von Samples.
#[derive(Debug)]
pub enum AudioError {
    /// Dateisystemfehler beim Lesen oder Schreiben einer Audiodatei.
    Io(std::io::Error),
    /// RIFF-/WAVE-Struktur ist ungültig, unvollständig oder nicht unterstützt.
    InvalidWav(String),
    /// MP3-Datenstrom ist ungültig, unvollständig oder nicht unterstützt.
    InvalidMp3(String),
    /// Anzahl Kanäle, Sample-Rate oder Frameparameter sind ungültig.
    InvalidFormat(String),
}

impl Display for AudioError {
    /// Zweck: Formuliert einen Audiofehler als verständliche deutsche Meldung.
    /// Eingaben: Fehlerwert und Formatierungsziel.
    /// Ausgabe: `Ok(())` nach erfolgreicher Fehlertextausgabe.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "Audiodateizugriff fehlgeschlagen: {error}"),
            Self::InvalidWav(reason) => write!(f, "WAV-Datei ungültig: {reason}"),
            Self::InvalidMp3(reason) => write!(f, "MP3-Datei ungültig: {reason}"),
            Self::InvalidFormat(reason) => write!(f, "Audioformat ungültig: {reason}"),
        }
    }
}
impl std::error::Error for AudioError {}

/// Metadaten des dekodierten PCM-Audiostroms aus WAV oder MP3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub is_float: bool,
}

/// Dekodierte WAV-/MP3-Audiodaten; Samples liegen interleaved und im normierten Bereich vor.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveFile {
    pub format: AudioFormat,
    pub samples: Vec<f32>,
}

impl WaveFile {
    /// Zweck: Liefert die Anzahl vollständiger Sample-Frames.
    /// Eingaben: `self` – dekodierte Audioquelle mit interleaved Kanalwerten.
    /// Ausgabe: Zahl der Zeitpunkte, jeweils über alle Kanäle.
    pub fn frame_count(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels)
    }

    /// Zweck: Berechnet die Audiodauer anhand der Sample-Rate und vollständigen Frames.
    /// Eingaben: `self` – dekodierte WAV- oder MP3-Audiodaten.
    /// Ausgabe: Dauer in Sekunden oder `0.0`, falls die Sample-Rate ungültig wäre.
    pub fn duration_seconds(&self) -> f64 {
        if self.format.sample_rate == 0 {
            0.0
        } else {
            self.frame_count() as f64 / f64::from(self.format.sample_rate)
        }
    }

    /// Zweck: Leitet ein Mono-Analyseriesignal durch Mittelung der Kanäle ab.
    /// Eingaben: `self` – interleaved WAV-/MP3-Samples.
    /// Ausgabe: Endliche Mono-Samplefolge; nichtendliche Eingaben werden als Stille behandelt.
    pub fn mono_samples(&self) -> Vec<f32> {
        let channels = usize::from(self.format.channels);
        self.samples
            .chunks_exact(channels)
            .map(|frame| {
                frame
                    .iter()
                    .copied()
                    .filter(|sample| sample.is_finite())
                    .sum::<f32>()
                    / channels as f32
            })
            .collect()
    }
}

/// Ein validierter Analyseframe mit Sampleposition und Grundpegel.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub start_sample: u64,
    pub samples: Vec<f32>,
    pub rms: f32,
    pub peak: f32,
}

impl AudioFrame {
    /// Zweck: Erzeugt einen mono Analyseframe und berechnet RMS-/Peak-Pegel.
    /// Eingaben: Sample-Rate größer null, Startsample und endliche oder zu Null bereinigte Samples.
    /// Ausgabe: Validierter Frame oder `InvalidFormat` bei ungültiger Sample-Rate/leerem Fenster.
    pub fn new(sample_rate: u32, start_sample: u64, samples: Vec<f32>) -> Result<Self, AudioError> {
        if sample_rate == 0 || samples.is_empty() {
            return Err(AudioError::InvalidFormat(
                "Sample-Rate und Frameinhalt müssen positiv sein.".into(),
            ));
        }
        let cleaned = samples
            .into_iter()
            .map(|value| {
                if value.is_finite() {
                    value.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        let sum = cleaned.iter().map(|value| value * value).sum::<f32>();
        let rms = (sum / cleaned.len() as f32).sqrt();
        let peak = cleaned.iter().map(|value| value.abs()).fold(0.0, f32::max);
        Ok(Self {
            sample_rate,
            start_sample,
            samples: cleaned,
            rms,
            peak,
        })
    }
}

/// Thread-sicherer Ringpuffer mit einem Capture-Schreiber und einem Analyse-Leser.
#[derive(Debug)]
pub struct AudioRingBuffer {
    slots: Arc<[AtomicU32]>,
    write_index: AtomicU64,
    read_index: AtomicU64,
    overruns: AtomicU64,
}

impl AudioRingBuffer {
    /// Zweck: Erstellt einen Ringpuffer für mindestens einen Mono-Sample.
    /// Eingaben: `capacity_samples` – maximale Zahl atomarer Sampleplätze.
    /// Ausgabe: Neuer leerer SPSC-Puffer; eine Kapazität null wird auf eins angehoben.
    pub fn new(capacity_samples: usize) -> Self {
        let slots = (0..capacity_samples.max(1))
            .map(|_| AtomicU32::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots: Arc::from(slots),
            write_index: AtomicU64::new(0),
            read_index: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
        }
    }

    /// Zweck: Schreibt interleaved Kanäle als Mono in den Ringpuffer, ohne auf den Leser zu warten.
    /// Eingaben: `interleaved` – Frames mit alternierenden Kanälen; `channels` – Kanalzahl größer null.
    /// Ausgabe: Anzahl geschriebener Mono-Frames oder `InvalidFormat`; Überlaufzähler bleibt abrufbar.
    pub fn push_interleaved(
        &self,
        interleaved: &[f32],
        channels: u16,
    ) -> Result<usize, AudioError> {
        let channel_count = usize::from(channels);
        if channel_count == 0 || interleaved.len() % channel_count != 0 {
            return Err(AudioError::InvalidFormat(
                "Kanalzahl oder interleaved Sampleanzahl ist ungültig.".into(),
            ));
        }
        let capacity = self.slots.len() as u64;
        let first = self.write_index.load(Ordering::Relaxed);
        let mut cursor = first;
        for frame in interleaved.chunks_exact(channel_count) {
            let mono = frame
                .iter()
                .copied()
                .filter(|sample| sample.is_finite())
                .sum::<f32>()
                / channel_count as f32;
            self.slots[(cursor % capacity) as usize]
                .store(mono.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
            cursor = cursor.wrapping_add(1);
        }
        self.write_index.store(cursor, Ordering::Release);
        let minimum_read = cursor.saturating_sub(capacity);
        let observed_read = self.read_index.load(Ordering::Relaxed);
        if observed_read < minimum_read {
            self.overruns
                .fetch_add(minimum_read - observed_read, Ordering::Relaxed);
            self.read_index.store(minimum_read, Ordering::Release);
        }
        Ok(interleaved.len() / channel_count)
    }

    /// Zweck: Entnimmt bis zu `maximum` aktuell verfügbare Samples ohne den Schreiber zu blockieren.
    /// Eingaben: `maximum` – gewünschte Anzahl Mono-Samples.
    /// Ausgabe: Chronologische Samples, wobei überholte Werte übersprungen werden.
    pub fn read_available(&self, maximum: usize) -> Vec<f32> {
        if maximum == 0 {
            return Vec::new();
        }
        let capacity = self.slots.len() as u64;
        let published = self.write_index.load(Ordering::Acquire);
        let mut read = self.read_index.load(Ordering::Relaxed);
        let earliest = published.saturating_sub(capacity);
        if read < earliest {
            self.overruns.fetch_add(earliest - read, Ordering::Relaxed);
            read = earliest;
        }
        let count = (published.saturating_sub(read) as usize).min(maximum);
        let mut samples = Vec::with_capacity(count);
        for index in read..read + count as u64 {
            samples.push(f32::from_bits(
                self.slots[(index % capacity) as usize].load(Ordering::Acquire),
            ));
        }
        self.read_index
            .store(read + count as u64, Ordering::Release);
        samples
    }

    /// Zweck: Liefert die Anzahl durch Überlauf verworfener Samples.
    /// Eingaben: `self` – Ringpuffer.
    /// Ausgabe: Monoton steigender Überlaufzähler.
    pub fn overrun_samples(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// Zweck: Liefert die aktuell publizierte Schreibposition.
    /// Eingaben: `self` – Ringpuffer.
    /// Ausgabe: Monotone Zahl geschriebener Sampleframes.
    pub fn samples_written(&self) -> u64 {
        self.write_index.load(Ordering::Acquire)
    }
}

/// Zweck: Dekodiert eine RIFF/WAVE-Datei mit PCM 8/16/24/32 Bit oder IEEE-float32.
/// Eingaben: `path` – lokaler Dateipfad; Dateigröße wird vor dem Laden begrenzt.
/// Ausgabe: Dekodierte interleaved Samples und Audioformat oder ein lesbarer WAV-/I/O-Fehler.
pub fn read_wav(path: impl AsRef<Path>) -> Result<WaveFile, AudioError> {
    read_wav_with_progress(path.as_ref(), &mut |_| {})
}

/// Zweck: Dekodiert WAV und meldet Fortschritt anhand tatsächlich gelesener Bytes und konvertierter Samples.
/// Eingaben: `path` – lokale WAV-Datei; `progress` – Callback mit monotonem Anteil von null bis eins.
/// Ausgabe: Dekodierte interleaved Samples und Audioformat oder ein lesbarer WAV-/I/O-Fehler.
fn read_wav_with_progress<F>(path: &Path, progress: &mut F) -> Result<WaveFile, AudioError>
where
    F: FnMut(f32),
{
    let metadata = fs::metadata(path).map_err(AudioError::Io)?;
    if metadata.len() == 0 || metadata.len() > MAX_WAV_BYTES {
        return Err(AudioError::InvalidWav(
            "Datei ist leer oder überschreitet die 2-GiB-Grenze.".into(),
        ));
    }
    let total_bytes = metadata.len() as usize;
    let mut file = File::open(path).map_err(AudioError::Io)?;
    let mut bytes = Vec::with_capacity(total_bytes);
    let mut read_buffer = [0_u8; 64 * 1024];
    let mut bytes_read = 0_usize;
    loop {
        let count = file.read(&mut read_buffer).map_err(AudioError::Io)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&read_buffer[..count]);
        bytes_read += count;
        progress(0.4 * bytes_read.min(total_bytes) as f32 / total_bytes as f32);
    }
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(AudioError::InvalidWav("RIFF/WAVE-Kopf fehlt.".into()));
    }
    let mut offset = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let id = &bytes[offset..offset + 4];
        let length = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start
            .checked_add(length)
            .ok_or_else(|| AudioError::InvalidWav("Chunklänge läuft über.".into()))?;
        if end > bytes.len() {
            return Err(AudioError::InvalidWav(
                "Chunk endet außerhalb der Datei.".into(),
            ));
        }
        if id == b"fmt " {
            if length < 16 {
                return Err(AudioError::InvalidWav(
                    "Format-Chunk ist kürzer als 16 Byte.".into(),
                ));
            }
            let chunk = &bytes[start..end];
            fmt = Some((
                u16::from_le_bytes([chunk[0], chunk[1]]),
                u16::from_le_bytes([chunk[2], chunk[3]]),
                u32::from_le_bytes(chunk[4..8].try_into().unwrap()),
                u16::from_le_bytes([chunk[12], chunk[13]]),
                u16::from_le_bytes([chunk[14], chunk[15]]),
            ));
        } else if id == b"data" {
            data = Some(&bytes[start..end]);
        }
        offset = end + (length & 1);
    }
    let (encoding, channels, sample_rate, block_align, bits) =
        fmt.ok_or_else(|| AudioError::InvalidWav("Format-Chunk fehlt.".into()))?;
    if !matches!(encoding, 1 | 3)
        || channels == 0
        || channels > 8
        || !(8_000..=192_000).contains(&sample_rate)
        || !matches!(bits, 8 | 16 | 24 | 32)
        || (encoding == 3 && bits != 32)
    {
        return Err(AudioError::InvalidWav(format!(
            "Nicht unterstütztes PCM-Format: encoding={encoding}, channels={channels}, rate={sample_rate}, bits={bits}."
        )));
    }
    let bytes_per_sample = usize::from(bits / 8);
    if usize::from(block_align) < bytes_per_sample * usize::from(channels) {
        return Err(AudioError::InvalidWav(
            "Blockausrichtung ist kleiner als die Kanal-Samples.".into(),
        ));
    }
    let data = data.ok_or_else(|| AudioError::InvalidWav("Data-Chunk fehlt.".into()))?;
    if data.len() % usize::from(block_align) != 0 {
        return Err(AudioError::InvalidWav(
            "Data-Chunk enthält einen unvollständigen Frame.".into(),
        ));
    }
    let mut samples = Vec::with_capacity(data.len() / bytes_per_sample);
    let total_samples = data.len() / bytes_per_sample;
    for (index, sample) in data.chunks_exact(bytes_per_sample).enumerate() {
        let value = match (encoding, bits) {
            (1, 8) => (f32::from(sample[0]) - 128.0) / 128.0,
            (1, 16) => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0,
            (1, 24) => {
                let raw = i32::from(sample[0])
                    | (i32::from(sample[1]) << 8)
                    | (i32::from(sample[2]) << 16);
                let signed = (raw << 8) >> 8;
                signed as f32 / 8_388_608.0
            }
            (1, 32) => i32::from_le_bytes(sample.try_into().unwrap()) as f32 / 2_147_483_648.0,
            (3, 32) => f32::from_le_bytes(sample.try_into().unwrap()),
            _ => unreachable!(),
        };
        samples.push(if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        });
        if index % 16_384 == 0 {
            progress(0.4 + 0.6 * (index + 1) as f32 / total_samples as f32);
        }
    }
    progress(1.0);
    Ok(WaveFile {
        format: AudioFormat {
            sample_rate,
            channels,
            bits_per_sample: bits,
            is_float: encoding == 3,
        },
        samples,
    })
}

/// Zweck: Öffnet eine lokale WAV- oder MP3-Datei über den einheitlichen Medienpfad.
/// Eingaben: `path` – Datei mit `.wav`- oder `.mp3`-Endung; die Erkennung ignoriert Groß-/Kleinschreibung.
/// Ausgabe: Interleaved, normalisierte Samples und Quellmetadaten oder ein formatbezogener Datei-/Dekodierfehler.
pub fn read_audio(path: impl AsRef<Path>) -> Result<WaveFile, AudioError> {
    read_audio_with_progress(path, |_| {})
}

/// Zweck: Dekodiert WAV oder MP3 und meldet echten Fortschritt des Datei-/Samplepfads.
/// Eingaben: `path` – lokale WAV-/MP3-Datei; `progress` – Callback mit Anteil von null bis eins.
/// Ausgabe: Interleaved Samples und Metadaten oder ein formatbezogener Datei-/Decoderfehler.
pub fn read_audio_with_progress<F>(
    path: impl AsRef<Path>,
    mut progress: F,
) -> Result<WaveFile, AudioError>
where
    F: FnMut(f32),
{
    let path = path.as_ref();
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("wav") => {
            read_wav_with_progress(path, &mut progress)
        }
        Some(extension) if extension.eq_ignore_ascii_case("mp3") => {
            read_mp3_with_progress(path, &mut progress)
        }
        Some(extension) => Err(AudioError::InvalidFormat(format!(
            "Dateiendung .{extension} wird nicht als Audiodatei unterstützt; erlaubt sind WAV und MP3."
        ))),
        None => Err(AudioError::InvalidFormat(
            "Audiodatei besitzt keine unterstützte .wav- oder .mp3-Endung.".into(),
        )),
    }
}

/// Zweck: Dekodiert einen MP3-Datenstrom mit Symphonia in interleaved Float-Samples.
/// Eingaben: `path` – lokale MP3-Datei bis 2 GiB; `progress` – Fortschritt aus dekodierten Paketbytes.
/// Ausgabe: `WaveFile` mit MP3-Quellrate und Kanalzahl oder ein verständlicher Datei-/Format-/Decoderfehler.
fn read_mp3_with_progress<F>(path: &Path, progress: &mut F) -> Result<WaveFile, AudioError>
where
    F: FnMut(f32),
{
    let metadata = fs::metadata(path).map_err(AudioError::Io)?;
    if metadata.len() == 0 || metadata.len() > MAX_MP3_BYTES {
        return Err(AudioError::InvalidMp3(
            "Datei ist leer oder überschreitet die 2-GiB-Grenze.".into(),
        ));
    }
    let source = File::open(path).map_err(AudioError::Io)?;
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let format_options = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &format_options, &MetadataOptions::default())
        .map_err(|error| {
            AudioError::InvalidMp3(format!("Format konnte nicht erkannt werden: {error}"))
        })?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| AudioError::InvalidMp3("Kein dekodierbarer Audiotrack gefunden.".into()))?;
    let track_id = track.id;
    let codec_parameters = track.codec_params.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_parameters, &DecoderOptions::default())
        .map_err(|error| {
            AudioError::InvalidMp3(format!("MP3-Decoder konnte nicht erstellt werden: {error}"))
        })?;

    let mut samples = Vec::new();
    let mut sample_rate = None;
    let mut channels = None;
    let mut decoded_packet_bytes = 0_u64;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(error) => {
                return Err(AudioError::InvalidMp3(format!(
                    "MP3-Paket konnte nicht gelesen werden: {error}"
                )));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder.decode(&packet).map_err(|error| {
            AudioError::InvalidMp3(format!("MP3-Paket konnte nicht dekodiert werden: {error}"))
        })?;
        let rate = decoded.spec().rate;
        let channel_count = decoded.spec().channels.count();
        if !(8_000..=192_000).contains(&rate) || channel_count == 0 || channel_count > 8 {
            return Err(AudioError::InvalidMp3(format!(
                "Nicht unterstütztes PCM-Ausgabeformat: {rate} Hz, {channel_count} Kanäle."
            )));
        }
        let channel_count = channel_count as u16;
        if sample_rate.is_some_and(|previous| previous != rate)
            || channels.is_some_and(|previous| previous != channel_count)
        {
            return Err(AudioError::InvalidMp3(
                "Formatwechsel innerhalb eines MP3-Datenstroms wird nicht unterstützt.".into(),
            ));
        }
        sample_rate = Some(rate);
        channels = Some(channel_count);

        let mut decoded_samples =
            SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        decoded_samples.copy_interleaved_ref(decoded);
        let decoded_samples = decoded_samples.samples();
        if decoded_samples.len() > MAX_MP3_SAMPLES.saturating_sub(samples.len()) {
            return Err(AudioError::InvalidMp3(
                "Dekodierte Audioausgabe überschreitet die 2-GiB-Sicherheitsgrenze.".into(),
            ));
        }
        samples.extend(decoded_samples.iter().map(|sample| {
            if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        }));
        decoded_packet_bytes = decoded_packet_bytes.saturating_add(packet.buf().len() as u64);
        progress(
            (decoded_packet_bytes.min(metadata.len()) as f32 / metadata.len() as f32)
                .clamp(0.0, 1.0),
        );
    }

    let (sample_rate, channels) = match (sample_rate, channels) {
        (Some(sample_rate), Some(channels)) => (sample_rate, channels),
        _ => {
            return Err(AudioError::InvalidMp3(
                "MP3 enthält keine dekodierbaren Audioframes.".into(),
            ));
        }
    };
    if samples.is_empty() || samples.len() % usize::from(channels) != 0 {
        return Err(AudioError::InvalidMp3(
            "Dekodierte MP3-Samples enthalten keinen vollständigen Audioframe.".into(),
        ));
    }
    progress(1.0);
    Ok(WaveFile {
        format: AudioFormat {
            sample_rate,
            channels,
            bits_per_sample: 32,
            is_float: true,
        },
        samples,
    })
}

/// Zweck: Schreibt mono oder mehrkanalige Samples als PCM-16-Bit-WAV.
/// Eingaben: `path` – Ziel; `samples` – interleaved Samples; `sample_rate` – Hz; `channels` – Kanalzahl.
/// Ausgabe: `Ok(())` nach vollständigem RIFF/WAVE-Schreiben oder ein I/O-/Formatfehler.
pub fn write_wav(
    path: impl AsRef<Path>,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), AudioError> {
    write_wav_with_progress(path, samples, sample_rate, channels, |_| {})
}

/// Zweck: Schreibt PCM-16-Bit-WAV in Datenblöcken und meldet tatsächlich geschriebene Samplebytes.
/// Eingaben: Zielpfad, interleaved Samples, Sample-Rate, Kanalzahl und Fortschrittscallback.
/// Ausgabe: `Ok(())` nach vollständigem RIFF/WAVE-Schreiben oder ein I/O-/Formatfehler.
pub fn write_wav_with_progress<F>(
    path: impl AsRef<Path>,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    mut progress: F,
) -> Result<(), AudioError>
where
    F: FnMut(f32),
{
    if sample_rate == 0
        || channels == 0
        || channels > 8
        || samples.len() % usize::from(channels) != 0
    {
        return Err(AudioError::InvalidFormat(
            "Sample-Rate, Kanalzahl oder Sampleanzahl ist ungültig.".into(),
        ));
    }
    let data_bytes = samples
        .len()
        .checked_mul(2)
        .ok_or_else(|| AudioError::InvalidFormat("WAV-Datenlänge läuft über.".into()))?;
    let riff_size = 36usize
        .checked_add(data_bytes)
        .filter(|size| *size <= u32::MAX as usize)
        .ok_or_else(|| {
            AudioError::InvalidFormat("PCM-WAV überschreitet das RIFF-32-Bit-Limit.".into())
        })?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(channels) * 2)
        .ok_or_else(|| AudioError::InvalidFormat("Byte-Rate läuft über.".into()))?;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(riff_size as u32).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&channels.to_le_bytes());
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&(channels * 2).to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(data_bytes as u32).to_le_bytes());

    let mut file = File::create(path).map_err(AudioError::Io)?;
    file.write_all(&header).map_err(AudioError::Io)?;
    for (index, chunk) in samples.chunks(8_192).enumerate() {
        let mut bytes = Vec::with_capacity(chunk.len() * 2);
        for value in chunk {
            let clean = if value.is_finite() {
                value.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let pcm = (clean * 32767.0).round() as i16;
            bytes.extend_from_slice(&pcm.to_le_bytes());
        }
        file.write_all(&bytes).map_err(AudioError::Io)?;
        let samples_written = ((index + 1) * 8_192).min(samples.len());
        progress(samples_written as f32 / samples.len().max(1) as f32);
    }
    file.flush().map_err(AudioError::Io)?;
    if samples.is_empty() {
        progress(1.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft PCM-WAV-Schreiben, erneutes Dekodieren und Stereo-Monodownmix.
    /// Eingaben: Keine; erzeugt eine temporäre kurze Stereo-WAV-Datei.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Kanalzahl, Dauer und Samples.
    #[test]
    fn wav_round_trip_preserves_stereo_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.wav");
        let source = [0.5, -0.5, 0.25, -0.25];
        write_wav(&path, &source, 48_000, 2).unwrap();
        let mut progress = Vec::new();
        let decoded = read_audio_with_progress(&path, |value| progress.push(value)).unwrap();
        assert_eq!(decoded.format.channels, 2);
        assert_eq!(decoded.frame_count(), 2);
        assert!((decoded.mono_samples()[0]).abs() < 0.001);
        assert!((decoded.duration_seconds() - 2.0 / 48_000.0).abs() < 1e-9);
        assert!(
            progress.windows(2).all(|values| values[0] <= values[1]),
            "WAV-Fortschritt ist nicht monoton: {progress:?}"
        );
        assert_eq!(progress.last(), Some(&1.0));
    }

    /// Zweck: Prüft den Ringpuffer-Downmix und das nachvollziehbare Verwerfen alter Samples.
    /// Eingaben: Keine; schreibt mehr Frames als die Testkapazität zulässt.
    /// Ausgabe: Keine Rückgabe; Assertions prüfen jüngste Reihenfolge und Überlaufzähler.
    #[test]
    fn ring_buffer_keeps_recent_samples_and_counts_overruns() {
        let buffer = AudioRingBuffer::new(3);
        buffer
            .push_interleaved(&[1.0, -1.0, 0.25, 0.25, 0.5, 0.5, -0.5, -0.5], 2)
            .unwrap();
        assert_eq!(buffer.read_available(8), vec![0.25, 0.5, -0.5]);
        assert!(buffer.overrun_samples() >= 1);
    }

    /// Zweck: Stellt sicher, dass ungültige Kanäle und nicht vollständige Frames klar abgewiesen werden.
    /// Eingaben: Keine; testet null Kanäle, fehlerhafte Interleaving-Länge und leere Analyseframes.
    /// Ausgabe: Keine Rückgabe; Assertions erwarten jeweils einen Fehler.
    #[test]
    fn rejects_invalid_audio_shapes() {
        let buffer = AudioRingBuffer::new(8);
        assert!(buffer.push_interleaved(&[0.1], 0).is_err());
        assert!(buffer.push_interleaved(&[0.1], 2).is_err());
        assert!(AudioFrame::new(0, 0, vec![0.1]).is_err());
        assert!(AudioFrame::new(48_000, 0, Vec::new()).is_err());
    }
}
