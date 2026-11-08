//! Lightweight music-theory analysis over a 12-bin chroma vector.
//! This code is intentionally independent of llama.cpp and safe to call from a real-time
//! analysis worker; model inference must never run in an audio callback.

pub use crate::domain::{ChordQuality, KeyMode};

#[derive(Debug, Clone, PartialEq)]
pub struct ChordEstimate {
    pub root: &'static str,
    pub quality: ChordQuality,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyEstimate {
    pub tonic: &'static str,
    pub mode: KeyMode,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameAnalysis {
    pub chord: Option<ChordEstimate>,
    pub key: Option<KeyEstimate>,
}

const PITCH_NAMES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];
const MAJOR_INTERVALS: [usize; 3] = [0, 4, 7];
const MINOR_INTERVALS: [usize; 3] = [0, 3, 7];
const MAJOR_PROFILE: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
const SILENCE_EPSILON: f32 = 1.0e-6;

/// Zweck: Bewertet alle Dur-/Moll-Dreiklangsvorlagen gegen ein 12-Tonklassen-Chroma.
/// Eingaben: `chroma` – zwölf Chroma-Amplituden; nichtendliche/negative Werte werden abgefangen.
/// Ausgabe: Beste `ChordEstimate` oder `None` bei praktisch vollständiger Stille.
pub fn estimate_chord(chroma: &[f32; 12]) -> Option<ChordEstimate> {
    let total = chroma.iter().copied().map(non_negative).sum::<f32>();
    if total <= SILENCE_EPSILON {
        return None;
    }

    let mut best: Option<(usize, ChordQuality, f32)> = None;
    for root in 0..12 {
        for (quality, intervals) in [
            (ChordQuality::Major, &MAJOR_INTERVALS),
            (ChordQuality::Minor, &MINOR_INTERVALS),
        ] {
            let score = intervals
                .iter()
                .map(|interval| non_negative(chroma[(root + interval) % 12]))
                .sum::<f32>()
                / total;
            if best.map(|(_, _, current)| score > current).unwrap_or(true) {
                best = Some((root, quality, score));
            }
        }
    }

    best.map(|(root, quality, confidence)| ChordEstimate {
        root: PITCH_NAMES[root],
        quality,
        confidence,
    })
}

/// Zweck: Schätzt Dur-/Moll-Tonart mit rotierten Krumhansl-Schmuckler-Profilen.
/// Eingaben: `chroma` – zwölf Tonklassen-Amplituden.
/// Ausgabe: Beste `KeyEstimate` oder `None` bei praktisch vollständiger Stille.
pub fn estimate_key(chroma: &[f32; 12]) -> Option<KeyEstimate> {
    let normalized = chroma.map(non_negative);
    if normalized.iter().sum::<f32>() <= SILENCE_EPSILON {
        return None;
    }
    let mut best: Option<(usize, KeyMode, f32)> = None;
    for tonic in 0..12 {
        let major = rotated_profile_correlation(&normalized, &MAJOR_PROFILE, tonic);
        let minor = rotated_profile_correlation(&normalized, &MINOR_PROFILE, tonic);
        for (mode, score) in [(KeyMode::Major, major), (KeyMode::Minor, minor)] {
            if best.map(|(_, _, current)| score > current).unwrap_or(true) {
                best = Some((tonic, mode, score));
            }
        }
    }
    best.map(|(tonic, mode, confidence)| KeyEstimate {
        tonic: PITCH_NAMES[tonic],
        mode,
        confidence,
    })
}

/// Zweck: Berechnet Akkord- und Tonartschätzung gemeinsam für einen Analyse-Frame.
/// Eingaben: `chroma` – zwölf Tonklassen-Amplituden.
/// Ausgabe: `FrameAnalysis` mit optionaler Akkord- und Tonartschätzung.
pub fn analyze_chroma(chroma: &[f32; 12]) -> FrameAnalysis {
    FrameAnalysis {
        chord: estimate_chord(chroma),
        key: estimate_key(chroma),
    }
}

/// Zweck: Berechnet die normierte Korrelation zwischen Chroma und um einen Grundton rotiertem Tonartprofil.
/// Eingaben: Chroma-Vektor, Tonartprofil und Tonika-Index von 0 bis 11.
/// Ausgabe: Korrelationswert als `f32`; bei unzureichender Energie `0.0`.
fn rotated_profile_correlation(chroma: &[f32; 12], profile: &[f32; 12], tonic: usize) -> f32 {
    let mean_chroma = chroma.iter().sum::<f32>() / 12.0;
    let mean_profile = profile.iter().sum::<f32>() / 12.0;
    let mut numerator = 0.0;
    let mut chroma_energy = 0.0;
    let mut profile_energy = 0.0;
    for pitch in 0..12 {
        let left = chroma[pitch] - mean_chroma;
        let right = profile[(pitch + 12 - tonic) % 12] - mean_profile;
        numerator += left * right;
        chroma_energy += left * left;
        profile_energy += right * right;
    }
    let denominator = (chroma_energy * profile_energy).sqrt();
    if denominator <= SILENCE_EPSILON {
        0.0
    } else {
        numerator / denominator
    }
}

/// Zweck: Bereinigt einen Analysewert für die nachfolgenden Chroma-Berechnungen.
/// Eingaben: Einzelner `f32`-Wert.
/// Ausgabe: Endlicher Wert mindestens `0.0`; nichtendliche Werte werden zu `0.0`.
fn non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft, dass künstliche C-Dur- und A-Moll-Chroma-Frames korrekt klassifiziert werden.
    /// Eingaben: Keine; die Testframes werden im Test aufgebaut.
    /// Ausgabe: Keine Rückgabe; Assertions prüfen Grundton und Akkordqualität.
    #[test]
    fn identifies_major_and_minor_triads() {
        let mut c_major = [0.01; 12];
        c_major[0] = 1.0;
        c_major[4] = 0.9;
        c_major[7] = 0.8;
        let major = estimate_chord(&c_major).unwrap();
        assert_eq!((major.root, major.quality), ("C", ChordQuality::Major));

        let mut a_minor = [0.01; 12];
        a_minor[9] = 1.0;
        a_minor[0] = 0.9;
        a_minor[4] = 0.8;
        let minor = estimate_chord(&a_minor).unwrap();
        assert_eq!((minor.root, minor.quality), ("A", ChordQuality::Minor));
    }

