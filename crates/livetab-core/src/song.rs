/*
===============================================================================
Division By Zeros (DBZS) - Rust-Core-Songmodelle
Modul: song
Zweck: Validierte Song-Metadaten sowie zeitbezogene Musik-, Lyrik- und Tabereignisse.
Herkunft: Neu in Rust formuliert; C#-Modelle auf G:\Dev\LIVETAB wurden nur gelesen.
===============================================================================
*/

use crate::domain::{
    Chord, DomainError, KeySignature, MusicalPosition, NoteName, TimeSignature, validate_bpm,
    validate_confidence, validate_time_range,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(1);

/// Zweck: Liefert eine auf `u64` begrenzte Unix-Millisekundenzahl für lokale Event-IDs.
/// Eingaben: Keine; liest die Systemuhr und verwendet bei Uhrfehlern null Dauer.
/// Ausgabe: Nichtnegative Millisekunden seit Unix-Epoch.
fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// Zweck: Reserviert die globale ID-Sequenz hinter allen bereits geladenen Projektevents.
/// Eingaben: Größte im Projekt gespeicherte Ereignis-ID.
/// Ausgabe: Keine; spätere neu erzeugte IDs liegen oberhalb der geladenen IDs.
pub(crate) fn reserve_event_ids_after(maximum_id: u64) {
    NEXT_EVENT_ID.fetch_max(maximum_id.saturating_add(1), Ordering::AcqRel);
}

/// Zweck: Erzeugt eine monoton steigende Ereignis-ID mit Zeitanteil für stabile lokale Identität.
/// Eingaben: Keine; liest die Systemzeit und aktualisiert einen atomaren Zähler.
/// Ausgabe: Nichtnull-ID, die im laufenden Prozess nicht wiederverwendet wird.
pub(crate) fn next_event_id() -> u64 {
    let time_floor = unix_time_ms().saturating_mul(65_536).max(1);
    let mut current = NEXT_EVENT_ID.load(Ordering::Relaxed);
    loop {
        let next = current.max(time_floor).saturating_add(1);
        match NEXT_EVENT_ID.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(observed) => current = observed,
        }
    }
}

/// Song-Metadaten und zugehörige Akkordereignisse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    title: String,
    artist: String,
    bpm: f64,
    time_signature: TimeSignature,
    #[serde(rename = "key")]
    key_signature: Option<KeySignature>,
    created_at_unix_ms: u64,
    #[serde(rename = "events")]
    chord_events: Vec<ChordEvent>,
}

impl Song {
    /// Zweck: Erstellt einen Song mit 120 BPM, 4/4-Takt und leerer Akkordliste.
    /// Eingaben: `title` – Songtitel; `artist` – Künstlername.
    /// Ausgabe: Neue Songinstanz mit Erstellungszeitpunkt in Unix-Millisekunden.
    pub fn new(title: impl Into<String>, artist: impl Into<String>) -> Self {
        let created_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        Self {
            title: title.into(),
            artist: artist.into(),
            bpm: 120.0,
            time_signature: TimeSignature::default(),
            key_signature: None,
            created_at_unix_ms,
            chord_events: Vec::new(),
        }
    }

    /// Zweck: Liefert den Songtitel.
    /// Eingaben: `self` – Songinstanz.
    /// Ausgabe: Unveränderliche Sicht auf den Titel.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Zweck: Liefert den Künstlernamen.
    /// Eingaben: `self` – Songinstanz.
    /// Ausgabe: Unveränderliche Sicht auf den Künstlernamen.
    pub fn artist(&self) -> &str {
        &self.artist
    }

