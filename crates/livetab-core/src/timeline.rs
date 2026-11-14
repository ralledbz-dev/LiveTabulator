/*
===============================================================================
DBZS LiveTabulator – gemeinsame musikalische Timeline
Modul: timeline
Zweck: Übersetzt Sekunden, Viertelnoten-Ticks, Tempoereignisse und Taktwechsel
       auf einer validierten, plattformneutralen Zeitbasis.
Sprache: Rust
Hinweise:
- Taktwechsel beginnen am Wechselzeitpunkt einen neuen Takt.
- Tempo wird in Viertelnoten pro Minute gespeichert; der angezeigte Beat folgt
  dem Nenner der jeweils gültigen Taktart.
- Ereigniszeiten bleiben in Sekunden; Austauschformate erhalten eigene Ticks.
===============================================================================
*/
use crate::domain::{MusicalPosition, TimeSignature, validate_bpm};
use serde::{Deserialize, Serialize};

/// Harte Tickgrenze für Import/Export und versehentliche Extremwerte.
pub const MAX_TIMELINE_TICKS: u64 = 100_000_000;
/// Harte Zeitgrenze für musikalische Timeline-Ereignisse in Sekunden.
pub const MAX_TIMELINE_SECONDS: f64 = 604_800.0;

/// Tempoänderung auf einer absoluten Songzeit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TempoChange {
    /// Zeit ab Songanfang in Sekunden.
    pub time_seconds: f64,
    /// Neues Viertelnoten-Tempo.
    pub bpm: f64,
}

/// Taktartwechsel auf einer absoluten Songzeit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterChange {
    /// Zeit ab Songanfang in Sekunden; an dieser Stelle beginnt ein neuer Takt.
    pub time_seconds: f64,
    /// Neue gültige Zähler-/Nennerkombination.
    pub time_signature: TimeSignature,
}

/// Gemeinsame, unveränderliche Timeline-Sicht für Playback, Export und Analyse.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMap {
    tempo_changes: Vec<TempoChange>,
    meter_changes: Vec<MeterChange>,
}

impl TimelineMap {
    /// Zweck: Validiert und übernimmt sortierte Tempo- und Taktwechsel.
    /// Eingaben: Tempo-/Meterlisten; beide Listen müssen strikt nach Zeit sortiert und im sicheren Zeitbereich sein.
    /// Ausgabe: Timeline oder deutsche Validierungsmeldung ohne stilles Umsortieren.
    pub fn new(tempo_changes: Vec<TempoChange>, meter_changes: Vec<MeterChange>) -> Result<Self, String> {
        validate_tempo_changes(&tempo_changes)?;
        validate_meter_changes(&meter_changes)?;
        Ok(Self { tempo_changes, meter_changes })
    }

    /// Zweck: Liefert das nach den gespeicherten Wechseln an `seconds` gültige Tempo.
    /// Eingaben: Nichtnegative Songzeit und positives Projekt-Basistempo.
    /// Ausgabe: Viertelnoten pro Minute oder Validierungsfehler.
    pub fn tempo_at(&self, seconds: f64, base_bpm: f64) -> Result<f64, String> {
        validate_seconds(seconds)?;
        validate_bpm(base_bpm).map_err(|error| error.to_string())?;
        Ok(self.tempo_changes.iter().take_while(|change| change.time_seconds <= seconds)
            .last().map(|change| change.bpm).unwrap_or(base_bpm))
    }

    /// Zweck: Liefert die nach den gespeicherten Wechseln an `seconds` gültige Taktart.
    /// Eingaben: Nichtnegative Songzeit und gültige Projekt-Grundtaktart.
    /// Ausgabe: An der Zeitposition geltende Taktart oder Validierungsfehler.
    pub fn meter_at(&self, seconds: f64, base_meter: TimeSignature) -> Result<TimeSignature, String> {
        validate_seconds(seconds)?;
        validate_meter(base_meter)?;
        Ok(self.meter_changes.iter().take_while(|change| change.time_seconds <= seconds)
            .last().map(|change| change.time_signature).unwrap_or(base_meter))
    }

