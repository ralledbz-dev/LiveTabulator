/*
===============================================================================
Division By Zeros (DBZS) - Rust-Core-Domänenmodelle
Modul: domain
Zweck: Typsichere musikalische Grundbegriffe und validierte Zeit-/Taktwerte.
Herkunft: Neu in Rust formuliert; DBZS.LiveTab.Core auf G:\Dev\LIVETAB diente
          ausschließlich als fachliche Referenz und wurde nicht verändert.
===============================================================================
*/

use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};

/// Fehler, die beim Erzeugen oder Berechnen musikalischer Domänenwerte auftreten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
    /// Die Zähler-/Nennerkombination ist keine unterstützte Taktart.
    InvalidTimeSignature,
    /// Das Tempo ist nicht endlich oder nicht größer als null.
    InvalidBpm,
    /// Start und Ende bilden keinen gültigen positiven Zeitbereich.
    InvalidTimeRange,
    /// Die Konfidenz liegt nicht im geschlossenen Bereich von null bis eins.
    InvalidConfidence,
    /// Die berechnete MIDI-Notennummer liegt außerhalb von 0 bis 127.
    MidiOutOfRange,
    /// Die MIDI-Anschlagstärke liegt außerhalb von 0 bis 127.
    InvalidVelocity,
    /// Die Gitarrensaitennummer ist nicht positiv und einsbasiert.
    InvalidStringNumber,
    /// Ein Lyrik-Wort enthält keinen sichtbaren Text.
    EmptyText,
}

impl Display for DomainError {
    /// Zweck: Formuliert einen Domänenfehler als kurze deutsche Meldung.
    /// Eingaben: `self` – Fehlerursache; `formatter` – Ziel für die Textausgabe.
    /// Ausgabe: `Ok(())` nach erfolgreicher Formatierung oder ein Formatierungsfehler.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidTimeSignature => "Die Taktart ist ungültig.",
            Self::InvalidBpm => "Das Tempo muss endlich und größer als null sein.",
            Self::InvalidTimeRange => {
                "Der Zeitbereich muss endlich, nichtnegativ und positiv lang sein."
            }
            Self::InvalidConfidence => "Die Konfidenz muss zwischen 0 und 1 liegen.",
            Self::MidiOutOfRange => "Die MIDI-Notennummer muss zwischen 0 und 127 liegen.",
            Self::InvalidVelocity => "Die MIDI-Anschlagstärke muss zwischen 0 und 127 liegen.",
            Self::InvalidStringNumber => {
                "Die Gitarrensaitennummer muss einsbasiert und positiv sein."
            }
            Self::EmptyText => "Der Lyriktext darf nicht leer sein.",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for DomainError {}

/// Kanonische chromatische Notennamen; enharmonische Schreibweisen werden auf zwölf Tonklassen abgebildet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteName {
    /// Tonklasse C.
    C,
    /// Tonklasse Cis/Des, kanonisch als Cis geschrieben.
    CSharp,
    /// Tonklasse D.
    D,
    /// Tonklasse Dis/Es, kanonisch als Dis geschrieben.
    DSharp,
    /// Tonklasse E.
    E,
    /// Tonklasse F.
    F,
    /// Tonklasse Fis/Ges, kanonisch als Fis geschrieben.
    FSharp,
    /// Tonklasse G.
    G,
    /// Tonklasse Gis/As, kanonisch als Gis geschrieben.
    GSharp,
    /// Tonklasse A.
    A,
    /// Tonklasse Ais/B, kanonisch als Ais geschrieben.
    ASharp,
    /// Internationale Tonklasse B; im Deutschen H.
    B,
}

impl NoteName {
    /// Deutscher Alias für die internationale Tonklasse B.
    pub const H: Self = Self::B;

    /// Enharmonischer C#-Alias aus dem C#-Domänenmodell.
    #[allow(non_upper_case_globals)]
    pub const Db: Self = Self::CSharp;
    /// Enharmonischer D#-Alias aus dem C#-Domänenmodell.
    #[allow(non_upper_case_globals)]
    pub const Eb: Self = Self::DSharp;
    /// Enharmonischer F#-Alias aus dem C#-Domänenmodell.
    #[allow(non_upper_case_globals)]
    pub const Gb: Self = Self::FSharp;
    /// Enharmonischer G#-Alias aus dem C#-Domänenmodell.
    #[allow(non_upper_case_globals)]
    pub const Ab: Self = Self::GSharp;
    /// Enharmonischer A#-Alias aus dem C#-Domänenmodell.
    #[allow(non_upper_case_globals)]
    pub const Bb: Self = Self::ASharp;