    /// Zweck: Liefert das aktuell gespeicherte Tempo.
    /// Eingaben: `self` – Songinstanz.
    /// Ausgabe: Viertelnoten pro Minute.
    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Zweck: Setzt ein validiertes positives Songtempo.
    /// Eingaben: `bpm` – endliches Tempo in Viertelnoten pro Minute.
    /// Ausgabe: `Ok(())` bei Erfolg oder `InvalidBpm`, ohne den bisherigen Wert zu ändern.
    pub fn set_bpm(&mut self, bpm: f64) -> Result<(), DomainError> {
        validate_bpm(bpm)?;
        self.bpm = bpm;
        Ok(())
    }

    /// Zweck: Liefert die gültige Taktart des Songs.
    /// Eingaben: `self` – Songinstanz.
    /// Ausgabe: Kopie der aktuell gespeicherten Taktart.
    pub fn time_signature(&self) -> TimeSignature {
        self.time_signature
    }

    /// Zweck: Setzt eine zuvor validierte Taktart.
    /// Eingaben: `time_signature` – gültiger Zähler/Nenner-Wert.
    /// Ausgabe: Keine Rückgabe; ersetzt die bisherige Taktart.
    pub fn set_time_signature(&mut self, time_signature: TimeSignature) {
        self.time_signature = time_signature;
    }

    /// Zweck: Liefert die optionale erkannte oder manuell gesetzte Tonart.
    /// Eingaben: `self` – Songinstanz.
    /// Ausgabe: Kopie der Tonart oder `None`, falls noch keine Tonart feststeht.
    pub fn key_signature(&self) -> Option<KeySignature> {
        self.key_signature
    }

    /// Zweck: Setzt oder entfernt die Songtonart.
    /// Eingaben: `key_signature` – Tonart oder `None` zum Leeren des Feldes.
    /// Ausgabe: Keine Rückgabe; aktualisiert die Songmetadaten.
    pub fn set_key_signature(&mut self, key_signature: Option<KeySignature>) {
        self.key_signature = key_signature;
    }

    /// Zweck: Liefert den Erstellungszeitpunkt als Unix-Millisekunden.
    /// Eingaben: `self` – Songinstanz.
    /// Ausgabe: Nichtnegative Millisekunden seit dem 1. Januar 1970 UTC.
    pub fn created_at_unix_ms(&self) -> u64 {
        self.created_at_unix_ms
    }

    /// Zweck: Liefert die Akkordereignisse in ihrer gespeicherten Reihenfolge.
    /// Eingaben: `self` – Songinstanz.
    /// Ausgabe: Unveränderliche Sicht auf die Ereignisliste.
    pub fn chord_events(&self) -> &[ChordEvent] {
        &self.chord_events
    }

    /// Zweck: Fügt ein zuvor validiertes Akkordereignis an den Song an.
    /// Eingaben: `event` – zeitlich gültiges Akkordereignis.
    /// Ausgabe: Keine Rückgabe; erhält die Einfügereihenfolge.
    pub fn add_chord_event(&mut self, event: ChordEvent) {
        self.chord_events.push(event);
    }

    /// Zweck: Berechnet die Songdauer aus dem spätesten Ende eines Akkordereignisses.
    /// Eingaben: `self` – Songinstanz.
    /// Ausgabe: Maximale Ereignisendzeit in Sekunden oder `0.0` bei leerer Liste.
    pub fn duration_seconds(&self) -> f64 {
        self.chord_events
            .iter()
            .map(ChordEvent::end_seconds)
            .fold(0.0, f64::max)
    }
}

/// Zeitlich begrenztes Akkordereignis mit Konfidenz.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChordEvent {
    /// Über Speichern/Wiederöffnen stabile lokale Ereignis-ID.
    #[serde(default = "next_event_id")]
    id: u64,
    chord: Chord,
    #[serde(rename = "start")]
    start_seconds: f64,
    #[serde(rename = "end")]
    end_seconds: f64,
    confidence: f32,
}

