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
use crate::song::{ChordEvent, LyricsWord, NoteEvent, TabNote, next_event_id, reserve_event_ids_after};
use crate::timeline::{MAX_TIMELINE_SECONDS, MeterChange, TempoChange, TimelineMap};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Aktuelle Version des serialisierten LiveTab-Songmodells.
pub const PROJECT_SCHEMA_VERSION: u32 = 2;
/// Höchstzahl kanonischer Events in einem editierbaren Songprojekt.
pub const MAX_PROJECT_EVENTS: usize = 500_000;
/// Höchstzahl editierbarer Tabspuren.
pub const MAX_PROJECT_TRACKS: usize = 256;
/// Höchstzahl Tempo- oder Taktartwechsel je Projekt.
pub const MAX_TIMELINE_CHANGES: usize = 100_000;

/// Ursprungskategorie eines musikalischen Ereignisses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventOrigin {
    /// Herkunft wurde nicht gespeichert oder kann nicht sicher rekonstruiert werden.
    Unknown,
    /// Lokaler Live-DSP-Eingang.
    LiveDsp,
    /// Lokale Offline-DSP-Analyse.
    OfflineAnalysis,
    /// Import aus einer MIDI-Datei.
    MidiImport,
    /// Lokale Lyrics-/Spracherkennung.
    LocalTranscription,
    /// Manuelle Nutzerkorrektur.
    UserEdit,
    /// Expliziter Import aus einem Songprojekt oder einer Austauschdatei.
    FileImport,
    /// Deterministische, regelbasierte Erzeugung ohne Modellinferenz.
    RuleBasedGeneration,
}

/// Ein nachvollziehbarer Erzeugungs-/Bearbeitungsschritt eines kanonischen Events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventProvenance {
    /// Stabile ID des betroffenen kanonischen Events.
    pub event_id: u64,
    /// Herkunft oder Bearbeitungsart.
    pub origin: EventOrigin,
    /// Optionaler lokaler Provider-/Algorithmusname samt Version.
    pub provider: Option<String>,
    /// Kurzer, datensparsamer Transformationshinweis.
    pub details: Option<String>,
    /// Aufzeichnungszeitpunkt als Unix-Millisekunden.
    pub recorded_at_unix_ms: u64,
}

/// Zeitlich verorteter Beat mit Downbeat-Markierung und Erkennungskonfidenz.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeatEvent {
    /// Über Speichern/Wiederöffnen stabile Event-ID.
    #[serde(default = "next_event_id")]
    pub id: u64,
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
            id: next_event_id(),
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
    /// Über Speichern/Wiederöffnen stabile Spuren-ID.
    #[serde(default = "next_event_id")]
    pub id: u64,
    /// Anzeigename der Spur.
    pub name: String,
    /// Verwendete Gitarrenstimmung.
    pub tuning: GuitarTuning,
    /// In zeitlicher Reihenfolge notierte Gitarrentöne.
    pub notes: Vec<FrettedNote>,
    /// Hinweis, ob die Spur von einer Analyse oder vom Nutzer stammt.
    pub generated: bool,
}

/// Modellklasse einer lokalen Vier-Stem-Ausgabe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StemKind {
    /// Gesungene oder gesprochene Hauptstimme.
    Vocals,
    /// Schlagzeug- und Percussionanteil.
    Drums,
    /// Bassinstrumente.
    Bass,
    /// Übrige Instrumente des Vier-Stem-Modells.
    Other,
}

impl StemKind {
    /// Zweck: Liefert den deutschen Anzeigenamen der Stem-Klasse.
    /// Eingaben: `self` – klassifizierte Modellspur.
    /// Ausgabe: Stabile deutschsprachige Kurzbezeichnung.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Vocals => "Gesang",
            Self::Drums => "Schlagzeug",
            Self::Bass => "Bass",
            Self::Other => "Übrige Instrumente",
        }
    }
}

/// Separat abspielbare, synchronisierte WAV-Spur aus der lokalen Stem-Trennung.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StemTrack {
    /// Sicherer, projektweit eindeutiger Dateibestandteil.
    pub id: String,
    /// Im Mixer angezeigter Spurname.
    pub name: String,
    /// Von der Modellkonfiguration gelieferte Spurklasse.
    pub kind: StemKind,
    /// Lokaler WAV-Pfad; portable Archive schreiben nur einen kanonischen relativen Namen.
    pub file_path: PathBuf,
    /// WAV-Samplerate in Hertz.
    pub sample_rate: u32,
    /// Zahl interleavter WAV-Kanäle.
    pub channels: u16,
    /// Gemessene Spurzeit in Sekunden.
    pub duration_seconds: f64,
    /// Angeforderte Separator-Modellkonfiguration.
    pub model_filename: String,
    /// Version des Python-Workers oder des Pakets, falls ermittelt.
    #[serde(default)]
    pub model_version: String,
}

