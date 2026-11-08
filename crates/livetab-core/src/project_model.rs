/*
===============================================================================
Division By Zeros (DBZS) - Rust-Core-Projektmodell
Modul: project_model
Zweck: Versioniertes, editierbares Songprojekt mit Akkorden, Noten, Lyrics,
       Beatgrid, Tabulatur und Audioreferenz.
Sprache: Rust
Hinweise:
- Domänenwerte werden mit den bestehenden Core-Typen validiert.
- Die Oberfläche erhält nur ein Datenmodell und enthält keine Fachlogik.
===============================================================================
*/
use crate::domain::{DomainError, KeySignature, TimeSignature, validate_bpm};
use crate::song::{ChordEvent, LyricsWord, NoteEvent, TabNote};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Aktuelle Version des serialisierten LiveTab-Songmodells.
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// Zeitlich verorteter Beat mit Downbeat-Markierung und Erkennungskonfidenz.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeatEvent {
    /// Zeitposition des Beats in Sekunden ab Audioanfang.
    pub time_seconds: f64,
    /// `true`, wenn dieser Beat als Taktanfang markiert ist.
    pub is_downbeat: bool,
    /// Konfidenz der Beat-Erkennung zwischen null und eins.
    pub confidence: f32,
}

impl BeatEvent {
    /// Zweck: Erstellt einen Beat und validiert Zeitstempel sowie Konfidenz.
    /// Eingaben: `time_seconds` – nichtnegative Zeit; `is_downbeat` – Taktanfang; `confidence` – Wert 0 bis 1.
    /// Ausgabe: Validierter Beat oder `DomainError` bei ungültigen Werten.
    pub fn new(time_seconds: f64, is_downbeat: bool, confidence: f32) -> Result<Self, DomainError> {
        if !time_seconds.is_finite() || time_seconds < 0.0 {
            return Err(DomainError::InvalidTimeRange);
        }
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(DomainError::InvalidConfidence);
        }
        Ok(Self {
            time_seconds,
            is_downbeat,
            confidence,
        })
    }
}

/// Eine zusammenhängende Lyrics-Zeile mit zugehörigen wortgenauen Zeitstempeln.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsLine {
    /// Zusammengesetzter, editierbarer Text der Zeile.
    pub text: String,
    /// Startzeit der ersten Zeileinheit in Sekunden.
    pub start_seconds: f64,
    /// Endzeit der letzten Zeileinheit in Sekunden.
    pub end_seconds: f64,
    /// Einsbasierte Taktnummer am Zeilenanfang.
    pub start_bar: u64,
    /// Einsbasierte Taktnummer am Zeilenende.
    pub end_bar: u64,
    /// Wortereignisse in zeitlicher Reihenfolge.
    pub words: Vec<LyricsWord>,
}

/// Gitarrenstimmung in MIDI-Notennummern, intern von der tiefsten zur höchsten Saite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuitarTuning {
    /// Lesbarer Stimmungsname.
    pub name: String,
    /// Leere Saiten als MIDI-Werte, Index 0 ist die tiefe E-Saite.
    pub open_midi_low_to_high: [u8; 6],
    /// Capo-Bund von null bis zum zwölften Bund.
    pub capo: u8,
}

impl GuitarTuning {
    /// Zweck: Erstellt die Standardstimmung E2-A2-D3-G3-B3-E4 ohne Capo.
    /// Eingaben: Keine.
    /// Ausgabe: Neue Standardstimmung für eine sechssaitige Gitarre.
    pub fn standard() -> Self {
        Self {
            name: "EADGBE".into(),
            open_midi_low_to_high: [40, 45, 50, 55, 59, 64],
            capo: 0,
        }
    }

    /// Zweck: Prüft die Gitarrenstimmung und begrenzt den Capo auf einen sinnvollen Bundbereich.
    /// Eingaben: Keine; verwendet die bereits gespeicherten offenen Saiten und den Capo.
    /// Ausgabe: `Ok(())` bei sechs aufsteigenden, gültigen MIDI-Tönen; sonst eine Meldung.
    pub fn validate(&self) -> Result<(), String> {
        if self.capo > 12 || self.open_midi_low_to_high.iter().any(|midi| *midi > 127) {
            return Err("Stimmung oder Capo liegt außerhalb des unterstützten Bereichs.".into());
        }
        if self
            .open_midi_low_to_high
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("Leere Gitarrensaiten müssen streng aufsteigend gestimmt sein.".into());
        }
        Ok(())
    }
}