impl ChordEvent {
    /// Zweck: Erstellt ein Akkordereignis mit Standardkonfidenz 1.0.
    /// Eingaben: `chord` – erkannter oder gesetzter Akkord; `start_seconds` und `end_seconds` – Grenzen in Sekunden.
    /// Ausgabe: Validiertes Ereignis oder `InvalidTimeRange` bei ungültigen Grenzen.
    pub fn new(chord: Chord, start_seconds: f64, end_seconds: f64) -> Result<Self, DomainError> {
        Self::with_confidence(chord, start_seconds, end_seconds, 1.0)
    }

    /// Zweck: Erstellt ein Akkordereignis mit expliziter Konfidenz.
    /// Eingaben: Akkord, positive Zeitgrenzen in Sekunden und Konfidenz zwischen null und eins.
    /// Ausgabe: Validiertes Ereignis oder ein Domänenfehler.
    pub fn with_confidence(
        chord: Chord,
        start_seconds: f64,
        end_seconds: f64,
        confidence: f32,
    ) -> Result<Self, DomainError> {
        Self::with_event_id(next_event_id(), chord, start_seconds, end_seconds, confidence)
    }

    /// Zweck: Erstellt oder rekonstruiert ein Akkordereignis unter Erhalt einer vorhandenen ID.
    /// Eingaben: Ereignis-ID, Akkord, Zeitgrenzen und Konfidenz.
    /// Ausgabe: Validiertes Ereignis mit stabiler ID oder Domänenfehler.
    pub fn with_event_id(
        id: u64,
        chord: Chord,
        start_seconds: f64,
        end_seconds: f64,
        confidence: f32,
    ) -> Result<Self, DomainError> {
        validate_time_range(start_seconds, end_seconds)?;
        validate_confidence(confidence)?;
        Ok(Self {
            id,
            chord,
            start_seconds,
            end_seconds,
            confidence,
        })
    }

    /// Zweck: Liefert die stabile ID dieses Akkordereignisses.
    /// Eingaben: `self` – Akkordereignis.
    /// Ausgabe: Nichtnull-ID, die sich bei inhaltlicher Korrektur nicht ändert.
    pub fn id(self) -> u64 {
        self.id
    }

    /// Zweck: Liefert den Akkord des Ereignisses.
    /// Eingaben: `self` – Akkordereignis.
    /// Ausgabe: Kopie des Akkordwerts.
    pub fn chord(self) -> Chord {
        self.chord
    }

    /// Zweck: Liefert die Startzeit des Ereignisses.
    /// Eingaben: `self` – Akkordereignis.
    /// Ausgabe: Startzeit in Sekunden.
    pub fn start_seconds(self) -> f64 {
        self.start_seconds
    }

    /// Zweck: Liefert die Endzeit des Ereignisses.
    /// Eingaben: `self` – Akkordereignis.
    /// Ausgabe: Endzeit in Sekunden.
    pub fn end_seconds(&self) -> f64 {
        self.end_seconds
    }

    /// Zweck: Liefert die Dauer des Ereignisses.
    /// Eingaben: `self` – zuvor validiertes Akkordereignis.
    /// Ausgabe: Positive Dauer in Sekunden.
    pub fn duration_seconds(self) -> f64 {
        self.end_seconds - self.start_seconds
    }

    /// Zweck: Liefert die Konfidenz des Ereignisses.
    /// Eingaben: `self` – Akkordereignis.
    /// Ausgabe: Wert zwischen null und eins.
    pub fn confidence(self) -> f32 {
        self.confidence
    }

    /// Zweck: Berechnet die Taktposition des Ereignisbeginns anhand des Songs.
    /// Eingaben: `bpm` – Viertelnoten pro Minute; `time_signature` – gültige Songtaktart.
    /// Ausgabe: Einsbasierte Takt-/Schlagposition oder ein Domänenfehler.
    pub fn musical_position(
        self,
        bpm: f64,
        time_signature: TimeSignature,
    ) -> Result<MusicalPosition, DomainError> {
        MusicalPosition::at(self.start_seconds, bpm, time_signature)
    }
}