    /// Zweck: Prüft C-Dur-Tonartschätzung sowie das Verhalten bei stillem Chroma.
    /// Eingaben: Keine; verwendet ein festgelegtes Dur-Profil und einen Nullvektor.
    /// Ausgabe: Keine Rückgabe; Assertions prüfen Tonart und `None` bei Stille.
    #[test]
    fn estimates_key_and_returns_no_result_for_silence() {
        let c_major_scale = [
            1.0, 0.05, 0.7, 0.05, 0.9, 0.6, 0.05, 0.8, 0.05, 0.65, 0.05, 0.5,
        ];
        let key = estimate_key(&c_major_scale).unwrap();
        assert_eq!((key.tonic, key.mode), ("C", KeyMode::Major));
        assert!(estimate_chord(&[0.0; 12]).is_none());
        assert!(estimate_key(&[0.0; 12]).is_none());
    }

    /// Zweck: Prüft, dass NaN im Chroma die endlichen Akkordanteile nicht unbrauchbar macht.
    /// Eingaben: Keine; der Test erstellt einen Frame mit NaN und endlichen Tönen.
    /// Ausgabe: Keine Rückgabe; eine Assertion erwartet den E-Dur-Grundton.
    #[test]
    fn ignores_non_finite_chroma_values() {
        let mut frame = [0.0; 12];
        frame[0] = f32::NAN;
        frame[4] = 1.0;
        frame[8] = 0.8;
        frame[11] = 0.7;
        let estimate = estimate_chord(&frame).unwrap();
        assert_eq!(estimate.root, "E");
    }
}
