/*
===============================================================================
Division By Zeros (DBZS) - Rust-DSP-Features
Modul: dsp
Zweck: Chroma, monophone Pitch-Schätzung, RMS-/Peak-Pegel und kompakte
       Waveformdarstellung für Live- und Offlineanalyse.
Sprache: Rust
Hinweise:
- Dieser deterministische CPU-DSP-Pfad ist unabhängig von llama.cpp und ML-Modellen.
- Er bereinigt nichtendliche Samples, bevor sie in musikalische Features eingehen.
===============================================================================
*/
use crate::domain::NoteName;

/// RMS- und Peak-Pegel eines endlichen mono Audiofensters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub rms: f32,
    pub peak: f32,
}

/// Einzelne monophone Pitch-Schätzung mit MIDI-Tonhöhe und Konfidenz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchEstimate {
    pub midi: u8,
    pub frequency_hz: f32,
    pub confidence: f32,
}

/// Zweck: Berechnet Chromaenergie für die zwölf chromatischen Tonklassen.
/// Eingaben: Mono-Samples und Sample-Rate; zu kurze oder stille Frames werden als Null-Chroma behandelt.
/// Ausgabe: Normierter 12-Bin-Vektor mit Wertebereich 0 bis 1.
pub fn extract_chroma(samples: &[f32], sample_rate: u32) -> [f32; 12] {
    let mut chroma = [0.0f32; 12];
    if sample_rate == 0 || samples.len() < 64 {
        return chroma;
    }
    let mut windowed = Vec::with_capacity(samples.len());
    let denominator = (samples.len().saturating_sub(1)).max(1) as f32;
    for (index, sample) in samples.iter().enumerate() {
        let hann = 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / denominator).cos();
        windowed.push((if sample.is_finite() { *sample } else { 0.0 }) * hann);
    }
    for midi in 36u8..=88 {
        let frequency = 440.0 * 2.0f32.powf((f32::from(midi) - 69.0) / 12.0);
        if frequency >= sample_rate as f32 * 0.48 {
            continue;
        }
        let power = goertzel_power(&windowed, sample_rate, frequency);
        chroma[usize::from(midi % 12)] += power.sqrt();
    }
    let maximum = chroma.iter().copied().fold(0.0f32, f32::max);
    if maximum > 1e-12 {
        for value in &mut chroma {
            *value /= maximum;
        }
    }
    chroma
}

/// Zweck: Schätzt eine monophone Grundfrequenz mittels normalisierter Autokorrelation.
/// Eingaben: Mono-Audioframe, Sample-Rate und optionaler Frequenzbereich.
/// Ausgabe: MIDI-Tonhöhe und periodizitätsbasierte Konfidenz oder `None` bei Stille/Unklarheit.
pub fn estimate_pitch(
    samples: &[f32],
    sample_rate: u32,
    minimum_hz: f32,
    maximum_hz: f32,
) -> Option<PitchEstimate> {
    if sample_rate == 0
        || samples.len() < 128
        || !minimum_hz.is_finite()
        || !maximum_hz.is_finite()
        || minimum_hz <= 0.0
        || maximum_hz <= minimum_hz
    {
        return None;
    }
    let finite = samples
        .iter()
        .map(|value| if value.is_finite() { *value } else { 0.0 })
        .collect::<Vec<_>>();
    let energy = finite.iter().map(|value| value * value).sum::<f32>();
    if energy < 1e-7 {
        return None;
    }
    let min_lag = ((sample_rate as f32 / maximum_hz).floor() as usize).max(2);
    let max_lag = ((sample_rate as f32 / minimum_hz).ceil() as usize).min(finite.len() / 2);
    if max_lag <= min_lag {
        return None;
    }
    let mut correlations = vec![0.0f32; max_lag + 1];
    for lag in min_lag..=max_lag {
        let count = finite.len() - lag;
        let numerator = (0..count)
            .map(|index| finite[index] * finite[index + lag])
            .sum::<f32>();
        let left = (0..count)
            .map(|index| finite[index] * finite[index])
            .sum::<f32>();
        let right = (0..count)
            .map(|index| finite[index + lag] * finite[index + lag])
            .sum::<f32>();
        let denominator = (left * right).sqrt();
        let correlation = if denominator > 1e-12 {
            numerator / denominator
        } else {
            0.0
        };
        correlations[lag] = correlation;
    }
    let highest = correlations[min_lag..=max_lag]
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let first_periodic_peak = (min_lag..=max_lag).find(|lag| {
        let correlation = correlations[*lag];
        let is_peak = (*lag == min_lag || correlation >= correlations[*lag - 1])
            && (*lag == max_lag || correlation >= correlations[*lag + 1]);
        is_peak && correlation >= 0.35 && correlation >= highest * 0.90
    });
    let best_lag = first_periodic_peak.unwrap_or_else(|| {
        (min_lag..=max_lag)
            .max_by(|left, right| correlations[*left].total_cmp(&correlations[*right]))
            .unwrap_or(0)
    });
    let confidence = correlations.get(best_lag).copied().unwrap_or_default();
    if best_lag == 0 || confidence < 0.35 {
        return None;
    }
    let frequency_hz = sample_rate as f32 / best_lag as f32;
    let midi_float = 69.0 + 12.0 * (frequency_hz / 440.0).log2();
    if !midi_float.is_finite() || !(0.0..=127.0).contains(&midi_float) {
        return None;
    }
    Some(PitchEstimate {
        midi: midi_float.round() as u8,
        frequency_hz,
        confidence: confidence.clamp(0.0, 1.0),
    })
}