    /// Zweck: Wandelt eine chromatische Tonklassennummer in einen kanonischen Notennamen um.
    /// Eingaben: `pitch_class` – beliebige ganzzahlige Tonklasse; Werte außerhalb 0 bis 11 werden modulo 12 abgebildet.
    /// Ausgabe: Kanonischer Notenname mit bevorzugter Kreuz-Schreibweise.
    pub const fn from_pitch_class(pitch_class: u8) -> Self {
        match pitch_class % 12 {
            0 => Self::C,
            1 => Self::CSharp,
            2 => Self::D,
            3 => Self::DSharp,
            4 => Self::E,
            5 => Self::F,
            6 => Self::FSharp,
            7 => Self::G,
            8 => Self::GSharp,
            9 => Self::A,
            10 => Self::ASharp,
            _ => Self::B,
        }
    }

    /// Zweck: Liefert die chromatische Tonklassennummer dieses Notennamens.
    /// Eingaben: `self` – ein kanonischer Notenname.
    /// Ausgabe: Ganzzahlige Tonklasse von C=0 bis H=11.
    pub const fn pitch_class(self) -> u8 {
        match self {
            Self::C => 0,
            Self::CSharp => 1,
            Self::D => 2,
            Self::DSharp => 3,
            Self::E => 4,
            Self::F => 5,
            Self::FSharp => 6,
            Self::G => 7,
            Self::GSharp => 8,
            Self::A => 9,
            Self::ASharp => 10,
            Self::B => 11,
        }
    }

    /// Zweck: Liefert die kurze ASCII-Schreibweise für Anzeige und Protokolle.
    /// Eingaben: `self` – ein kanonischer Notenname.
    /// Ausgabe: Statische Zeichenkette wie `C`, `C#` oder `A#`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::C => "C",
            Self::CSharp => "C#",
            Self::D => "D",
            Self::DSharp => "D#",
            Self::E => "E",
            Self::F => "F",
            Self::FSharp => "F#",
            Self::G => "G",
            Self::GSharp => "G#",
            Self::A => "A",
            Self::ASharp => "A#",
            Self::B => "B",
        }
    }

    /// Zweck: Liefert die deutsche Notenbezeichnung mit B für B und H für die internationale Tonklasse B.
    /// Eingaben: `self` – kanonischer chromatischer Notenname.
    /// Ausgabe: Deutsche Schreibweise; Halbton 10 heißt B, Halbton 11 heißt H.
    pub const fn as_german_str(self) -> &'static str {
        match self {
            Self::C => "C",
            Self::CSharp => "Cis",
            Self::D => "D",
            Self::DSharp => "Dis",
            Self::E => "E",
            Self::F => "F",
            Self::FSharp => "Fis",
            Self::G => "G",
            Self::GSharp => "Gis",
            Self::A => "A",
            Self::ASharp => "B",
            Self::B => "H",
        }
    }
}

impl Display for NoteName {
    /// Zweck: Schreibt den kanonischen Notennamen in einen Formatierungspuffer.
    /// Eingaben: `self` – Notenname; `formatter` – Ziel für die Textausgabe.
    /// Ausgabe: `Ok(())` nach erfolgreicher Formatierung oder ein Formatierungsfehler.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Unterstützte Akkordqualitäten und ihre Intervalle relativ zum Grundton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChordQuality {
    /// Dur-Dreiklang.
    Major,
    /// Moll-Dreiklang.
    Minor,
    /// Verminderter Dreiklang.
    Diminished,
    /// Übermäßiger Dreiklang.
    Augmented,
    /// Dominantseptakkord.
    Dominant7,
    /// Dur-Septakkord.
    Major7,
    /// Moll-Septakkord.
    Minor7,
    /// Sus2-Akkord.
    Suspended2,
    /// Sus4-Akkord.
    Suspended4,
    /// Powerchord aus Grundton und Quinte.
    PowerChord,
}