    /// Zweck: Integriert alle Tempoabschnitte in Viertelnoten und berechnet eine SMF-PPQ-Position.
    /// Eingaben: Nichtnegative Songzeit, positives Basistempo und positive PPQ-Auflösung.
    /// Ausgabe: Begrenzte MIDI-Tickposition oder Fehler bei Überlauf/ungültigen Werten.
    pub fn seconds_to_ticks(&self, seconds: f64, base_bpm: f64, ppq: u16) -> Result<u64, String> {
        validate_seconds(seconds)?;
        validate_bpm(base_bpm).map_err(|error| error.to_string())?;
        if ppq == 0 {
            return Err("MIDI-PPQ muss größer als null sein.".into());
        }
        let beats = self.quarter_beats_between(0.0, seconds, base_bpm)?;
        let ticks = (beats * f64::from(ppq)).round();
        if !ticks.is_finite() || ticks < 0.0 || ticks > MAX_TIMELINE_TICKS as f64 {
            return Err("Timeline-Position überschreitet das sichere MIDI-Ticklimit.".into());
        }
        Ok(ticks as u64)
    }

    /// Zweck: Invertiert die PPQ-Position unter Berücksichtigung jedes Tempoabschnitts.
    /// Eingaben: Tickposition bis zum sicheren Limit, positives Basistempo und positive PPQ-Auflösung.
    /// Ausgabe: Songzeit in Sekunden oder Validierungs-/Bereichsfehler.
    pub fn ticks_to_seconds(&self, ticks: u64, base_bpm: f64, ppq: u16) -> Result<f64, String> {
        validate_bpm(base_bpm).map_err(|error| error.to_string())?;
        if ppq == 0 || ticks > MAX_TIMELINE_TICKS {
            return Err("MIDI-Ticks oder PPQ liegen außerhalb des sicheren Bereichs.".into());
        }
        let target_beats = ticks as f64 / f64::from(ppq);
        let mut consumed_beats = 0.0;
        let mut cursor_seconds = 0.0;
        let mut bpm = self.tempo_at(0.0, base_bpm)?;
        for change in self.tempo_changes.iter().filter(|change| change.time_seconds > 0.0) {
            let segment_beats = (change.time_seconds - cursor_seconds) * bpm / 60.0;
            if target_beats <= consumed_beats + segment_beats {
                return Ok(cursor_seconds + (target_beats - consumed_beats).max(0.0) * 60.0 / bpm);
            }
            consumed_beats += segment_beats;
            cursor_seconds = change.time_seconds;
            bpm = change.bpm;
        }
        let seconds = cursor_seconds + (target_beats - consumed_beats).max(0.0) * 60.0 / bpm;
        validate_seconds(seconds)?;
        Ok(seconds)
    }

    /// Zweck: Berechnet Takt/Beat unter Integration von Tempo- und Taktwechseln.
    /// Eingaben: Nichtnegative Songzeit, positives Basistempo und gültige Grundtaktart.
    /// Ausgabe: Einsbasierte musikalische Position oder Fehler bei ungültiger/zu großer Zeit.
    pub fn musical_position_at(
        &self,
        seconds: f64,
        base_bpm: f64,
        base_meter: TimeSignature,
    ) -> Result<MusicalPosition, String> {
        validate_seconds(seconds)?;
        validate_bpm(base_bpm).map_err(|error| error.to_string())?;
        validate_meter(base_meter)?;
        let mut bar = 1_u64;
        let mut beats_in_bar = 0.0_f64;
        let mut cursor = 0.0_f64;
        let mut meter = self.meter_at(0.0, base_meter)?;
        for change in self.meter_changes.iter().filter(|change| change.time_seconds <= seconds) {
            if change.time_seconds > cursor {
                let quarter_beats = self.quarter_beats_between(cursor, change.time_seconds, base_bpm)?;
                beats_in_bar += quarter_beats * meter.denominator() as f64 / 4.0;
                advance_bar(&mut bar, &mut beats_in_bar, meter);
                if beats_in_bar > 1e-9 {
                    bar = bar.saturating_add(1);
                }
                beats_in_bar = 0.0;
                cursor = change.time_seconds;
            }
            meter = change.time_signature;
        }
        if seconds > cursor {
            let quarter_beats = self.quarter_beats_between(cursor, seconds, base_bpm)?;
            beats_in_bar += quarter_beats * meter.denominator() as f64 / 4.0;
        }
        advance_bar(&mut bar, &mut beats_in_bar, meter);
        Ok(MusicalPosition { bar: bar.max(1), beat: beats_in_bar + 1.0 })
    }