/// Zweck: Berechnet RMS- und Spitzenpegel für ein Audiofenster.
/// Eingaben: Beliebige f32-Samples; nichtendliche Werte werden als Stille gezählt.
/// Ausgabe: `Level` mit RMS- und Absolutspitzenwert zwischen null und eins.
pub fn measure_level(samples: &[f32]) -> Level {
    if samples.is_empty() {
        return Level {
            rms: 0.0,
            peak: 0.0,
        };
    }
    let mut sum = 0.0;
    let mut peak: f32 = 0.0;
    for value in samples {
        let sample = if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        sum += sample * sample;
        peak = peak.max(sample.abs());
    }
    Level {
        rms: (sum / samples.len() as f32).sqrt(),
        peak,
    }
}

/// Zweck: Wandelt Messpegel in eine kompakte Unicode-Wellenform für die UI um.
/// Eingaben: Pegelwerte und gewünschte Spaltenzahl; nichtendliche Werte werden als Stille gezeigt.
/// Ausgabe: Zeichenfolge aus acht Pegelstufen, mindestens eine Spalte.
pub fn waveform_bars(levels: &[f32], columns: usize) -> String {
    const BARS: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];
    if levels.is_empty() {
        return "Keine Samples".into();
    }
    let width = columns.max(1).min(levels.len());
    (0..width)
        .map(|column| {
            let start = column * levels.len() / width;
            let end = ((column + 1) * levels.len() / width).max(start + 1);
            let peak = levels[start..end]
                .iter()
                .copied()
                .filter(|value| value.is_finite())
                .map(f32::abs)
                .fold(0.0, f32::max)
                .clamp(0.0, 1.0);
            BARS[(peak * 7.0).round() as usize]
        })
        .collect()
}

/// Zweck: Formatiert eine MIDI-Tonhöhe als kanonischen Notennamen mit wissenschaftlicher Oktave.
/// Eingaben: `midi` – gültiger MIDI-Wert von null bis 127.
/// Ausgabe: Zeichenfolge wie `E4` oder `F#3`.
pub fn midi_note_name(midi: u8) -> String {
    format!(
        "{}{}",
        NoteName::from_pitch_class(midi % 12),
        i16::from(midi) / 12 - 1
    )
}

/// Zweck: Berechnet die Leistung einer Frequenzkomponente mit dem Goertzel-Rekursionsfilter.
/// Eingaben: Fensterte Samples, Sample-Rate und Frequenz in Hertz.
/// Ausgabe: Nichtnegative Energie oder null bei ungültigen/außerhalb liegenden Frequenzen.
fn goertzel_power(samples: &[f32], sample_rate: u32, frequency_hz: f32) -> f32 {
    if samples.is_empty()
        || sample_rate == 0
        || frequency_hz <= 0.0
        || frequency_hz >= sample_rate as f32 * 0.5
    {
        return 0.0;
    }
    let omega = std::f32::consts::TAU * frequency_hz / sample_rate as f32;
    let coefficient = 2.0 * omega.cos();
    let mut previous = 0.0;
    let mut previous2 = 0.0;
    for sample in samples {
        let current = *sample + coefficient * previous - previous2;
        previous2 = previous;
        previous = current;
    }
    (previous * previous + previous2 * previous2 - coefficient * previous * previous2).max(0.0)
        / samples.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft, dass ein synthetischer A4-Ton A-Chroma und Pitch liefert.
    /// Eingaben: Keine; erzeugt 0,2 Sekunden 440-Hz-Sinus bei 44,1 kHz.
    /// Ausgabe: Keine Rückgabe; Assertions sichern dominante Tonklasse und MIDI 69.
    #[test]
    fn extracts_chroma_and_pitch_from_a4() {
        let sample_rate = 44_100;
        let samples = (0..8_820)
            .map(|index| {
                (std::f32::consts::TAU * 440.0 * index as f32 / sample_rate as f32).sin() * 0.5
            })
            .collect::<Vec<_>>();
        let chroma = extract_chroma(&samples, sample_rate);
        let dominant = chroma
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .unwrap()
            .0;
        assert_eq!(dominant, 9);
        let pitch = estimate_pitch(&samples, sample_rate, 70.0, 800.0).unwrap();
        assert_eq!(pitch.midi, 69);
        assert!(pitch.confidence > 0.9);
    }

    /// Zweck: Sichert Stillebehandlung, endliche Pegelmessung und Waveformlänge ab.
    /// Eingaben: Keine; verwendet leere, nichtendliche und gemischte Samples.
    /// Ausgabe: Keine Rückgabe; Assertions prüfen Stille und bounded UI-Ausgabe.
    #[test]
    fn handles_silence_and_non_finite_samples() {
        assert_eq!(
            measure_level(&[]),
            Level {
                rms: 0.0,
                peak: 0.0
            }
        );
        assert_eq!(measure_level(&[f32::NAN, 0.5]).peak, 0.5);
        assert!(estimate_pitch(&[0.0; 1024], 48_000, 60.0, 900.0).is_none());
        assert_eq!(waveform_bars(&[0.0, 1.0, 0.5], 2).chars().count(), 2);
    }
}