/// Einzelnes musikalisches Notenereignis mit gespeichertem oder aus MIDI abgeleitetem Notenlabel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteEvent {
    /// Über Speichern/Wiederöffnen stabile lokale Ereignis-ID.
    #[serde(default = "next_event_id")]
    id: u64,
    pitch: String,
    midi: u8,
    #[serde(rename = "start")]
    start_seconds: f64,
    #[serde(rename = "duration")]
    duration_seconds: f64,
    velocity: u8,
    confidence: f32,
}

impl NoteEvent {
    /// Zweck: Erstellt ein Notenereignis mit Anschlagstärke 100 und Konfidenz 1.0.
    /// Eingaben: MIDI-Notennummer 0 bis 127, nichtnegative Startzeit und positive Dauer in Sekunden.
    /// Ausgabe: Validiertes Ereignis oder ein Domänenfehler.
    pub fn new(midi: u8, start_seconds: f64, duration_seconds: f64) -> Result<Self, DomainError> {
        Self::with_metadata(midi, start_seconds, duration_seconds, 100, 1.0)
    }

    /// Zweck: Erstellt ein Notenereignis mit expliziter Anschlagstärke und Konfidenz.
    /// Eingaben: MIDI-Nummer, Startzeit, Dauer, Anschlagstärke 0 bis 127 und Konfidenz 0 bis 1.
    /// Ausgabe: Validiertes Ereignis oder ein Domänenfehler.
    pub fn with_metadata(
        midi: u8,
        start_seconds: f64,
        duration_seconds: f64,
        velocity: u8,
        confidence: f32,
    ) -> Result<Self, DomainError> {
        Self::with_metadata_and_id(next_event_id(), midi, start_seconds, duration_seconds, velocity, confidence)
    }

    /// Zweck: Erstellt oder korrigiert ein Notenereignis unter Erhalt seiner bisherigen ID.
    /// Eingaben: Ereignis-ID, MIDI-Nummer, Zeit, Dauer, Velocity und Konfidenz.
    /// Ausgabe: Validiertes Notenereignis oder Domänenfehler.
    pub fn with_metadata_and_id(
        id: u64,
        midi: u8,
        start_seconds: f64,
        duration_seconds: f64,
        velocity: u8,
        confidence: f32,
    ) -> Result<Self, DomainError> {
        if midi > 127 {
            return Err(DomainError::MidiOutOfRange);
        }
        if velocity > 127 {
            return Err(DomainError::InvalidVelocity);
        }
        let end_seconds = start_seconds + duration_seconds;
        validate_time_range(start_seconds, end_seconds)?;
        validate_confidence(confidence)?;
        let pitch = format!(
            "{}{}",
            NoteName::from_pitch_class(midi % 12).as_str(),
            i16::from(midi) / 12 - 1
        );
        Ok(Self {
            id,
            pitch,
            midi,
            start_seconds,
            duration_seconds,
            velocity,
            confidence,
        })
    }

    /// Zweck: Liefert die stabile ID dieses Notenereignisses.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: Nichtnull-ID des kanonischen Ereignisses.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Zweck: Liefert die MIDI-Notennummer.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: MIDI-Wert von 0 bis 127.
    pub fn midi(&self) -> u8 {
        self.midi
    }

    /// Zweck: Erstellt ein Notenereignis und erhält die vom C#-Modell gespeicherte Notenbezeichnung.
    /// Eingaben: `pitch` – unverändertes Anzeigelabel; MIDI-Nummer, Startzeit und Dauer.
    /// Ausgabe: Validiertes Ereignis mit Standardvelocity und Standardkonfidenz oder ein Domänenfehler.
    pub fn with_pitch_label(
        pitch: impl Into<String>,
        midi: u8,
        start_seconds: f64,
        duration_seconds: f64,
    ) -> Result<Self, DomainError> {
        let mut event = Self::new(midi, start_seconds, duration_seconds)?;
        event.pitch = pitch.into();
        Ok(event)
    }