impl ChordQuality {
    /// Zweck: Liefert die chromatischen Intervalle dieser Akkordqualität.
    /// Eingaben: `self` – eine unterstützte Akkordqualität.
    /// Ausgabe: Statische Halbtonintervalle relativ zum Grundton.
    pub const fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Dominant7 => &[0, 4, 7, 10],
            Self::Major7 => &[0, 4, 7, 11],
            Self::Minor7 => &[0, 3, 7, 10],
            Self::Suspended2 => &[0, 2, 7],
            Self::Suspended4 => &[0, 5, 7],
            Self::PowerChord => &[0, 7],
        }
    }

    /// Zweck: Liefert den in Akkordnamen verwendeten Qualitätssuffix.
    /// Eingaben: `self` – eine unterstützte Akkordqualität.
    /// Ausgabe: Statischer Suffix wie `m`, `dim`, `7` oder `sus4`.
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Major => "maj",
            Self::Minor => "m",
            Self::Diminished => "dim",
            Self::Augmented => "aug",
            Self::Dominant7 => "7",
            Self::Major7 => "maj7",
            Self::Minor7 => "m7",
            Self::Suspended2 => "sus2",
            Self::Suspended4 => "sus4",
            Self::PowerChord => "5",
        }
    }
}

/// Modus einer Tonart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyMode {
    /// Dur-Modus.
    Major,
    /// Moll-Modus.
    Minor,
}

impl KeyMode {
    /// Zweck: Liefert den englischen Kurznamen des Tonartmodus für Projektdaten.
    /// Eingaben: `self` – Dur- oder Moll-Modus.
    /// Ausgabe: Statische Zeichenkette `major` oder `minor`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
        }
    }
}

/// Akkord aus Grundton und Qualität.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chord {
    /// Grundton des Akkords.
    pub root: NoteName,
    /// Qualität des Akkords.
    pub quality: ChordQuality,
}

impl Chord {
    /// Zweck: Formatiert den Akkord aus Grundton und Qualität.
    /// Eingaben: `self` – Akkord mit Grundton und Qualität.
    /// Ausgabe: Anzeigename wie `Em`, `C#maj7` oder `G5`.
    pub fn name(self) -> String {
        format!("{}{}", self.root, self.quality.suffix())
    }

    /// Zweck: Formatiert den Akkord mit den deutschen Notennamen B und H.
    /// Eingaben: `self` – Akkord mit Grundton und Qualität.
    /// Ausgabe: Deutsche Bezeichnung wie `Hm`, `Bmaj` oder `Fis7`.
    pub fn german_name(self) -> String {
        format!("{}{}", self.root.as_german_str(), self.quality.suffix())
    }

    /// Zweck: Berechnet die MIDI-Notennummern des Akkords in einer Zieloktave.
    /// Eingaben: `self` – Akkord; `octave` – wissenschaftliche MIDI-Oktave, wobei C4 die MIDI-Nummer 60 ist.
    /// Ausgabe: Akkordtöne in aufsteigender Reihenfolge oder `MidiOutOfRange`, falls ein Ton außerhalb 0 bis 127 liegt.
    pub fn midi_notes(self, octave: i8) -> Result<Vec<u8>, DomainError> {
        let root_midi = (i16::from(octave) + 1) * 12 + i16::from(self.root.pitch_class());
        self.quality
            .intervals()
            .iter()
            .map(|interval| {
                let midi = root_midi + i16::from(*interval);
                u8::try_from(midi).map_err(|_| DomainError::MidiOutOfRange)
            })
            .collect()
    }

    /// Zweck: Berechnet Akkordnoten nach der C#-Formel `Grundton + baseOctave * 12`.
    /// Eingaben: `base_octave` – C#-kompatibler Basisoktavwert; Werte außerhalb des MIDI-Bereichs schlagen fehl.
    /// Ausgabe: MIDI-Notennummern in aufsteigender Reihenfolge oder `MidiOutOfRange`.
    pub fn midi_notes_for_base_octave(self, base_octave: i16) -> Result<Vec<u8>, DomainError> {
        let octave = base_octave
            .checked_sub(1)
            .and_then(|value| i8::try_from(value).ok())
            .ok_or(DomainError::MidiOutOfRange)?;
        self.midi_notes(octave)
    }
}

/// Tonart aus Tonika und Modus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySignature {
    /// Tonika der Tonart.
    pub root: NoteName,
    /// Dur- oder Moll-Modus.
    pub mode: KeyMode,
}