    /// Zweck: Wandelt einen absoluten Sampleindex in die gemeinsame Songsekundenachse um.
    /// Eingaben: Sampleindex ab Quellbeginn und positive native Sample-Rate.
    /// Ausgabe: Endliche Sekundenposition innerhalb der Timelinegrenze oder Validierungsfehler.
    pub fn sample_to_seconds(sample_index: u64, sample_rate: u32) -> Result<f64, String> {
        if sample_rate == 0 {
            return Err("Sample-Rate muss größer als null sein.".into());
        }
        let seconds = sample_index as f64 / f64::from(sample_rate);
        validate_seconds(seconds)?;
        Ok(seconds)
    }

    /// Zweck: Wandelt eine Songsekunde in den nächstliegenden nativen Sampleindex um.
    /// Eingaben: Endliche Songsekunden und positive Sample-Rate.
    /// Ausgabe: Gerundeter Sampleindex oder Fehler bei Zeit-/Indexüberlauf.
    pub fn seconds_to_sample(seconds: f64, sample_rate: u32) -> Result<u64, String> {
        validate_seconds(seconds)?;
        if sample_rate == 0 {
            return Err("Sample-Rate muss größer als null sein.".into());
        }
        let samples = (seconds * f64::from(sample_rate)).round();
        if !samples.is_finite() || samples < 0.0 || samples >= u64::MAX as f64 {
            return Err("Timeline-Sampleposition überschreitet den sicheren Bereich.".into());
        }
        Ok(samples as u64)
    }

    /// Zweck: Berechnet die PPQ-Tickposition eines Quell-Sampleindex.
    /// Eingaben: Sampleindex, Sample-Rate, Basis-BPM und MIDI-PPQ.
    /// Ausgabe: Integrierte MIDI-Ticks auf derselben Tempo-Map.
    pub fn sample_to_ticks(
        &self,
        sample_index: u64,
        sample_rate: u32,
        base_bpm: f64,
        ppq: u16,
    ) -> Result<u64, String> {
        self.seconds_to_ticks(Self::sample_to_seconds(sample_index, sample_rate)?, base_bpm, ppq)
    }

    /// Zweck: Ermittelt den nächstliegenden Quell-Sampleindex für einen PPQ-Tick.
    /// Eingaben: MIDI-Ticks, Basis-BPM, PPQ und Sample-Rate.
    /// Ausgabe: Sampleindex auf der gemeinsamen Audiozeitachse oder Bereichsfehler.
    pub fn ticks_to_sample(
        &self,
        ticks: u64,
        base_bpm: f64,
        ppq: u16,
        sample_rate: u32,
    ) -> Result<u64, String> {
        Self::seconds_to_sample(self.ticks_to_seconds(ticks, base_bpm, ppq)?, sample_rate)
    }

    /// Zweck: Integriert Viertelnoten über ein geschlossenes Sekundenintervall.
    /// Eingaben: Geordnete Zeitgrenzen und positives Projekt-Basistempo.
    /// Ausgabe: Endliche Viertelnotenanzahl oder Validierungsfehler.
    fn quarter_beats_between(&self, start: f64, end: f64, base_bpm: f64) -> Result<f64, String> {
        validate_seconds(start)?;
        validate_seconds(end)?;
        validate_bpm(base_bpm).map_err(|error| error.to_string())?;
        if end < start {
            return Err("Timeline-Intervall endet vor seinem Start.".into());
        }
        let mut cursor = start;
        let mut bpm = self.tempo_at(start, base_bpm)?;
        let mut beats = 0.0;
        for change in self.tempo_changes.iter().filter(|change| change.time_seconds > start && change.time_seconds < end) {
            beats += (change.time_seconds - cursor) * bpm / 60.0;
            cursor = change.time_seconds;
            bpm = change.bpm;
        }
        beats += (end - cursor) * bpm / 60.0;
        if beats.is_finite() { Ok(beats) } else { Err("Tempo-Integration ist nicht endlich.".into()) }
    }
}

