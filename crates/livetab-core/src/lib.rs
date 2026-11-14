/*
===============================================================================
Division By Zeros (DBZS) - LiveTabulator-Fachkern
Modul: livetab_core
Zweck: Bündelt validierte Musikmodelle, Timeline, lokale DSP-Analyse und Persistenz.
Sprache: Rust
Hinweise:
- Der Fachkern bleibt plattformneutral und benötigt keine GUI- oder Cloud-Runtime.
- Live-DSP und Projektlogik warten nicht auf den lokalen Sprachmodell-Worker.
===============================================================================
*/
#![forbid(unsafe_code)]

/// Zweck: Stellt lokale DSP-Analyse, Akkord-, Tonart-, Beat- und Notenerkennung bereit.
pub mod analysis;
/// Zweck: Stellt WAV-Ein-/Ausgabe, validierte Frames und den atomaren Audio-Ringpuffer bereit.
pub mod audio;
/// Zweck: Erzeugt und korrigiert tempo- und taktbasierte Beatgrids.
pub mod beat;
/// Zweck: Orchestriert Songmodell, Audio, Jobs, Eventbus, Playback und Exporte.
pub mod controller;
pub mod domain;
/// Zweck: Berechnet Chroma-, Pitch-, Pegel- und Waveform-Merkmale ohne LLM-Abhängigkeit.
pub mod dsp;
/// Zweck: Verteilt plattformneutrale Anwendungsereignisse an UI und Workflows.
pub mod event_bus;
/// Zweck: Importiert und exportiert portable Projekt-, Text-, Untertitel- und MIDI-Dateien.
pub mod export;
/// Zweck: Führt abbrechbare, fortschrittsmeldende Offlinejobs aus.
pub mod jobs;
/// Zweck: Führt optionale lokale Lyrics-Transkription, Beatzuordnung und Textkorrekturen aus.
pub mod lyrics;
pub mod model_registry;
pub mod music;
/// Zweck: Hält strukturierte Diagnoseereignisse und ausschließlich gemessene optionale Werte.
pub mod observability;
/// Zweck: Steuert musikalischen Playback-Cursor, Loops, Tempo und Metronomzeitpunkte.
pub mod playback;
/// Zweck: Bietet geprüften Import und Wiederherstellung portabler BANDMAP-Projekte.
pub mod project;
/// Zweck: Definiert das validierte, serialisierbare lokale Songprojektmodell.
pub mod project_model;
pub mod song;
/// Zweck: Berechnet spielbare Tabpositionen für Gitarrennoten und Akkorde.
pub mod tab_engine;

pub use controller::AppController;
pub use domain::{
    Chord, ChordQuality, DomainError, KeyMode, KeySignature, MusicalPosition, NoteName,
    TimeSignature,
};
pub use event_bus::{AppEvent, EventBus};
pub use project::{ProjectError, ProjectRecord, ProjectStore};
pub use project_model::{SongProject, TabTrack};
pub use song::{ChordEvent, LyricsWord, NoteEvent, Song, TabNote};
/// Zweck: Vereinheitlicht Sekunden-, Viertelnoten-, Takt- und MIDI-Tick-Zeitachsen.
pub mod timeline;
pub use timeline::{MeterChange, TempoChange, TimelineMap};