impl StemTrack {
    /// Zweck: Prüft sichere Kennung, WAV-Metadaten und Modellreferenz einer Stem-Spur.
    /// Eingaben: `self` – aus Workerantwort oder Projektdatei gelesene Metadaten.
    /// Ausgabe: `Ok(())` bei plausiblen Werten oder eine konkrete Validierungsmeldung.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("Stem-Spur-ID darf nur Buchstaben, Ziffern, Bindestrich oder Unterstrich enthalten.".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 80 {
            return Err("Stem-Spurname muss zwischen 1 und 80 Zeichen lang sein.".into());
        }
        if !self.duration_seconds.is_finite() || self.duration_seconds <= 0.0 {
            return Err("Stem-Spurdauer muss endlich und größer null sein.".into());
        }
        if !(8_000..=384_000).contains(&self.sample_rate) || !(1..=8).contains(&self.channels) {
            return Err("Stem-WAV benötigt 8–384 kHz und 1–8 Kanäle.".into());
        }
        if self.model_filename.trim().is_empty()
            || self.model_filename.contains('/')
            || self.model_filename.contains('\\')
        {
            return Err("Stem-Modellreferenz muss ein einfacher Dateiname sein.".into());
        }
        if self.file_path.as_os_str().is_empty() {
            return Err("Stem-WAV benötigt einen lokalen Dateipfad.".into());
        }
        Ok(())
    }
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
    /// Chronologisch sortierte, optionale Tempoänderungen auf der gemeinsamen Sekundenachse.
    #[serde(default)]
    pub tempo_changes: Vec<TempoChange>,
    /// Chronologisch sortierte Taktwechsel; jeder Wechsel beginnt einen neuen Takt.
    #[serde(default)]
    pub meter_changes: Vec<MeterChange>,
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
    /// Optionale synchronisierte Audio-Stems; fehlt das Feld in älteren Projekten, ist die Liste leer.
    #[serde(default)]
    pub stem_tracks: Vec<StemTrack>,
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
    /// Optionale, begrenzte Herkunfts-/Transformationshistorie für kanonische Events.
    #[serde(default)]
    pub event_provenance: Vec<EventProvenance>,
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
            tempo_changes: Vec::new(),
            meter_changes: Vec::new(),
            key_signature: None,
            chords: Vec::new(),
            notes: Vec::new(),
            lyrics_words: Vec::new(),
            lyrics_lines: Vec::new(),
            tab_tracks: Vec::new(),
            stem_tracks: Vec::new(),
            beats: Vec::new(),
            audio_path: None,
            duration_seconds: 0.0,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            revision: 0,
            event_provenance: Vec::new(),
        }
    }

    /// Zweck: Migriert additive Schema-1-Projekte, ergänzt nicht rekonstruierbare Herkunft als `Unknown` und reserviert IDs.
    /// Eingaben: `self` – deserialisiertes Projekt mit alten oder aktuellen Event-/Provenancefeldern.
    /// Ausgabe: `Ok(())` für unterstützte Versionen oder ein begrenzter Fehler bei Schema-/Provenanceüberlauf.
    pub fn migrate_schema(&mut self) -> Result<(), String> {
        let was_legacy = self.schema_version == 1;
        let active_ids = self.chords.iter().map(|event| event.id())
            .chain(self.notes.iter().map(|event| event.id()))
            .chain(self.lyrics_words.iter().map(|word| word.id()))
            .chain(self.lyrics_lines.iter().flat_map(|line| line.words.iter().map(LyricsWord::id)))
            .chain(self.beats.iter().map(|event| event.id))
            .chain(self.tab_tracks.iter().flat_map(|track| {
                std::iter::once(track.id).chain(track.notes.iter().map(|note| note.position.id()))
            }))
            .collect::<HashSet<_>>();
        let provenance_ids = self.event_provenance.iter()
            .map(|entry| entry.event_id).collect::<HashSet<_>>();
        let missing_provenance = active_ids.difference(&provenance_ids)
            .copied().collect::<Vec<_>>();
        if self.event_provenance.len().saturating_add(missing_provenance.len()) > MAX_PROJECT_EVENTS {
            return Err("Projektmigration würde die sichere Provenance-Grenze überschreiten.".into());
        }
        match self.schema_version {
            1 => {
                self.chords.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
                self.notes.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
                self.lyrics_words.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
                self.lyrics_lines.sort_by(|left, right| left.start_seconds.total_cmp(&right.start_seconds));
                for line in &mut self.lyrics_lines {
                    line.words.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
                }
                self.beats.sort_by(|left, right| left.time_seconds.total_cmp(&right.time_seconds));
                for track in &mut self.tab_tracks {
                    track.notes.sort_by(|left, right| left.position.start_seconds().total_cmp(&right.position.start_seconds()));
                }
                self.schema_version = PROJECT_SCHEMA_VERSION;
            }
            PROJECT_SCHEMA_VERSION => {}
            other => return Err(format!("Projektschema {other} kann nicht migriert werden.")),
        }
        if !missing_provenance.is_empty() {
            let timestamp = unix_time_ms();
            self.event_provenance.extend(missing_provenance.into_iter().map(|event_id| {
                EventProvenance {
                    event_id,
                    origin: EventOrigin::Unknown,
                    provider: None,
                    details: Some("Herkunft war im gespeicherten Projekt nicht vorhanden.".into()),
                    recorded_at_unix_ms: timestamp,
                }
            }));
        }
        if was_legacy || active_ids.difference(&provenance_ids).next().is_some() {
            self.touch();
        }
        let maximum_id = self.maximum_event_id();
        reserve_event_ids_after(maximum_id);
        Ok(())
    }

    /// Zweck: Erstellt eine validierte gemeinsame Timeline-Sicht aus dem kanonischen Projekt.
    /// Eingaben: `self` – Songprojekt mit Grundtempo/-takt und optionalen Änderungen.
    /// Ausgabe: `TimelineMap` oder eine deutsche Meldung bei unsortierten/ungültigen Änderungen.
    pub fn timeline_map(&self) -> Result<TimelineMap, String> {
        TimelineMap::new(self.tempo_changes.clone(), self.meter_changes.clone())
    }

    /// Zweck: Ersetzt Tempoänderungen nur nach vollständiger Timeline-Validierung.
    /// Eingaben: Chronologisch sortierte Tempoänderungen.
    /// Ausgabe: `Ok(())` mit Revision/Dauer oder Validierungsfehler ohne Teiländerung.
    pub fn set_tempo_changes(&mut self, changes: Vec<TempoChange>) -> Result<(), String> {
        TimelineMap::new(changes.clone(), self.meter_changes.clone())?;
        self.tempo_changes = changes;
        self.recalculate_duration();
        self.touch();
        Ok(())
    }

    /// Zweck: Ersetzt Taktwechsel nur nach vollständiger Timeline-Validierung.
    /// Eingaben: Chronologisch sortierte Meteränderungen.
    /// Ausgabe: `Ok(())` mit Revision/Dauer oder Validierungsfehler ohne Teiländerung.
    pub fn set_meter_changes(&mut self, changes: Vec<MeterChange>) -> Result<(), String> {
        TimelineMap::new(self.tempo_changes.clone(), changes.clone())?;
        self.meter_changes = changes;
        self.recalculate_duration();
        self.touch();
        Ok(())
    }

    /// Zweck: Zeichnet einen datensparsamen Provenance-Schritt für ein kanonisches Event auf.
    /// Eingaben: Event-ID, Herkunft, optionaler Provider und optionaler Transformationshinweis.
    /// Ausgabe: `Ok(())` oder kontrollierter Fehler bei überschrittener Historiengrenze.
    pub fn record_provenance(
        &mut self,
        event_id: u64,
        origin: EventOrigin,
        provider: Option<String>,
        details: Option<String>,
    ) -> Result<(), String> {
        self.record_provenance_batch(vec![(event_id, origin, provider, details)])
    }

    /// Zweck: Zeichnet mehrere aktive Eventherkünfte mit einer einzigen Revisionserhöhung auf.
    /// Eingaben: Begrenzte Tupel aus aktiver Event-ID, Herkunft, optionalem Provider und Details.
    /// Ausgabe: `Ok(())` nach vollständiger Prüfung oder ein Fehler ohne teilweise übernommene Einträge.
    pub fn record_provenance_batch(
        &mut self,
        entries: Vec<(u64, EventOrigin, Option<String>, Option<String>)>,
    ) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        if self.event_provenance.len().saturating_add(entries.len()) > MAX_PROJECT_EVENTS {
            return Err("Provenance-Historie erreicht die zulässige Projektgrenze.".into());
        }
        let active_ids = self.chords.iter().map(|event| event.id())
            .chain(self.notes.iter().map(NoteEvent::id))
            .chain(self.lyrics_words.iter().map(LyricsWord::id))
            .chain(self.lyrics_lines.iter().flat_map(|line| line.words.iter().map(LyricsWord::id)))
            .chain(self.beats.iter().map(|event| event.id))
            .chain(self.tab_tracks.iter().flat_map(|track| {
                std::iter::once(track.id).chain(track.notes.iter().map(|note| note.position.id()))
            }))
            .collect::<HashSet<_>>();
        if entries.iter().any(|(id, _, provider, details)| {
            !active_ids.contains(id)
                || provider.as_ref().is_some_and(|value| value.len() > 256)
                || details.as_ref().is_some_and(|value| value.len() > 4_096)
        }) {
            return Err("Provenance-Batch enthält eine unbekannte Event-ID oder zu lange Textfelder.".into());
        }
        let timestamp = unix_time_ms();
        self.event_provenance.extend(entries.into_iter().map(|(event_id, origin, provider, details)| {
            EventProvenance { event_id, origin, provider, details, recorded_at_unix_ms: timestamp }
        }));
        self.touch();
        Ok(())
    }

    /// Zweck: Entfernt Herkunftseinträge zu Event-IDs, die durch eine kanonische Mutation ersetzt wurden.
    /// Eingaben: `ids` – deduplizierte IDs der zu entfernenden aktiven Events.
    /// Ausgabe: Keine; Provenance zu unveränderten Events bleibt erhalten.
    fn remove_provenance_for_ids(&mut self, ids: &HashSet<u64>) {
        self.event_provenance.retain(|entry| !ids.contains(&entry.event_id));
    }

    /// Zweck: Sucht die größte ID über alle persistierten kanonischen Eventtypen.
    /// Eingaben: `self` – aktueller Projektzustand.
    /// Ausgabe: Maximale bekannte ID oder null bei einem leeren Projekt.
    fn maximum_event_id(&self) -> u64 {
        self.chords.iter().map(|event| event.id())
            .chain(self.notes.iter().map(NoteEvent::id))
            .chain(self.lyrics_words.iter().map(LyricsWord::id))
            .chain(self.lyrics_lines.iter().flat_map(|line| line.words.iter().map(LyricsWord::id)))
            .chain(self.beats.iter().map(|event| event.id))
            .chain(self.tab_tracks.iter().flat_map(|track| {
                std::iter::once(track.id).chain(track.notes.iter().map(|note| note.position.id()))
            }))
            .max().unwrap_or(0)
    }

    /// Zweck: Zählt begrenzt relevante kanonische Events einschließlich Lyrics- und Tabspurdaten.
    /// Eingaben: `self` – aktueller Projektzustand.
    /// Ausgabe: Sättigend berechnete Eventanzahl zur frühzeitigen Prüfung des Projektlimits.
    pub fn canonical_event_count(&self) -> usize {
        let mut count = self.chords.len()
            .saturating_add(self.notes.len())
            .saturating_add(self.lyrics_words.len())
            .saturating_add(self.beats.len())
            .saturating_add(self.tab_tracks.len());
        for line in &self.lyrics_lines {
            count = count.saturating_add(line.words.len());
        }
        for track in &self.tab_tracks {
            count = count.saturating_add(track.notes.len());
        }
        count
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
        let removed = self.chords.iter().map(|event| event.id()).collect::<HashSet<_>>();
        self.remove_provenance_for_ids(&removed);
        chords.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
        self.chords = chords;
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Ersetzt erkannte Notenereignisse und sortiert sie nach Startzeit.
    /// Eingaben: `notes` – validierte Tonhöhen-/Zeitereignisse.
    /// Ausgabe: Keine Rückgabe; aktualisiert Noten, Dauer und Revision.
    pub fn replace_notes(&mut self, mut notes: Vec<NoteEvent>) {
        let removed = self.notes.iter().map(NoteEvent::id).collect::<HashSet<_>>();
        self.remove_provenance_for_ids(&removed);
        notes.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
        self.notes = notes;
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Fügt eine kanonische Note hinzu oder ersetzt dieselbe stabile Event-ID.
    /// Eingaben: Validiertes NoteEvent aus Live-DSP, Offlineanalyse oder Korrektur.
    /// Ausgabe: `true`, wenn eine neue ID eingefügt wurde; sonst `false` bei Erhalt/Update der vorhandenen ID.
    pub fn upsert_note(&mut self, note: NoteEvent) -> bool {
        let inserted = if let Some(index) = self.notes.iter().position(|event| event.id() == note.id()) {
            self.notes[index] = note;
            false
        } else {
            self.notes.push(note);
            true
        };
        self.notes.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
        self.recalculate_duration();
        self.touch();
        inserted
    }

    /// Zweck: Ersetzt Beat-Ereignisse chronologisch und berechnet daraus die Songdauer.
    /// Eingaben: `beats` – geprüfte Beat-/Downbeat-Ereignisse.
    /// Ausgabe: Keine Rückgabe; aktualisiert Beatgrid, Dauer und Revision.
    pub fn replace_beats(&mut self, mut beats: Vec<BeatEvent>) {
        let removed = self.beats.iter().map(|event| event.id).collect::<HashSet<_>>();
        self.remove_provenance_for_ids(&removed);
        beats.sort_by(|left, right| left.time_seconds.total_cmp(&right.time_seconds));
        self.beats = beats;
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Übernimmt eine vollständige Lyrics-Transkription und deren Zeilen.
    /// Eingaben: Wörter und daraus gebildete Lyrics-Zeilen.
    /// Ausgabe: Keine Rückgabe; aktualisiert Lyrics, Dauer und Revision.
    pub fn replace_lyrics(&mut self, mut words: Vec<LyricsWord>, mut lines: Vec<LyricsLine>) {
        let removed = self.lyrics_words.iter()
            .chain(self.lyrics_lines.iter().flat_map(|line| line.words.iter()))
            .map(LyricsWord::id)
            .collect::<HashSet<_>>();
        self.remove_provenance_for_ids(&removed);
        words.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
        lines.sort_by(|left, right| left.start_seconds.total_cmp(&right.start_seconds));
        for line in &mut lines {
            line.words.sort_by(|left, right| left.start_seconds().total_cmp(&right.start_seconds()));
        }
        self.lyrics_words = words;
        self.lyrics_lines = lines;
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Fügt oder ersetzt eine generierte Tabspur.
    /// Eingaben: `track` – validierte Gitarrenspur mit Stimmung und Tab-Noten.
    /// Ausgabe: Keine Rückgabe; hält pro Spurname höchstens eine Spur und erhöht die Revision.
    pub fn upsert_tab_track(&mut self, mut track: TabTrack) {
        track.notes.sort_by(|left, right| left.position.start_seconds().total_cmp(&right.position.start_seconds()));
        if let Some(index) = self
            .tab_tracks
            .iter()
            .position(|item| item.name == track.name)
        {
            let removed_ids = std::iter::once(self.tab_tracks[index].id)
                .chain(self.tab_tracks[index].notes.iter().map(|note| note.position.id()))
                .collect::<HashSet<_>>();
            self.event_provenance.retain(|entry| !removed_ids.contains(&entry.event_id));
            self.tab_tracks[index] = track;
        } else {
            self.tab_tracks.push(track);
        }
        self.recalculate_duration();
        self.touch();
    }

    /// Zweck: Ersetzt alle lokal generierten Stem-Spuren nach Metadaten-/ID-Prüfung.
    /// Eingaben: `tracks` – vollständiger, synchronisierter Vier-Stem-Satz.
    /// Ausgabe: `Ok(())` und neue Revision oder Validierungsfehler ohne Projektänderung.
    pub fn replace_stem_tracks(&mut self, tracks: Vec<StemTrack>) -> Result<(), String> {
        let mut ids = HashSet::new();
        for track in &tracks {
            track.validate()?;
            if !ids.insert(track.id.as_str()) {
                return Err("Stem-Spur-IDs müssen innerhalb eines Projekts eindeutig sein.".into());
            }
        }
        self.stem_tracks = tracks;
        self.recalculate_duration();
        self.touch();
        Ok(())
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
        let timeline_end = self
            .tempo_changes
            .iter()
            .map(|change| change.time_seconds)
            .chain(self.meter_changes.iter().map(|change| change.time_seconds))
            .fold(0.0, f64::max);
        let stem_end = self
            .stem_tracks
            .iter()
            .map(|track| track.duration_seconds)
            .fold(0.0, f64::max);
        self.duration_seconds
            .max(chord_end)
            .max(note_end)
            .max(lyric_end)
            .max(tab_end)
            .max(beat_end)
            .max(timeline_end)
            .max(stem_end)
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
        if self.duration_seconds > MAX_TIMELINE_SECONDS
            || self.title.len() > 1_024
            || self.artist.len() > 1_024
            || self.id.is_empty()
            || self.id.len() > 256
        {
            return Err("Projektmetadaten oder Dauer überschreiten die zulässigen Grenzen.".into());
        }
        if !self
            .title
            .chars()
            .any(|character| !character.is_whitespace())
        {
            return Err("Ein Songtitel darf nicht leer sein.".into());
        }
        if self.tempo_changes.len() > MAX_TIMELINE_CHANGES
            || self.meter_changes.len() > MAX_TIMELINE_CHANGES
            || self.tab_tracks.len() > MAX_PROJECT_TRACKS
            || self.lyrics_lines.len() > MAX_PROJECT_EVENTS
            || self.event_provenance.len() > MAX_PROJECT_EVENTS
        {
            return Err("Projekt enthält mehr Tracks, Timeline-Änderungen oder Lyrics-Zeilen als zulässig.".into());
        }
        self.timeline_map()?;
        if self.chords.windows(2).any(|pair| pair[0].start_seconds().total_cmp(&pair[1].start_seconds()).is_gt())
            || self.notes.windows(2).any(|pair| pair[0].start_seconds().total_cmp(&pair[1].start_seconds()).is_gt())
            || self.lyrics_words.windows(2).any(|pair| pair[0].start_seconds().total_cmp(&pair[1].start_seconds()).is_gt())
            || self.lyrics_lines.windows(2).any(|pair| pair[0].start_seconds.total_cmp(&pair[1].start_seconds).is_gt())
            || self.beats.windows(2).any(|pair| pair[0].time_seconds.total_cmp(&pair[1].time_seconds).is_gt())
            || self.lyrics_lines.iter().any(|line| line.words.windows(2).any(|pair| {
                pair[0].start_seconds().total_cmp(&pair[1].start_seconds()).is_gt()
            }))
            || self.tab_tracks.iter().any(|track| track.notes.windows(2).any(|pair| {
                pair[0].position.start_seconds().total_cmp(&pair[1].position.start_seconds()).is_gt()
            }))
        {
            return Err("Projekt-Eventlisten müssen chronologisch nach Startzeit sortiert sein.".into());
        }
        let mut event_count = self.chords.len()
            .saturating_add(self.notes.len())
            .saturating_add(self.lyrics_words.len())
            .saturating_add(self.beats.len())
            .saturating_add(self.tab_tracks.len());
        for line in &self.lyrics_lines {
            if line.text.len() > 16_384
                || !line.start_seconds.is_finite()
                || !line.end_seconds.is_finite()
                || line.start_seconds < 0.0
                || line.end_seconds < line.start_seconds
                || line.end_seconds > MAX_TIMELINE_SECONDS
                || line.start_bar == 0
                || line.end_bar < line.start_bar
            {
                return Err("Lyrics-Zeile enthält ungültige Zeit-, Takt- oder Textwerte.".into());
            }
            event_count = event_count.saturating_add(line.words.len());
        }
        for track in &self.tab_tracks {
            if track.id == 0 || track.name.is_empty() || track.name.len() > 256 {
                return Err("Tabspur benötigt eine stabile ID und einen begrenzten Namen.".into());
            }
            event_count = event_count.saturating_add(track.notes.len());
        }
        if event_count > MAX_PROJECT_EVENTS {
            return Err(format!("Projekt überschreitet das Limit von {MAX_PROJECT_EVENTS} Events."));
        }
        let canonical_lyrics = self.lyrics_words.iter()
            .map(|word| (word.id(), word))
            .collect::<HashMap<_, _>>();
        let mut event_ids = HashSet::with_capacity(event_count.min(MAX_PROJECT_EVENTS));
        for id in self.chords.iter().map(|event| event.id())
            .chain(self.notes.iter().map(NoteEvent::id))
            .chain(self.lyrics_words.iter().map(LyricsWord::id))
            .chain(self.beats.iter().map(|event| event.id))
            .chain(self.tab_tracks.iter().flat_map(|track| {
                std::iter::once(track.id).chain(track.notes.iter().map(|note| note.position.id()))
            }))
        {
            if id == 0 || !event_ids.insert(id) {
                return Err("Kanonische Projekt-Event-IDs müssen eindeutig und nichtnull sein.".into());
            }
        }
        let mut nested_lyric_ids = HashSet::new();
        for word in self.lyrics_lines.iter().flat_map(|line| line.words.iter()) {
            let id = word.id();
            if id == 0 || !nested_lyric_ids.insert(id) {
                return Err("Verschachtelte Lyrics-Wort-IDs müssen eindeutig und nichtnull sein.".into());
            }
            if let Some(canonical) = canonical_lyrics.get(&id) {
                if *canonical != word {
                    return Err("Lyrics-Zeilenwort weicht bei gleicher Event-ID vom kanonischen Wort ab.".into());
                }
            } else if !event_ids.insert(id) {
                return Err("Kanonische Projekt-Event-IDs müssen eindeutig und nichtnull sein.".into());
            }
        }
        for provenance in &self.event_provenance {
            if !event_ids.contains(&provenance.event_id)
                || provenance.provider.as_ref().is_some_and(|value| value.len() > 256)
                || provenance.details.as_ref().is_some_and(|value| value.len() > 4_096)
            {
                return Err("Provenance verweist auf ein unbekanntes Event oder überschreitet Textgrenzen.".into());
            }
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
        for word in self.lyrics_lines.iter().flat_map(|line| line.words.iter()) {
            LyricsWord::with_confidence_and_id(
                word.id(),
                word.text(),
                word.start_seconds(),
                word.end_seconds(),
                word.confidence(),
            )
            .map_err(|error| error.to_string())?;
        }
        for beat in &self.beats {
            if beat.id == 0
                || !beat.time_seconds.is_finite()
                || !(0.0..=MAX_TIMELINE_SECONDS).contains(&beat.time_seconds)
                || !beat.confidence.is_finite()
                || !(0.0..=1.0).contains(&beat.confidence)
            {
                return Err("Beat-Event enthält ungültige ID, Zeit oder Konfidenz.".into());
            }
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
        let mut stem_ids = HashSet::new();
        for track in &self.stem_tracks {
            track.validate()?;
            if !stem_ids.insert(track.id.as_str()) {
                return Err("Stem-Spur-IDs müssen innerhalb eines Projekts eindeutig sein.".into());
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

    /// Zweck: Prüft die additive Schema-1-Migration einschließlich stabiler chronologischer Eventreihenfolge.
    /// Eingaben: Legacy-Projekt mit zwei absichtlich unsortierten Noten.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Schema 2, Sortierung und anschließende Validierung.
    #[test]
    fn schema_one_migration_sorts_legacy_events() {
        let mut project = SongProject::new("Alt", "DBZS");
        project.schema_version = 1;
        project.duration_seconds = 3.0;
        project.notes = vec![
            NoteEvent::new(69, 2.0, 0.5).unwrap(),
            NoteEvent::new(60, 0.5, 0.5).unwrap(),
        ];
        project.migrate_schema().unwrap();
        assert_eq!(project.schema_version, PROJECT_SCHEMA_VERSION);
        assert_eq!(project.notes[0].start_seconds(), 0.5);
        assert_eq!(project.event_provenance.len(), 2);
        assert!(project.event_provenance.iter().all(|entry| entry.origin == EventOrigin::Unknown));
        assert!(project.validate().is_ok());
    }

    /// Zweck: Prüft Migration, Provenance und ID-Reservierung für Lyrics-Wörter, die nur in Zeilen gespeichert sind.
    /// Eingaben: Schema-1-Projekt mit einem verschachtelten Wort und einer explizit hohen stabilen ID.
    /// Ausgabe: Keine Rückgabe; Assertions sichern `Unknown`-Herkunft, gültiges Modell und nächste freie ID.
    #[test]
    fn schema_one_migration_tracks_nested_lyrics_words() {
        let word = LyricsWord::with_confidence_and_id(10_000_000, "Hallo", 0.0, 0.5, 1.0).unwrap();
        let word_id = word.id();
        let mut project = SongProject::new("Lyrics alt", "DBZS");
        project.schema_version = 1;
        project.duration_seconds = 0.5;
        project.lyrics_lines.push(LyricsLine {
            text: "Hallo".into(),
            start_seconds: 0.0,
            end_seconds: 0.5,
            start_bar: 1,
            end_bar: 1,
            words: vec![word],
        });

        project.migrate_schema().unwrap();
        assert!(project.event_provenance.iter().any(|entry| {
            entry.event_id == word_id && entry.origin == EventOrigin::Unknown
        }));
        assert!(project.validate().is_ok());
        assert!(NoteEvent::new(60, 0.0, 0.5).unwrap().id() > word_id);
    }

    /// Zweck: Erlaubt dieselbe kanonische Lyrics-ID in Top-Level-Liste und Zeilenspiegel, verwirft aber Mehrfachbelegung.
    /// Eingaben: Gültiges Lyrics-Wort, dessen ID in `lyrics_words` und genau einer Zeile wiederverwendet wird.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Spiegelkonsistenz und eindeutige verschachtelte Events.
    #[test]
    fn mirrored_lyrics_words_validate_once_and_nested_duplicates_fail() {
        let word = LyricsWord::new("Hallo", 0.0, 0.5).unwrap();
        let mut project = SongProject::new("Lyrics", "DBZS");
        project.duration_seconds = 0.5;
        project.lyrics_words.push(word.clone());
        project.lyrics_lines.push(LyricsLine {
            text: "Hallo".into(),
            start_seconds: 0.0,
            end_seconds: 0.5,
            start_bar: 1,
            end_bar: 1,
            words: vec![word.clone()],
        });
        assert!(project.validate().is_ok());

        let mut duplicate = project.clone();
        duplicate.lyrics_lines[0].words.push(word);
        assert!(duplicate.validate().unwrap_err().contains("Verschachtelte Lyrics-Wort-IDs"));

        let conflicting_word = LyricsWord::with_confidence_and_id(
            project.lyrics_words[0].id(), "Anders", 0.0, 0.5, 1.0,
        ).unwrap();
        let mut conflicting = project;
        conflicting.lyrics_lines[0].words[0] = conflicting_word;
        assert!(conflicting.validate().unwrap_err().contains("weicht bei gleicher Event-ID"));

        let invalid_word: LyricsWord = serde_json::from_value(serde_json::json!({
            "id": 10_000_001_u64,
            "text": "Hallo",
            "start": 0.0,
            "end": 0.5,
            "confidence": 1.5,
        })).unwrap();
        let mut invalid_nested = SongProject::new("Lyrics", "DBZS");
        invalid_nested.duration_seconds = 0.5;
        invalid_nested.lyrics_lines.push(LyricsLine {
            text: "Hallo".into(),
            start_seconds: 0.0,
            end_seconds: 0.5,
            start_bar: 1,
            end_bar: 1,
            words: vec![invalid_word],
        });
        assert!(invalid_nested.validate().is_err());
    }

    /// Zweck: Prüft, dass Event-IDs und Provenance über JSON-Speichern sowie Schemaöffnung erhalten bleiben.
    /// Eingaben: Projekt mit einer manuell markierten Note und JSON-Roundtrip.
    /// Ausgabe: Keine Rückgabe; Assertions sichern identische ID und Herkunftsreferenz.
    #[test]
    fn event_ids_and_provenance_survive_json_roundtrip() {
        let mut project = SongProject::new("ID", "DBZS");
        let note = NoteEvent::new(64, 0.0, 0.5).unwrap();
        let id = note.id();
        project.notes.push(note);
        project.duration_seconds = 0.5;
        project.record_provenance(
            id, EventOrigin::UserEdit, Some("test".into()), Some("Korrektur".into()),
        ).unwrap();
        let bytes = serde_json::to_vec(&project).unwrap();
        let mut restored: SongProject = serde_json::from_slice(&bytes).unwrap();
        restored.migrate_schema().unwrap();
        assert!(restored.validate().is_ok());
        assert_eq!(restored.notes[0].id(), id);
        assert_eq!(restored.event_provenance[0].event_id, id);
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

    /// Zweck: Sichert die Vier-Stem-Metadatenvalidierung und Dauerfortschreibung ab.
    /// Eingaben: Keine; verwendet vier synthetische, valide WAV-Metadatensätze.
    /// Ausgabe: Keine Rückgabe; Assertions prüfen Spurklassen, Dauer und Fehlerfälle.
    #[test]
    fn stem_tracks_validate_and_extend_project_duration() {
        let mut project = SongProject::new("Probe", "DBZS");
        let tracks = [
            StemKind::Vocals,
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, kind)| StemTrack {
            id: format!("stem-{index}"),
            name: kind.display_name().into(),
            kind,
            file_path: PathBuf::from(format!("/tmp/stem-{index}.wav")),
            sample_rate: 44_100,
            channels: 2,
            duration_seconds: 12.0,
            model_filename: "htdemucs.yaml".into(),
            model_version: "0.48.0".into(),
        })
        .collect::<Vec<_>>();
        project.replace_stem_tracks(tracks.clone()).unwrap();
        assert_eq!(project.stem_tracks.len(), 4);
        assert_eq!(project.calculated_duration_seconds(), 12.0);
        assert!(project.validate().is_ok());

        let mut invalid = tracks;
        invalid[0].id = "../escape".into();
        assert!(
            project
                .replace_stem_tracks(invalid)
                .unwrap_err()
                .contains("ID")
        );
        assert_eq!(project.stem_tracks.len(), 4);
    }

    /// Zweck: Prüft, dass vor Stem-Spuren gespeicherte Songprojekte ohne das neue optionale Feld weiter lesbar sind.
    /// Eingaben: Keine; erzeugt Projekt-JSON und entfernt nur `stemTracks`.
    /// Ausgabe: Keine Rückgabe; Assertions bestätigen leere Stem-Liste und gültiges Altschema.
    #[test]
    fn old_project_json_defaults_to_no_stem_tracks() {
        let project = SongProject::new("Probe", "DBZS");
        let mut value = serde_json::to_value(project).unwrap();
        value.as_object_mut().unwrap().remove("stemTracks");
        let loaded: SongProject = serde_json::from_value(value).unwrap();
        assert!(loaded.stem_tracks.is_empty());
        assert!(loaded.validate().is_ok());
    }
}
