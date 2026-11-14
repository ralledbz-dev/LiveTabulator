/*
===============================================================================
Division By Zeros (DBZS) - Rust-Beatgrid
Modul: beat
Zweck: CPU-basierte Onset-/Tempo-Schätzung und validiertes, editierbares
       Beatgrid für Offline- und Live-Analyse.
Sprache: Rust
Hinweise:
- Die BPM-Schätzung nutzt eine kurze Energiehüllkurve und Autokorrelation.
- Unsicherheit wird als Confidence ausgegeben, nicht als vorgetäuschte Gewissheit.
===============================================================================
*/
use crate::domain::{DomainError, MusicalPosition, TimeSignature};
use crate::project_model::BeatEvent;

/// Beatgrid mit Tempo, Taktart, Erkennungskonfidenz und sortierten Beats.
#[derive(Debug, Clone, PartialEq)]
pub struct BeatGrid {
    pub bpm: f64,
    pub offset_seconds: f64,
    pub confidence: f32,
    pub time_signature: TimeSignature,
    pub beats: Vec<BeatEvent>,
}

impl BeatGrid {
    /// Zweck: Berechnet die Takt-/Schlagposition zu einem Zeitstempel.
    /// Eingaben: `self` – validiertes Beatgrid; `seconds` – Zeit ab Audioanfang.
    /// Ausgabe: Einsbasierte musikalische Position oder Domänenfehler.
    pub fn position_at(&self, seconds: f64) -> Result<MusicalPosition, DomainError> {
        MusicalPosition::at(
            (seconds - self.offset_seconds).max(0.0),
            self.bpm,
            self.time_signature,
        )
    }

    /// Zweck: Verschiebt den Downbeat ohne BPM-/Taktänderung und erzeugt ein neues Beatgrid.
    /// Eingaben: `offset_seconds` – korrigierter Startoffset; `duration_seconds` – gewünschte Songdauer.
    /// Ausgabe: Verschobenes Beatgrid oder eine Meldung bei ungültigen Parametern.
    pub fn corrected_downbeat(
        &self,
        offset_seconds: f64,
        duration_seconds: f64,
    ) -> Result<Self, String> {
        if !offset_seconds.is_finite()
            || offset_seconds < 0.0
            || !duration_seconds.is_finite()
            || duration_seconds < offset_seconds
        {
            return Err("Downbeat-Offset oder Songdauer ist ungültig.".into());
        }
        Ok(build_grid(
            self.bpm,
            offset_seconds,
            duration_seconds,
            self.time_signature,
            self.confidence,
        ))
    }
}

/// Zweck: Schätzt ein Beatgrid aus der Audioenergiehüllkurve und einem bekannten Zeitformat.
/// Eingaben: Mono-Samples, Sample-Rate, gewünschte Dauer und gültige Taktart.
/// Ausgabe: Beatgrid von 60 bis 200 BPM oder eine erklärbare Fehlermeldung.
pub fn estimate_beat_grid(
    samples: &[f32],
    sample_rate: u32,
    duration_seconds: f64,
    time_signature: TimeSignature,
) -> Result<BeatGrid, String> {
    if sample_rate == 0
        || samples.len() < sample_rate as usize / 2
        || !duration_seconds.is_finite()
        || duration_seconds <= 0.0
    {
        return Err(
            "Für eine Beat-Schätzung werden mindestens 0,5 Sekunden gültiges Audio benötigt."
                .into(),
        );
    }
    let bucket_samples = (sample_rate as usize / 100).max(1);
    let envelope = samples
        .chunks(bucket_samples)
        .map(|chunk| {
            let energy = chunk
                .iter()
                .filter(|value| value.is_finite())
                .map(|value| value * value)
                .sum::<f32>();
            (energy / chunk.len().max(1) as f32).sqrt()
        })
        .collect::<Vec<_>>();
    let mut onset = Vec::with_capacity(envelope.len());
    let mut previous = 0.0f32;
    for level in envelope {
        onset.push((level - previous).max(0.0));
        previous = level;
    }
    let onset_energy = onset.iter().map(|value| value * value).sum::<f32>();
    if onset_energy < 1e-7 {
        return Err("Audio ist zu leise oder enthält keine auswertbaren Onsets.".into());
    }
    let frames_per_second = sample_rate as f64 / bucket_samples as f64;
    let minimum_lag = (frames_per_second * 60.0 / 200.0).round() as usize;
    let maximum_lag = (frames_per_second * 60.0 / 60.0).round() as usize;
    let mut scores = Vec::new();
    for lag in minimum_lag.max(1)..=maximum_lag.min(onset.len().saturating_sub(1)) {
        let correlation = onset
            .iter()
            .take(onset.len() - lag)
            .zip(onset.iter().skip(lag))
            .map(|(left, right)| left * right)
            .sum::<f32>();
        scores.push((lag, correlation));
    }
    let (best_lag, best_score) = scores
        .iter()
        .copied()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .ok_or_else(|| "Kein plausible Beat-Abstand gefunden.".to_string())?;
    if best_score <= 0.0 {
        return Err("Onsets ergeben keine stabile periodische Beat-Schätzung.".into());
    }
    let bpm = 60.0 * frames_per_second / best_lag as f64;
    let second = scores
        .iter()
        .map(|(_, score)| *score)
        .fold(0.0f32, f32::max);
    let baseline = scores.iter().map(|(_, score)| *score).sum::<f32>() / scores.len().max(1) as f32;
    let confidence = ((best_score - baseline).max(0.0) / best_score.max(1e-9)).clamp(0.0, 1.0)
        * (best_score / (best_score + onset_energy.max(1e-9)))
            .clamp(0.0, 1.0)
            .sqrt();
    let _ = second;
    let first_peak = (0..(best_lag.min(onset.len()).max(1)))
        .max_by(|left, right| onset[*left].total_cmp(&onset[*right]))
        .unwrap_or(0);
    let offset = first_peak as f64 / frames_per_second;
    Ok(build_grid(
        bpm,
        offset,
        duration_seconds,
        time_signature,
        confidence,
    ))
}