    /// Zweck: Liefert das gespeicherte oder aus MIDI abgeleitete Notenlabel.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: Unveränderliche Sicht auf die Notenbezeichnung einschließlich Oktave.
    pub fn pitch(&self) -> &str {
        &self.pitch
    }

    /// Zweck: Leitet den kanonischen Notennamen aus der MIDI-Nummer ab.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: Tonklasse ohne Oktavzahl.
    pub fn pitch_name(&self) -> NoteName {
        NoteName::from_pitch_class(self.midi % 12)
    }

    /// Zweck: Leitet die wissenschaftliche Oktavzahl aus der MIDI-Nummer ab.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: MIDI-Oktave, wobei C4 der MIDI-Nummer 60 entspricht.
    pub fn octave(&self) -> i8 {
        (i16::from(self.midi) / 12 - 1) as i8
    }

    /// Zweck: Liefert die Startzeit der Note.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: Startzeit in Sekunden.
    pub fn start_seconds(&self) -> f64 {
        self.start_seconds
    }

    /// Zweck: Liefert die Dauer der Note.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: Positive Dauer in Sekunden.
    pub fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    /// Zweck: Liefert die MIDI-Anschlagstärke.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: Anschlagstärke von 0 bis 127.
    pub fn velocity(&self) -> u8 {
        self.velocity
    }

    /// Zweck: Liefert die Analysekonfidenz.
    /// Eingaben: `self` – Notenereignis.
    /// Ausgabe: Konfidenzwert zwischen null und eins.
    pub fn confidence(&self) -> f32 {
        self.confidence
    }
}

/// Einzelnes, zeitlich ausgerichtetes Wort aus einer Liedtexttranskription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsWord {
    /// Über Speichern/Wiederöffnen stabile lokale Ereignis-ID.
    #[serde(default = "next_event_id")]
    id: u64,
    text: String,
    #[serde(rename = "start")]
    start_seconds: f64,
    #[serde(rename = "end")]
    end_seconds: f64,
    confidence: f32,
}

impl LyricsWord {
    /// Zweck: Erstellt ein Lyrik-Wort mit Standardkonfidenz 1.0.
    /// Eingaben: Sichtbarer Worttext sowie Start- und Endzeit in Sekunden.
    /// Ausgabe: Validiertes Wort oder ein Domänenfehler.
    pub fn new(
        text: impl Into<String>,
        start_seconds: f64,
        end_seconds: f64,
    ) -> Result<Self, DomainError> {
        Self::with_confidence(text, start_seconds, end_seconds, 1.0)
    }

    /// Zweck: Erstellt ein Lyrik-Wort mit expliziter Konfidenz.
    /// Eingaben: Worttext, positive Zeitgrenzen in Sekunden und Konfidenz zwischen null und eins.
    /// Ausgabe: Validiertes Wort oder ein Domänenfehler.
    pub fn with_confidence(
        text: impl Into<String>,
        start_seconds: f64,
        end_seconds: f64,
        confidence: f32,
    ) -> Result<Self, DomainError> {
        Self::with_confidence_and_id(next_event_id(), text, start_seconds, end_seconds, confidence)
    }