impl KeySignature {
    /// Zweck: Formatiert die Tonart als lesbaren Grundton und Modus.
    /// Eingaben: `self` – Tonika und Modus.
    /// Ausgabe: Zeichenkette wie `E minor` oder `C major`.
    pub fn name(self) -> String {
        format!("{} {}", self.root, self.mode.as_str())
    }
}

/// Zähler und Nenner eines musikalischen Takts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeSignature {
    numerator: u8,
    denominator: u8,
}

impl TimeSignature {
    /// Zweck: Erstellt eine Taktart und prüft Zähler sowie üblichen Zweierpotenz-Nenner.
    /// Eingaben: `numerator` – 1 bis 32 Schläge je Takt; `denominator` – 1, 2, 4, 8, 16 oder 32.
    /// Ausgabe: Gültige Taktart oder `InvalidTimeSignature` bei nicht unterstützten Werten.
    pub fn new(numerator: u8, denominator: u8) -> Result<Self, DomainError> {
        if !(1..=32).contains(&numerator)
            || !(1..=32).contains(&denominator)
            || !denominator.is_power_of_two()
        {
            return Err(DomainError::InvalidTimeSignature);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    /// Zweck: Erstellt eine Taktart mit dem C#-Standardnenner vier.
    /// Eingaben: `numerator` – Zähler von eins bis 32.
    /// Ausgabe: Validierte Taktart wie 3/4 oder `InvalidTimeSignature`.
    pub fn with_default_denominator(numerator: u8) -> Result<Self, DomainError> {
        Self::new(numerator, 4)
    }

    /// Zweck: Liefert den Zähler der Taktart.
    /// Eingaben: `self` – eine zuvor validierte Taktart.
    /// Ausgabe: Zahl der Nenner-Schläge je Takt.
    pub const fn numerator(self) -> u8 {
        self.numerator
    }

    /// Zweck: Liefert den Nenner der Taktart.
    /// Eingaben: `self` – eine zuvor validierte Taktart.
    /// Ausgabe: Notenwert des Taktschlags, etwa 4 für Viertel oder 8 für Achtel.
    pub const fn denominator(self) -> u8 {
        self.denominator
    }

    /// Zweck: Liefert die Zahl der Taktschläge pro Takt.
    /// Eingaben: `self` – eine zuvor validierte Taktart.
    /// Ausgabe: Der Zähler der Taktart.
    pub const fn beats_per_bar(self) -> u8 {
        self.numerator
    }

    /// Zweck: Liefert den C#-kompatiblen BeatValue als Nenner geteilt durch vier.
    /// Eingaben: `self` – eine zuvor validierte Taktart.
    /// Ausgabe: BeatValue wie 1.0 für 4/4 oder 2.0 für 6/8.
    pub fn beat_value(self) -> f64 {
        f64::from(self.denominator) / 4.0
    }

    /// Zweck: Berechnet die Dauer eines Taktschlags bei Viertelnoten-BPM.
    /// Eingaben: `self` – Taktart; `bpm` – endliches, positives Viertelnoten-Tempo.
    /// Ausgabe: Schlagdauer in Sekunden oder `InvalidBpm` bei ungültigem Tempo.
    pub fn beat_duration_seconds(self, bpm: f64) -> Result<f64, DomainError> {
        validate_bpm(bpm)?;
        Ok(60.0 / bpm * 4.0 / f64::from(self.denominator))
    }

    /// Zweck: Berechnet die Dauer eines vollständigen Takts.
    /// Eingaben: `self` – Taktart; `bpm` – endliches, positives Viertelnoten-Tempo.
    /// Ausgabe: Taktdauer in Sekunden oder `InvalidBpm` bei ungültigem Tempo.
    pub fn bar_duration_seconds(self, bpm: f64) -> Result<f64, DomainError> {
        Ok(self.beat_duration_seconds(bpm)? * f64::from(self.numerator))
    }

    /// Zweck: Formatiert Zähler und Nenner der Taktart.
    /// Eingaben: `self` – eine zuvor validierte Taktart.
    /// Ausgabe: Zeichenkette wie `4/4` oder `6/8`.
    pub fn name(self) -> String {
        format!("{}/{}", self.numerator, self.denominator)
    }
}

impl Default for TimeSignature {
    /// Zweck: Liefert die Standardtaktart des Projekts.
    /// Eingaben: Keine.
    /// Ausgabe: Gültige 4/4-Taktart.
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl Display for TimeSignature {
    /// Zweck: Schreibt die Taktart als Zähler/Nenner-Text.
    /// Eingaben: `self` – Taktart; `formatter` – Ziel für die Textausgabe.
    /// Ausgabe: `Ok(())` nach erfolgreicher Formatierung oder ein Formatierungsfehler.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.numerator, self.denominator)
    }
}

/// Einsbasierte Takt- und Schlagposition eines Zeitstempels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicalPosition {
    /// Einsbasierte Taktnummer.
    pub bar: u64,
    /// Einsbasierte Schlagposition; Nachkommastellen markieren Unterteilungen.
    pub beat: f64,
}

impl MusicalPosition {
    /// Zweck: Wandelt eine Zeit in Sekunden in Takt- und Schlagposition um.
    /// Eingaben: `seconds` – nichtnegative Zeit; `bpm` – Viertelnoten pro Minute; `time_signature` – gültige Taktart.
    /// Ausgabe: Einsbasierte Position oder ein Domänenfehler bei ungültiger Zeit oder ungültigem Tempo.
    pub fn at(seconds: f64, bpm: f64, time_signature: TimeSignature) -> Result<Self, DomainError> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(DomainError::InvalidTimeRange);
        }
        let beat_duration = time_signature.beat_duration_seconds(bpm)?;
        let elapsed_beats = seconds / beat_duration;
        let beats_per_bar = f64::from(time_signature.beats_per_bar());
        let zero_based_bar = (elapsed_beats / beats_per_bar).floor();
        let beat_in_bar = elapsed_beats - zero_based_bar * beats_per_bar;
        Ok(Self {
            bar: (zero_based_bar as u64).saturating_add(1),
            beat: beat_in_bar + 1.0,
        })
    }
}