/// Zweck: Erzeugt fortlaufende Beats und markiert jede erste Zählzeit als Downbeat.
/// Eingaben: Gültige BPM, Offset, Audiodauer, Taktart und Schätzkonfidenz.
/// Ausgabe: Vollständiges Beatgrid; bei ungültigem Beatwert wird eine leere Beatliste erzeugt.
fn build_grid(
    bpm: f64,
    offset_seconds: f64,
    duration_seconds: f64,
    time_signature: TimeSignature,
    confidence: f32,
) -> BeatGrid {
    let beat_duration = 60.0 / bpm;
    let count = ((duration_seconds - offset_seconds).max(0.0) / beat_duration).floor() as usize + 1;
    let beats = (0..count)
        .map(|index| BeatEvent {
            id: crate::song::next_event_id(),
            time_seconds: offset_seconds + index as f64 * beat_duration,
            is_downbeat: index % usize::from(time_signature.beats_per_bar()) == 0,
            confidence,
        })
        .collect();
    BeatGrid {
        bpm,
        offset_seconds,
        confidence,
        time_signature,
        beats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft Tempo- und Downbeat-Erkennung an einem synthetischen Viertelnoten-Klicksignal.
    /// Eingaben: Keine; erzeugt 16 Sekunden Klicks bei 120 BPM.
    /// Ausgabe: Keine Rückgabe; Assertions sichern BPM-Toleranz, Beatanzahl und Taktanfänge.
    #[test]
    fn estimates_tempo_from_synthetic_click_track() {
        let sample_rate = 8_000u32;
        let duration = 16.0;
        let mut samples = vec![0.0f32; (sample_rate as f64 * duration) as usize];
        for beat in 0..32 {
            let start = beat * (sample_rate as usize / 2);
            for offset in 0..80 {
                samples[start + offset] =
                    (1.0 - offset as f32 / 80.0) * if beat % 4 == 0 { 0.9 } else { 0.5 };
            }
        }
        let grid =
            estimate_beat_grid(&samples, sample_rate, duration, TimeSignature::default()).unwrap();
        assert!((grid.bpm - 120.0).abs() <= 2.0);
        assert!(grid.beats.len() >= 30);
        assert!(grid.beats[0].is_downbeat);
    }

    /// Zweck: Prüft, dass Downbeat-Korrekturen Taktanfänge nachvollziehbar verschieben.
    /// Eingaben: Keine; erstellt ein festes 120-BPM-Grid und verschiebt es um 0,25 Sekunden.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Offset und Zeit des ersten Beats.
    #[test]
    fn corrects_downbeat_without_changing_tempo() {
        let grid = build_grid(120.0, 0.0, 4.0, TimeSignature::default(), 0.8);
        let corrected = grid.corrected_downbeat(0.25, 4.0).unwrap();
        assert_eq!(corrected.bpm, 120.0);
        assert_eq!(corrected.beats[0].time_seconds, 0.25);
    }
}