    /// Zweck: Erstellt oder bearbeitet ein Wort unter Beibehaltung einer vorhandenen ID.
    /// Eingaben: Ereignis-ID, sichtbarer Text, Zeitgrenzen und Konfidenz.
    /// Ausgabe: Validiertes Wortereignis oder Domänenfehler.
    pub fn with_confidence_and_id(
        id: u64,
        text: impl Into<String>,
        start_seconds: f64,
        end_seconds: f64,
        confidence: f32,
    ) -> Result<Self, DomainError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(DomainError::EmptyText);
        }
        validate_time_range(start_seconds, end_seconds)?;
        validate_confidence(confidence)?;
        Ok(Self {
            id,
            text,
            start_seconds,
            end_seconds,
            confidence,
        })
    }

    /// Zweck: Liefert die stabile ID dieses Lyrics-Worts.
    /// Eingaben: `self` – Wortereignis.
    /// Ausgabe: Nichtnull-ID des wortgenauen Zeitsegments.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Zweck: Liefert den transkribierten Worttext.
    /// Eingaben: `self` – Lyrik-Wort.
    /// Ausgabe: Unveränderliche Sicht auf den Text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Zweck: Liefert die Startzeit des Wortes.
    /// Eingaben: `self` – Lyrik-Wort.
    /// Ausgabe: Startzeit in Sekunden.
    pub fn start_seconds(&self) -> f64 {
        self.start_seconds
    }

    /// Zweck: Liefert die Endzeit des Wortes.
    /// Eingaben: `self` – Lyrik-Wort.
    /// Ausgabe: Endzeit in Sekunden.
    pub fn end_seconds(&self) -> f64 {
        self.end_seconds
    }

    /// Zweck: Liefert die Wortkonfidenz.
    /// Eingaben: `self` – Lyrik-Wort.
    /// Ausgabe: Wert zwischen null und eins.
    pub fn confidence(&self) -> f32 {
        self.confidence
    }

    /// Zweck: Berechnet Takt und Schlag des Wortbeginns.
    /// Eingaben: `bpm` – Viertelnoten pro Minute; `time_signature` – gültige Taktart.
    /// Ausgabe: Einsbasierte Takt-/Schlagposition oder ein Domänenfehler.
    pub fn musical_position(
        &self,
        bpm: f64,
        time_signature: TimeSignature,
    ) -> Result<MusicalPosition, DomainError> {
        MusicalPosition::at(self.start_seconds, bpm, time_signature)
    }
}

/// Gitarrentabulaturnote mit einsbasierter Saitennummer und Bund.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabNote {
    /// Über Speichern/Wiederöffnen stabile lokale Ereignis-ID.
    #[serde(default = "next_event_id")]
    id: u64,
    #[serde(rename = "string")]
    string_number: u8,
    fret: u8,
    #[serde(rename = "start")]
    start_seconds: f64,
    #[serde(rename = "duration")]
    duration_seconds: f64,
    technique: Option<String>,
}

impl TabNote {
    /// Zweck: Erstellt eine Tabulatur-Note ohne Technikannotation.
    /// Eingaben: Positive, einsbasierte Saitennummer, Bundnummer, Startzeit und positive Dauer in Sekunden.
    /// Ausgabe: Validierte Tabulatur-Note oder `InvalidStringNumber`/`InvalidTimeRange`.
    pub fn new(
        string_number: u8,
        fret: u8,
        start_seconds: f64,
        duration_seconds: f64,
    ) -> Result<Self, DomainError> {
        Self::with_event_id(next_event_id(), string_number, fret, start_seconds, duration_seconds)
    }

    /// Zweck: Erstellt oder bearbeitet eine Tabnote unter Erhalt einer vorhandenen ID.
    /// Eingaben: Ereignis-ID, einsbasierte Saite, Bund, Startzeit und Dauer.
    /// Ausgabe: Validierte Tabnote oder `InvalidStringNumber`/`InvalidTimeRange`.
    pub fn with_event_id(
        id: u64,
        string_number: u8,
        fret: u8,
        start_seconds: f64,
        duration_seconds: f64,
    ) -> Result<Self, DomainError> {
        if string_number == 0 {
            return Err(DomainError::InvalidStringNumber);
        }
        let end_seconds = start_seconds + duration_seconds;
        validate_time_range(start_seconds, end_seconds)?;
        Ok(Self {
            id,
            string_number,
            fret,
            start_seconds,
            duration_seconds,
            technique: None,
        })
    }