/// Zweck: Prüft Zeit und begrenzt die gemeinsame Timeline auf sieben Tage.
/// Eingaben: Sekunden ab Songanfang.
/// Ausgabe: `Ok(())` für endliche nichtnegative Werte bis zur Obergrenze.
fn validate_seconds(seconds: f64) -> Result<(), String> {
    if seconds.is_finite() && (0.0..=MAX_TIMELINE_SECONDS).contains(&seconds) {
        Ok(())
    } else {
        Err("Timeline-Zeit muss endlich, nichtnegativ und höchstens sieben Tage sein.".into())
    }
}

/// Zweck: Validiert eine Grund- oder Wechsel-Taktart.
/// Eingaben: Zeitmaß mit Zähler 1–32 und unterstütztem Zweierpotenz-Nenner.
/// Ausgabe: `Ok(())` oder eine deutsche Fehlermeldung.
fn validate_meter(meter: TimeSignature) -> Result<(), String> {
    TimeSignature::new(meter.numerator(), meter.denominator())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Zweck: Prüft BPM-Wechsel auf zulässigen Bereich und strikt steigende Zeitpunkte.
/// Eingaben: Bereits chronologisch sortierte Tempoänderungen.
/// Ausgabe: `Ok(())` oder Fehler bei ungültigen BPM, Zeit, Sortierung oder Doppelzeitpunkt.
fn validate_tempo_changes(changes: &[TempoChange]) -> Result<(), String> {
    let mut previous = None;
    for change in changes {
        validate_seconds(change.time_seconds)?;
        validate_bpm(change.bpm).map_err(|error| error.to_string())?;
        if previous.is_some_and(|value| change.time_seconds <= value) {
            return Err("Tempoänderungen müssen strikt nach Zeit sortiert sein.".into());
        }
        previous = Some(change.time_seconds);
    }
    Ok(())
}

/// Zweck: Prüft Taktwechsel auf gültige Meter und strikt steigende Zeitpunkte.
/// Eingaben: Bereits chronologisch sortierte Taktartänderungen.
/// Ausgabe: `Ok(())` oder Fehler bei ungültiger Taktart, Zeit oder Sortierung.
fn validate_meter_changes(changes: &[MeterChange]) -> Result<(), String> {
    let mut previous = None;
    for change in changes {
        validate_seconds(change.time_seconds)?;
        validate_meter(change.time_signature)?;
        if previous.is_some_and(|value| change.time_seconds <= value) {
            return Err("Taktartwechsel müssen strikt nach Zeit sortiert sein.".into());
        }
        previous = Some(change.time_seconds);
    }
    Ok(())
}

/// Zweck: Überführt angesammelte Beats in abgeschlossene Takte und einen Beatrest.
/// Eingaben: Änderbare Taktzahl, Beatrest und gültige Taktart.
/// Ausgabe: Keine; abgeschlossene Takte erhöhen die einsbasierte Taktzahl.
fn advance_bar(bar: &mut u64, beats_in_bar: &mut f64, meter: TimeSignature) {
    let beats_per_bar = f64::from(meter.beats_per_bar());
    let complete_bars = (*beats_in_bar / beats_per_bar).floor().max(0.0) as u64;
    *bar = bar.saturating_add(complete_bars);
    *beats_in_bar -= complete_bars as f64 * beats_per_bar;
    if *beats_in_bar < 1e-9 {
        *beats_in_bar = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft Tick-/Sekunden-Roundtrip über einen Tempoabschnitt und einen Wechsel am Anfang.
    /// Eingaben: 120 BPM ab Start und 60 BPM ab zwei Sekunden, 480 PPQ.
    /// Ausgabe: Keine Rückgabe; Assertions sichern integrierte Tickwerte und inverse Position.
    #[test]
    fn converts_ticks_across_tempo_changes() {
        let map = TimelineMap::new(
            vec![TempoChange { time_seconds: 0.0, bpm: 120.0 }, TempoChange { time_seconds: 2.0, bpm: 60.0 }],
            vec![],
        ).unwrap();
        assert_eq!(map.seconds_to_ticks(2.0, 90.0, 480).unwrap(), 1_920);
        assert_eq!(map.seconds_to_ticks(4.0, 90.0, 480).unwrap(), 2_880);
        assert!((map.ticks_to_seconds(2_880, 90.0, 480).unwrap() - 4.0).abs() < 1e-9);
    }

    /// Zweck: Prüft den Sample-/Sekunden-/Tick-Roundtrip über einen Tempoabschnitt.
    /// Eingaben: 44,1-kHz-Sampleindex bei vier Sekunden und eine Tempoänderung nach zwei Sekunden.
    /// Ausgabe: Keine Rückgabe; Assertions sichern exakte Sample- und integrierte PPQ-Position.
    #[test]
    fn converts_samples_through_the_shared_timeline() {
        let map = TimelineMap::new(vec![
            TempoChange { time_seconds: 0.0, bpm: 120.0 },
            TempoChange { time_seconds: 2.0, bpm: 60.0 },
        ], vec![]).unwrap();
        assert_eq!(TimelineMap::sample_to_seconds(176_400, 44_100).unwrap(), 4.0);
        assert_eq!(map.sample_to_ticks(176_400, 44_100, 90.0, 480).unwrap(), 2_880);
        assert_eq!(map.ticks_to_sample(2_880, 90.0, 480, 44_100).unwrap(), 176_400);
        assert!(TimelineMap::sample_to_seconds(1, 0).is_err());
        assert!(TimelineMap::seconds_to_sample(1.0, 0).is_err());
    }

    /// Zweck: Prüft Takt-/Beatposition und explizite Re-Ankerung bei einem Taktwechsel.
    /// Eingaben: 120 BPM, Grundtakt 4/4 und Wechsel auf 3/4 bei einer Sekunde.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Grundtakt und neue Taktgrenze.
    #[test]
    fn meter_change_starts_a_new_bar() {
        let map = TimelineMap::new(vec![], vec![MeterChange {
            time_seconds: 1.0,
            time_signature: TimeSignature::new(3, 4).unwrap(),
        }]).unwrap();
        let before = map.musical_position_at(0.5, 120.0, TimeSignature::default()).unwrap();
        let after = map.musical_position_at(1.5, 120.0, TimeSignature::default()).unwrap();
        assert_eq!(before.bar, 1);
        assert!((before.beat - 2.0).abs() < 1e-9);
        assert_eq!(after.bar, 2);
        assert!((after.beat - 2.0).abs() < 1e-9);
    }

    /// Zweck: Verhindert ungeordnete Wechsel sowie MIDI-Tick- und Zeitüberläufe.
    /// Eingaben: Unscharf sortierte Wechsel, Null-PPQ und eine Zeit jenseits der Siebentagesgrenze.
    /// Ausgabe: Keine Rückgabe; Assertions erwarten kontrollierte Validierungsfehler.
    #[test]
    fn rejects_invalid_timeline_boundaries() {
        assert!(TimelineMap::new(vec![
            TempoChange { time_seconds: 2.0, bpm: 100.0 },
            TempoChange { time_seconds: 1.0, bpm: 110.0 },
        ], vec![]).is_err());
        let map = TimelineMap::new(vec![], vec![]).unwrap();
        assert!(map.seconds_to_ticks(1.0, 120.0, 0).is_err());
        assert!(map.musical_position_at(MAX_TIMELINE_SECONDS + 1.0, 120.0, TimeSignature::default()).is_err());
    }
}
