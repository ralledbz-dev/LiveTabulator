#![forbid(unsafe_code)]

pub mod domain;
pub mod model_registry;
pub mod music;
/// Zweck: Bietet geprüften Import und Wiederherstellung portabler BANDMAP-Projekte.
pub mod project;
pub mod song;

pub use domain::{
    Chord, ChordQuality, DomainError, KeyMode, KeySignature, MusicalPosition, NoteName,
    TimeSignature,
};
pub use project::{ProjectError, ProjectRecord, ProjectStore};
pub use song::{ChordEvent, LyricsWord, NoteEvent, Song, TabNote};