    /// Zweck: Liefert die stabile ID dieser Tabnote.
    /// Eingaben: `self` – Tabulatur-Note.
    /// Ausgabe: Nichtnull-ID zur Verknüpfung mit Projekt-Provenance.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Zweck: Setzt eine optionale Spieltechnik wie Hammer-on, Pull-off oder Slide.
    /// Eingaben: `technique` – Textbezeichnung oder `None` zum Entfernen der Annotation.
    /// Ausgabe: Keine Rückgabe; aktualisiert die Technikannotation.
    pub fn set_technique(&mut self, technique: Option<String>) {
        self.technique = technique;
    }

    /// Zweck: Liefert die einsbasierte Saitennummer.
    /// Eingaben: `self` – Tabulatur-Note.
    /// Ausgabe: Positive Saitennummer.
    pub fn string_number(&self) -> u8 {
        self.string_number
    }

    /// Zweck: Liefert die Bundnummer.
    /// Eingaben: `self` – Tabulatur-Note.
    /// Ausgabe: Bundnummer einschließlich null für eine offene Saite.
    pub fn fret(&self) -> u8 {
        self.fret
    }

    /// Zweck: Liefert die Startzeit der Tabulatur-Note.
    /// Eingaben: `self` – Tabulatur-Note.
    /// Ausgabe: Startzeit in Sekunden.
    pub fn start_seconds(&self) -> f64 {
        self.start_seconds
    }

    /// Zweck: Liefert die Dauer der Tabulatur-Note.
    /// Eingaben: `self` – Tabulatur-Note.
    /// Ausgabe: Positive Dauer in Sekunden.
    pub fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    /// Zweck: Liefert die optionale Spieltechnik.
    /// Eingaben: `self` – Tabulatur-Note.
    /// Ausgabe: Unveränderliche Sicht auf die Technik oder `None`.
    pub fn technique(&self) -> Option<&str> {
        self.technique.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ChordQuality, KeyMode, NoteName};

    /// Zweck: Prüft Songstandardwerte und die Dauer aus dem spätesten Akkordende.
    /// Eingaben: Keine; der Test baut einen Song mit zwei Akkordereignissen auf.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Defaults, Ereignisliste und Dauer.
    #[test]
    fn song_uses_valid_defaults_and_tracks_duration() {
        let mut song = Song::new("Test", "DBZS");
        assert_eq!(song.title(), "Test");
        assert_eq!(song.artist(), "DBZS");
        assert_eq!(song.bpm(), 120.0);
        assert_eq!(song.time_signature(), TimeSignature::default());
        assert_eq!(song.duration_seconds(), 0.0);
        assert!(song.created_at_unix_ms() > 0);

        let chord = Chord {
            root: NoteName::E,
            quality: ChordQuality::Minor,
        };
        song.add_chord_event(ChordEvent::new(chord, 0.0, 1.0).unwrap());
        song.add_chord_event(ChordEvent::new(chord, 2.0, 3.5).unwrap());
        assert_eq!(song.chord_events().len(), 2);
        assert_eq!(song.duration_seconds(), 3.5);
        assert!(song.set_bpm(0.0).is_err());
        assert_eq!(song.bpm(), 120.0);
        assert!(song.set_bpm(90.0).is_ok());
        assert_eq!(song.bpm(), 90.0);
        song.set_key_signature(Some(KeySignature {
            root: NoteName::E,
            mode: KeyMode::Minor,
        }));
        assert_eq!(song.key_signature().unwrap().name(), "E minor");
    }