impl Default for GuitarTuning {
    /// Zweck: Liefert die übliche Standardstimmung einer sechssaitigen Gitarre.
    /// Eingaben: Keine.
    /// Ausgabe: E2-A2-D3-G3-B3-E4 ohne Capo.
    fn default() -> Self {
        Self::standard()
    }
}

/// Tab-Note mit MIDI-Referenz und validierter Saiten-/Bundposition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrettedNote {
    /// MIDI-Notennummer des klingenden Tons.
    pub midi: u8,
    /// Saiten-/Bundposition und Zeitpunkt.
    pub position: TabNote,
}

/// Gitarrenspur mit Stimmung und zeitlich sortierten Tab-Noten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabTrack {
    /// Anzeigename der Spur.
    pub name: String,
    /// Verwendete Gitarrenstimmung.
    pub tuning: GuitarTuning,
    /// In zeitlicher Reihenfolge notierte Gitarrentöne.
    pub notes: Vec<FrettedNote>,
    /// Hinweis, ob die Spur von einer Analyse oder vom Nutzer stammt.
    pub generated: bool,
}

/// Vollständiger editierbarer Songzustand für Analyse, Playback und Export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongProject {
    /// Versionsnummer des serialisierten Projektschemas.
    pub schema_version: u32,
    /// Stabiler lokaler Projektbezeichner.
    pub id: String,
    /// Songtitel.
    pub title: String,
    /// Künstlername.
    pub artist: String,
    /// Viertelnoten pro Minute.
    pub bpm: f64,
    /// Metrische Taktart.
    pub time_signature: TimeSignature,
    /// Erkannte oder manuell korrigierte Tonart.
    pub key_signature: Option<KeySignature>,
    /// Erkannte oder manuell korrigierte Akkordereignisse.
    pub chords: Vec<ChordEvent>,
    /// Erkannte Melodie-/Instrument-Notenereignisse.
    pub notes: Vec<NoteEvent>,
    /// Wortgenaue Lyrics-Zeitstempel.
    pub lyrics_words: Vec<LyricsWord>,
    /// Gruppierte Lyrics-Zeilen für Editor und Anzeige.
    pub lyrics_lines: Vec<LyricsLine>,
    /// Gitarren- und weitere Tabspuren.
    pub tab_tracks: Vec<TabTrack>,
    /// Beat-Ereignisse einschließlich Downbeats.
    pub beats: Vec<BeatEvent>,
    /// Optionale lokale Audiodatei; portable Exporte betten sie separat ein.
    pub audio_path: Option<PathBuf>,
    /// Erkannte Audiodauer in Sekunden.
    pub duration_seconds: f64,
    /// Projekterstellungszeitpunkt in Unix-Millisekunden.
    pub created_at_unix_ms: u64,
    /// Letzter Änderungszeitpunkt in Unix-Millisekunden.
    pub updated_at_unix_ms: u64,
    /// Monoton steigende Revision für nachvollziehbare UI-/Speicheränderungen.
    pub revision: u64,
}