/// Zweck: Prüft, ob ein BPM-Wert für musikalische Zeitberechnungen verwendbar ist.
/// Eingaben: `bpm` – Viertelnoten pro Minute.
/// Ausgabe: `Ok(())` für positives, endliches Tempo; sonst `InvalidBpm`.
pub(crate) fn validate_bpm(bpm: f64) -> Result<(), DomainError> {
    if bpm.is_finite() && bpm > 0.0 {
        Ok(())
    } else {
        Err(DomainError::InvalidBpm)
    }
}

/// Zweck: Prüft, ob ein Ereignis einen endlichen, nichtnegativen und positiven Zeitbereich hat.
/// Eingaben: `start` und `end` – Ereignisgrenzen in Sekunden.
/// Ausgabe: Ereignisdauer in Sekunden oder `InvalidTimeRange` bei ungültigen Grenzen.
pub(crate) fn validate_time_range(start: f64, end: f64) -> Result<f64, DomainError> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
        return Err(DomainError::InvalidTimeRange);
    }
    Ok(end - start)
}

/// Zweck: Prüft den normierten Konfidenzwert eines Analyseereignisses.
/// Eingaben: `confidence` – erwarteter Wert zwischen null und eins.
/// Ausgabe: `Ok(())` bei gültiger Konfidenz; sonst `InvalidConfidence`.
pub(crate) fn validate_confidence(confidence: f32) -> Result<(), DomainError> {
    if confidence.is_finite() && (0.0..=1.0).contains(&confidence) {
        Ok(())
    } else {
        Err(DomainError::InvalidConfidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft die Tonklassenabbildung und kanonische Notenschreibweise.
    /// Eingaben: Keine; der Test verwendet ausgewählte Tonklassennummern.
    /// Ausgabe: Keine Rückgabe; Assertions sichern die Zuordnung einschließlich Modulo-Verhalten.
    #[test]
    fn maps_pitch_classes_to_canonical_names() {
        assert_eq!(NoteName::from_pitch_class(0), NoteName::C);
        assert_eq!(NoteName::from_pitch_class(10), NoteName::ASharp);
        assert_eq!(NoteName::from_pitch_class(11), NoteName::B);
        assert_eq!(NoteName::from_pitch_class(13), NoteName::CSharp);
        assert_eq!(NoteName::ASharp.as_str(), "A#");
        assert_eq!(NoteName::H, NoteName::B);
        assert_eq!(NoteName::Db, NoteName::CSharp);
        assert_eq!(NoteName::Eb, NoteName::DSharp);
        assert_eq!(NoteName::Gb, NoteName::FSharp);
        assert_eq!(NoteName::Ab, NoteName::GSharp);
        assert_eq!(NoteName::Bb, NoteName::ASharp);
        assert_eq!(NoteName::ASharp.as_german_str(), "B");
        assert_eq!(NoteName::B.as_german_str(), "H");
    }

    /// Zweck: Prüft Akkordnamen, Halbtonintervalle und MIDI-Noten in wissenschaftlicher Oktavlage.
    /// Eingaben: Keine; der Test erstellt C-Dur, E-Moll und einen Randfall außerhalb des MIDI-Bereichs.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Namen und Notennummern.
    #[test]
    fn builds_chord_names_and_midi_notes() {
        let c_major = Chord {
            root: NoteName::C,
            quality: ChordQuality::Major,
        };
        assert_eq!(c_major.name(), "Cmaj");
        assert_eq!(c_major.midi_notes(3).unwrap(), vec![48, 52, 55]);
        assert_eq!(
            c_major.midi_notes_for_base_octave(3).unwrap(),
            vec![36, 40, 43]
        );

        let e_minor = Chord {
            root: NoteName::E,
            quality: ChordQuality::Minor,
        };
        assert_eq!(e_minor.name(), "Em");
        assert_eq!(e_minor.midi_notes(3).unwrap(), vec![52, 55, 59]);
        assert!(c_major.midi_notes(-2).is_err());

        let h_minor = Chord {
            root: NoteName::H,
            quality: ChordQuality::Minor,
        };
        assert_eq!(h_minor.german_name(), "Hm");
    }

    /// Zweck: Prüft Tonartnamen sowie Intervalle der erweiterten Akkordqualitäten.
    /// Eingaben: Keine; der Test verwendet E-Moll und einen Dominantseptakkord.
    /// Ausgabe: Keine Rückgabe; Assertions sichern lesbare Bezeichnungen und Intervalle.
    #[test]
    fn names_keys_and_exposes_chord_intervals() {
        let key = KeySignature {
            root: NoteName::E,
            mode: KeyMode::Minor,
        };
        assert_eq!(key.name(), "E minor");
        let qualities: [(ChordQuality, &str, &[u8]); 10] = [
            (ChordQuality::Major, "maj", &[0, 4, 7]),
            (ChordQuality::Minor, "m", &[0, 3, 7]),
            (ChordQuality::Diminished, "dim", &[0, 3, 6]),
            (ChordQuality::Augmented, "aug", &[0, 4, 8]),
            (ChordQuality::Dominant7, "7", &[0, 4, 7, 10]),
            (ChordQuality::Major7, "maj7", &[0, 4, 7, 11]),
            (ChordQuality::Minor7, "m7", &[0, 3, 7, 10]),
            (ChordQuality::Suspended2, "sus2", &[0, 2, 7]),
            (ChordQuality::Suspended4, "sus4", &[0, 5, 7]),
            (ChordQuality::PowerChord, "5", &[0, 7]),
        ];
        for (quality, suffix, intervals) in qualities {
            assert_eq!(quality.suffix(), suffix);
            assert_eq!(quality.intervals(), intervals);
        }
    }

    /// Zweck: Prüft Taktartvalidierung, Dauern und exakte Taktgrenzen.
    /// Eingaben: Keine; der Test verwendet 3/4 bei 120 Viertelnoten pro Minute.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Default, Grenzposition und Fehlerfälle.
    #[test]
    fn validates_time_signatures_and_calculates_positions() {
        let three_four = TimeSignature::with_default_denominator(3).unwrap();
        assert_eq!(TimeSignature::default().name(), "4/4");
        assert_eq!(three_four.name(), "3/4");
        assert_eq!(three_four.beat_value(), 1.0);
        assert_eq!(three_four.bar_duration_seconds(120.0).unwrap(), 1.5);
        assert_eq!(
            MusicalPosition::at(1.5, 120.0, three_four).unwrap(),
            MusicalPosition { bar: 2, beat: 1.0 }
        );
        let six_eight = TimeSignature::new(6, 8).unwrap();
        assert_eq!(six_eight.beat_value(), 2.0);
        assert_eq!(six_eight.beat_duration_seconds(120.0), Ok(0.25));
        assert!(TimeSignature::new(4, 3).is_err());
        assert!(three_four.bar_duration_seconds(0.0).is_err());
        assert!(MusicalPosition::at(-0.1, 120.0, three_four).is_err());
    }
}