    /// Zweck: Prüft Akkordereignisvalidierung, Konfidenz und Taktposition.
    /// Eingaben: Keine; der Test verwendet ein C-Dur-Ereignis an einer Taktgrenze.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Standardkonfidenz und Fehlerfälle.
    #[test]
    fn chord_events_validate_and_calculate_musical_position() {
        let chord = Chord {
            root: NoteName::C,
            quality: ChordQuality::Major,
        };
        let event = ChordEvent::new(chord, 1.0, 2.0).unwrap();
        assert_eq!(event.duration_seconds(), 1.0);
        assert_eq!(event.confidence(), 1.0);
        assert_eq!(
            event
                .musical_position(120.0, TimeSignature::default())
                .unwrap(),
            MusicalPosition { bar: 1, beat: 3.0 }
        );
        let next_bar = ChordEvent::new(chord, 2.0, 3.0).unwrap();
        assert_eq!(
            next_bar
                .musical_position(120.0, TimeSignature::default())
                .unwrap(),
            MusicalPosition { bar: 2, beat: 1.0 }
        );
        assert!(ChordEvent::new(chord, 2.0, 1.0).is_err());
        assert!(ChordEvent::with_confidence(chord, 0.0, 1.0, 1.1).is_err());
    }

    /// Zweck: Prüft die Tonhöhenableitung und Grenzen eines MIDI-Notenereignisses.
    /// Eingaben: Keine; der Test verwendet MIDI 69 sowie ungültige Anschlagstärke und Note.
    /// Ausgabe: Keine Rückgabe; Assertions sichern A4, Standardvelocity und Validierung.
    #[test]
    fn note_events_derive_pitch_and_validate_midi_values() {
        let note = NoteEvent::new(69, 1.25, 0.5).unwrap();
        assert_eq!(note.pitch_name(), NoteName::A);
        assert_eq!(note.octave(), 4);
        assert_eq!(note.pitch(), "A4");
        assert_eq!(note.velocity(), 100);
        assert_eq!(note.confidence(), 1.0);
        let labeled = NoteEvent::with_pitch_label("Bb4", 70, 0.0, 0.5).unwrap();
        assert_eq!(labeled.pitch(), "Bb4");
        assert_eq!(labeled.pitch_name(), NoteName::ASharp);
        assert_eq!(labeled.octave(), 4);
        assert!(NoteEvent::with_metadata(128, 0.0, 1.0, 100, 1.0).is_err());
        assert!(NoteEvent::with_metadata(60, 0.0, 1.0, 128, 1.0).is_err());
        assert!(NoteEvent::new(60, 1.0, 0.0).is_err());
    }

    /// Zweck: Prüft Lyriktext, Zeitposition und Konfidenzvalidierung.
    /// Eingaben: Keine; der Test verwendet ein Wort an einer definierten Zeitposition.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Text, Position und Fehlerfälle.
    #[test]
    fn lyrics_words_validate_text_and_map_to_beats() {
        let word = LyricsWord::new("fire", 2.13, 2.52).unwrap();
        assert_eq!(word.text(), "fire");
        assert_eq!(word.confidence(), 1.0);
        let position = word
            .musical_position(120.0, TimeSignature::default())
            .unwrap();
        assert_eq!(position.bar, 2);
        assert!((position.beat - 1.26).abs() < 1e-9);
        assert!(LyricsWord::new("  ", 0.0, 1.0).is_err());
        assert!(LyricsWord::with_confidence("word", 0.0, 1.0, f32::NAN).is_err());
    }

    /// Zweck: Prüft Saiten-, Bund-, Zeit- und Technikdaten einer Tabulatur-Note.
    /// Eingaben: Keine; der Test erzeugt eine offene hohe E-Saite und setzt eine Technik.
    /// Ausgabe: Keine Rückgabe; Assertions sichern einsbasierte Saite und optionale Annotation.
    #[test]
    fn tab_notes_keep_string_fret_and_technique() {
        let mut note = TabNote::new(1, 0, 1.25, 0.5).unwrap();
        note.set_technique(Some("h".into()));
        assert_eq!(note.string_number(), 1);
        assert_eq!(note.fret(), 0);
        assert_eq!(note.start_seconds(), 1.25);
        assert_eq!(note.duration_seconds(), 0.5);
        assert_eq!(note.technique(), Some("h"));
        assert!(TabNote::new(0, 3, 0.0, 0.5).is_err());
    }
}