impl SongProject {
    /// Zweck: Erstellt ein leeres Projekt mit 120 BPM, 4/4-Takt und Standardmetadaten.
    /// Eingaben: `title` – Songtitel; `artist` – Künstlername.
    /// Ausgabe: Initialisiertes Projekt mit aktuellem Erstellungs-/Änderungszeitpunkt.
    pub fn new(title: impl Into<String>, artist: impl Into<String>) -> Self {
        let now = unix_time_ms();
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            id: format!("livetab-{now}"),
            title: title.into(),
            artist: artist.into(),
            bpm: 120.0,
            time_signature: TimeSignature::default(),
            key_signature: None,
            chords: Vec::new(),
            notes: Vec::new(),
            lyrics_words: Vec::new(),
            lyrics_lines: Vec::new(),
            tab_tracks: Vec::new(),
            beats: Vec::new(),
            audio_path: None,
            duration_seconds: 0.0,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            revision: 0,
        }
    }

    /// Zweck: Ändert das Songtempo nach positiver Endlichkeitsprüfung.
    /// Eingaben: `bpm` – Viertelnoten pro Minute größer null.
    /// Ausgabe: `Ok(())` mit neuer Revision oder `InvalidBpm` ohne Zustandsänderung.
    pub fn set_bpm(&mut self, bpm: f64) -> Result<(), DomainError> {
        validate_bpm(bpm)?;
        self.bpm = bpm;
        self.touch();
        Ok(())
    }

    /// Zweck: Ersetzt erkannte Akkorde und hält sie stabil nach Startzeit sortiert.
    /// Eingaben: `chords` – validierte Akkordereignisse aus Analyse oder Korrektur.
    /// Ausgabe: Keine Rückgabe; aktualisiert Ereignisse, Dauer und Revision.
    pub fn replace_chords(&mut self, mut chords: Vec<ChordEvent>) {
        chords.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
        self.chords = chords;
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Ersetzt erkannte Notenereignisse und sortiert sie nach Startzeit.
    /// Eingaben: `notes` – validierte Tonhöhen-/Zeitereignisse.
    /// Ausgabe: Keine Rückgabe; aktualisiert Noten, Dauer und Revision.
    pub fn replace_notes(&mut self, mut notes: Vec<NoteEvent>) {
        notes.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
        self.notes = notes;
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Ersetzt Beat-Ereignisse chronologisch und berechnet daraus die Songdauer.
    /// Eingaben: `beats` – geprüfte Beat-/Downbeat-Ereignisse.
    /// Ausgabe: Keine Rückgabe; aktualisiert Beatgrid, Dauer und Revision.
    pub fn replace_beats(&mut self, mut beats: Vec<BeatEvent>) {
        beats.sort_by(|left, right| left.time_seconds.total_cmp(&right.time_seconds));
        self.beats = beats;
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Übernimmt eine vollständige Lyrics-Transkription und deren Zeilen.
    /// Eingaben: Wörter und daraus gebildete Lyrics-Zeilen.
    /// Ausgabe: Keine Rückgabe; aktualisiert Lyrics, Dauer und Revision.
    pub fn replace_lyrics(&mut self, words: Vec<LyricsWord>, lines: Vec<LyricsLine>) {
        self.lyrics_words = words;
        self.lyrics_lines = lines;
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Fügt oder ersetzt eine generierte Tabspur.
    /// Eingaben: `track` – validierte Gitarrenspur mit Stimmung und Tab-Noten.
    /// Ausgabe: Keine Rückgabe; hält pro Spurname höchstens eine Spur und erhöht die Revision.
    pub fn upsert_tab_track(&mut self, track: TabTrack) {
        if let Some(existing) = self
            .tab_tracks
            .iter_mut()
            .find(|item| item.name == track.name)
        {
            *existing = track;
        } else {
            self.tab_tracks.push(track);
        }
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Berechnet die längste bekannte Songzeit aus Audio und allen Ereignistypen.
    /// Eingaben: `self` – vollständiger Projektzustand.
    /// Ausgabe: Nichtnegative maximale Endzeit in Sekunden.
    pub fn calculated_duration_seconds(&self) -> f64 {
        let chord_end = self
            .chords
            .iter()
            .map(ChordEvent::end_seconds)
            .fold(0.0, f64::max);
        let note_end = self
            .notes
            .iter()
            .map(|item| item.start_seconds() + item.duration_seconds())
            .fold(0.0, f64::max);
        let lyric_end = self
            .lyrics_words
            .iter()
            .map(LyricsWord::end_seconds)
            .fold(0.0, f64::max);
        let tab_end = self
            .tab_tracks
            .iter()
            .flat_map(|track| track.notes.iter())
            .map(|item| item.position.start_seconds() + item.position.duration_seconds())
            .fold(0.0, f64::max);
        let beat_end = self
            .beats
            .last()
            .map(|beat| beat.time_seconds)
            .unwrap_or(0.0);
        self.duration_seconds
            .max(chord_end)
            .max(note_end)
            .max(lyric_end)
            .max(tab_end)
            .max(beat_end)
    }

    /// Zweck: Prüft das Schema, das Tempo, die Audiodauer und alle untergeordneten Zeitwerte.
    /// Eingaben: `self` – zu speicherndes oder abzuspielendes Projekt.
    /// Ausgabe: `Ok(())` oder eine deutsche Erklärung für ungültige Projektdaten.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != PROJECT_SCHEMA_VERSION {
            return Err(format!(
                "Projektschema {} wird nicht unterstützt.",
                self.schema_version
            ));
        }
        validate_bpm(self.bpm).map_err(|error| error.to_string())?;
        TimeSignature::new(
            self.time_signature.numerator(),
            self.time_signature.denominator(),
        )
        .map_err(|error| error.to_string())?;
        if !self.duration_seconds.is_finite() || self.duration_seconds < 0.0 {
            return Err("Die Projektdauer muss endlich und nichtnegativ sein.".into());
        }
        if !self
            .title
            .chars()
            .any(|character| !character.is_whitespace())
        {
            return Err("Ein Songtitel darf nicht leer sein.".into());
        }
        for chord in &self.chords {
            ChordEvent::with_confidence(
                chord.chord(),
                chord.start_seconds(),
                chord.end_seconds(),
                chord.confidence(),
            )
            .map_err(|error| error.to_string())?;
        }
        for note in &self.notes {
            NoteEvent::with_metadata(
                note.midi(),
                note.start_seconds(),
                note.duration_seconds(),
                note.velocity(),
                note.confidence(),
            )
            .map_err(|error| error.to_string())?;
        }
        for word in &self.lyrics_words {
            LyricsWord::with_confidence(
                word.text(),
                word.start_seconds(),
                word.end_seconds(),
                word.confidence(),
            )
            .map_err(|error| error.to_string())?;
        }
        for beat in &self.beats {
            BeatEvent::new(beat.time_seconds, beat.is_downbeat, beat.confidence)
                .map_err(|error| error.to_string())?;
        }
        for track in &self.tab_tracks {
            track.tuning.validate()?;
            for note in &track.notes {
                TabNote::new(
                    note.position.string_number(),
                    note.position.fret(),
                    note.position.start_seconds(),
                    note.position.duration_seconds(),
                )
                .map_err(|error| error.to_string())?;
                if note.midi > 127 {
                    return Err("Tab-MIDI-Ton liegt außerhalb 0 bis 127.".into());
                }
            }
        }
        Ok(())
    }

    /// Zweck: Aktualisiert Änderungszeitpunkt und Revisionszähler nach einer Mutation.
    /// Eingaben: `self` – zu änderndes Projekt.
    /// Ausgabe: Keine Rückgabe; Zeitstempel und Revision werden erhöht.
    pub fn touch(&mut self) {
        self.updated_at_unix_ms = unix_time_ms();
        self.revision = self.revision.saturating_add(1);
    }

    /// Zweck: Setzt die sichtbare Dauer auf den spätesten bekannten Ereignisendpunkt.
    /// Eingaben: `self` – Projekt mit gegebenenfalls geänderten Ereignislisten.
    /// Ausgabe: Keine Rückgabe; `duration_seconds` wird monoton auf den bekannten Maximalwert erweitert.
    fn recalculate_duration(&mut self) {
        self.duration_seconds = self.calculated_duration_seconds();
    }
}

/// Zweck: Liefert die aktuelle Systemzeit als saturierte Unix-Millisekundenzahl.
/// Eingaben: Keine; liest die Systemuhr.
/// Ausgabe: Nichtnegative Millisekunden seit Unix-Epoch.
fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft Projektdaten, Standardstimmung, Tempoänderung und Schema-Validierung.
    /// Eingaben: Keine; verwendet ein leeres Testprojekt.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Initialwerte und Validierungsfehler.
    #[test]
    fn project_defaults_and_validates() {
        let mut project = SongProject::new("Probe", "DBZS");
        assert_eq!(project.schema_version, PROJECT_SCHEMA_VERSION);
        assert_eq!(project.time_signature.name(), "4/4");
        assert_eq!(project.bpm, 120.0);
        assert_eq!(project.tab_tracks.len(), 0);
        assert!(project.validate().is_ok());
        project.set_bpm(90.0).unwrap();
        assert_eq!(project.bpm, 90.0);
        assert!(project.set_bpm(0.0).is_err());
        assert_eq!(project.bpm, 90.0);
    }

    /// Zweck: Sichert die Standardstimmung, Validierung und Erkennung unsortierter Saiten ab.
    /// Eingaben: Keine; verwendet Standard- und fehlerhafte sechssaitige Stimmungen.
    /// Ausgabe: Keine Rückgabe; Assertions bestätigen EADGBE und klare Fehler.
    #[test]
    fn guitar_tuning_checks_physical_order() {
        let standard = GuitarTuning::standard();
        assert_eq!(standard.open_midi_low_to_high, [40, 45, 50, 55, 59, 64]);
        assert!(standard.validate().is_ok());
        let mut invalid = standard;
        invalid.open_midi_low_to_high[1] = 39;
        assert!(invalid.validate().unwrap_err().contains("aufsteigend"));
    }
}
