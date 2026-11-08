/*
===============================================================================
Division By Zeros (DBZS) - Rust-Core-Audio
Modul: audio
Zweck: Sichere WAV-Ein-/Ausgabe, Framevalidierung, Pegelmessung und
       atomarer Single-Producer/Single-Consumer-Ringbuffer.
Sprache: Rust
Hinweise:
- Der Audio-Schreibpfad verwendet atomare Slots, niemals eine UI-Sperre.
- Überläufe verwerfen alte Samples nachvollziehbar zugunsten aktueller Live-Audioframes.
===============================================================================
*/
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const MAX_WAV_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Audiofehler beim Dekodieren, Erzeugen oder Puffern von Samples.
#[derive(Debug)]
pub enum AudioError {
    /// Dateisystemfehler beim Lesen oder Schreiben einer Audiodatei.
    Io(std::io::Error),
    /// RIFF-/WAVE-Struktur ist ungültig, unvollständig oder nicht unterstützt.
    InvalidWav(String),
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
            Self::InvalidFormat(reason) => write!(f, "Audioformat ungültig: {reason}"),
        }
    }
}
impl std::error::Error for AudioError {}

/// Metadaten eines PCM- oder Float-WAV-Audiostroms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub is_float: bool,
}

/// Dekodierte WAV-Daten; Samples liegen interleaved und im normierten Bereich vor.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveFile {
    pub format: AudioFormat,
    pub samples: Vec<f32>,
}

impl WaveFile {
    /// Zweck: Liefert die Anzahl vollständiger Sample-Frames.
    /// Eingaben: `self` – WAV-Daten mit interleaved Kanalwerten.
    /// Ausgabe: Zahl der Zeitpunkte, jeweils über alle Kanäle.
    pub fn frame_count(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels)
    }

    /// Zweck: Berechnet die Audiodauer anhand der Sample-Rate und vollständigen Frames.
    /// Eingaben: `self` – WAV-Daten.
    /// Ausgabe: Dauer in Sekunden oder `0.0`, falls die Sample-Rate ungültig wäre.
    pub fn duration_seconds(&self) -> f64 {
        if self.format.sample_rate == 0 {
            0.0
        } else {
            self.frame_count() as f64 / f64::from(self.format.sample_rate)
        }
    }

    /// Zweck: Leitet ein Mono-Analyseriesignal durch Mittelung der Kanäle ab.
    /// Eingaben: `self` – interleaved WAV-Samples.
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
    let metadata = fs::metadata(path.as_ref()).map_err(AudioError::Io)?;
    if metadata.len() == 0 || metadata.len() > MAX_WAV_BYTES {
        return Err(AudioError::InvalidWav(
            "Datei ist leer oder überschreitet die 2-GiB-Grenze.".into(),
        ));
    }
    let bytes = fs::read(path).map_err(AudioError::Io)?;
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
    for sample in data.chunks_exact(bytes_per_sample) {
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
    }
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

/// Zweck: Schreibt mono oder mehrkanalige Samples als PCM-16-Bit-WAV.
/// Eingaben: `path` – Ziel; `samples` – interleaved Samples; `sample_rate` – Hz; `channels` – Kanalzahl.
/// Ausgabe: `Ok(())` nach vollständigem RIFF/WAVE-Schreiben oder ein I/O-/Formatfehler.
pub fn write_wav(
    path: impl AsRef<Path>,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), AudioError> {
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
    let mut bytes = Vec::with_capacity(riff_size + 8);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(riff_size as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for value in samples {
        let clean = if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let pcm = (clean * 32767.0).round() as i16;
        bytes.extend_from_slice(&pcm.to_le_bytes());
    }
    fs::write(path, bytes).map_err(AudioError::Io)
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
        let decoded = read_wav(&path).unwrap();
        assert_eq!(decoded.format.channels, 2);
        assert_eq!(decoded.frame_count(), 2);
        assert!((decoded.mono_samples()[0]).abs() < 0.001);
        assert!((decoded.duration_seconds() - 2.0 / 48_000.0).abs() < 1e-9);
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
