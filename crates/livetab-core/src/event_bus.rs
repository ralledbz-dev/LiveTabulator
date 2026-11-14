/*
===============================================================================
Division By Zeros (DBZS) - Rust-Eventbus
Modul: event_bus
Zweck: Verteilt Audio-, Musik-, Job-, Playback- und Fehlerereignisse zwischen
       Backend-Services und Oberfläche ohne direkte UI-Abhängigkeit.
Sprache: Rust
Hinweise:
- Ein Senderfehler entfernt nur den betroffenen Subscriber.
- EventBus-Nutzer dürfen keine Slint-Komponenten in den Fachkern injizieren.
===============================================================================
*/
use crate::project_model::BeatEvent;
use crate::song::ChordEvent;
use crate::song::NoteEvent;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// Plattformneutraler Zustand, den Analyse- und Controller-Services veröffentlichen.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// Audioaufnahme wurde gestartet oder beendet.
    AudioCaptureChanged { running: bool, source: String },
    /// Gemessener Zustand des bounded Capture-Transports, nicht eine zeitbasierte Schätzung.
    AudioTransportMetrics {
        /// Tatsächliche Capture-Samplerate in Hertz.
        sample_rate: u32,
        /// Tatsächliche Zahl erhaltener Quellkanäle.
        channels: u16,
        /// Zum Messzeitpunkt noch gepufferte vollständige Frames.
        queue_frames: usize,
        /// Seit Sessionstart verworfene Quellframes.
        dropped_frames: u64,
        /// Alle bis zur Messung gesehenen Quellframes einschließlich verworfener.
        input_frames_seen: u64,
    },
    /// Neue Pegel- und Waveformmessung.
    AudioLevel {
        rms: f32,
        peak: f32,
        waveform: String,
    },
    /// Geglättete monophone Pitchmessung; fehlende Werte bedeuten keine sichere Schätzung.
    LivePitch {
        /// Samplezeitpunkt ab Aufnahmebeginn in Sekunden.
        time_seconds: f64,
        /// Erkannte MIDI-Tonhöhe oder `None` bei Stille/Mehrdeutigkeit.
        midi: Option<u8>,
        /// Grundfrequenz in Hertz oder `None` bei fehlendem Pitch.
        frequency_hz: Option<f32>,
        /// Periodizitätskonfidenz der stabilisierten Schätzung.
        confidence: f32,
    },
    /// Neue oder verlängerte kanonische Live-Note mit stabiler Event-ID.
    NoteDetected(NoteEvent),
    /// Geglättete Rangfolge möglicher Tonarten aus mehreren Liveframes.
    LiveKeyCandidates {
        /// Höchstens drei lesbare Dur-/Moll-Kandidaten mit Rangreihenfolge.
        alternatives: Vec<String>,
        /// Korrelationskonfidenz des besten Kandidaten.
        confidence: f32,
    },
    /// Neuer analysierter oder korrigierter Akkord.
    ChordDetected(ChordEvent),
    /// Erkanntes Tempo-/Downbeat-Grid.
    BeatGridUpdated {
        bpm: f64,
        beats: Vec<BeatEvent>,
        confidence: f32,
    },
    /// Jobfortschritt im Bereich null bis eins.
    JobProgress {
        id: String,
        kind: String,
        progress: f32,
        message: String,
    },
    /// Erfolgreich abgeschlossener Job.
    JobCompleted { id: String, message: String },
    /// Job- oder Capturefehler mit Nutzertext.
    Error { category: String, message: String },
    /// Playback-Cursor für Timeline und Lyrics-Sync.
    PlaybackPosition {
        seconds: f64,
        bar: u64,
        beat: f64,
        lyric: Option<String>,
    },
}

/// Mehrteilnehmer-Eventbus mit standardmäßigen mpsc-Empfängern.
#[derive(Debug, Clone, Default)]
pub struct EventBus {
    subscribers: Arc<Mutex<Vec<Sender<AppEvent>>>>,
}

impl EventBus {
    /// Zweck: Erstellt einen neuen Eventbus ohne Abonnenten.
    /// Eingaben: Keine.
    /// Ausgabe: Leerer, klonbarer Publisherzustand.
    pub fn new() -> Self {
        Self::default()
    }

    /// Zweck: Registriert einen neuen Empfänger für künftig veröffentlichte Events.
    /// Eingaben: `self` – Eventbus.
    /// Ausgabe: Empfangskanal mit FIFO-Reihenfolge für diesen Subscriber.
    pub fn subscribe(&self) -> Receiver<AppEvent> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.push(sender);
        }
        receiver
    }

    /// Zweck: Verteilt ein Ereignis an alle aktiven Empfänger und entfernt geschlossene Kanäle.
    /// Eingaben: `event` – klonbares UI-/Workflow-Ereignis.
    /// Ausgabe: Zahl der Empfänger, die dieses Ereignis erhalten haben.
    pub fn publish(&self, event: AppEvent) -> usize {
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return 0;
        };
        let mut delivered = 0;
        subscribers.retain(|subscriber| match subscriber.send(event.clone()) {
            Ok(()) => {
                delivered += 1;
                true
            }
            Err(_) => false,
        });
        delivered
    }

    /// Zweck: Liefert die Zahl aktuell registrierter Event-Empfänger.
    /// Eingaben: `self` – Eventbus.
    /// Ausgabe: Subscriberanzahl oder null, wenn der Mutex vergiftet wurde.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .map(|items| items.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft Eventverteilung und Aufräumen eines geschlossenen Empfängers.
    /// Eingaben: Keine; erzeugt einen Subscriber und schließt ihn anschließend.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Eventinhalt und Subscriberzahl.
    #[test]
    fn distributes_and_prunes_events() {
        let bus = EventBus::new();
        let receiver = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
        assert_eq!(
            bus.publish(AppEvent::Error {
                category: "Test".into(),
                message: "Fehler".into()
            }),
            1
        );
        assert!(
            matches!(receiver.recv().unwrap(), AppEvent::Error { message, .. } if message == "Fehler")
        );
        drop(receiver);
        assert_eq!(
            bus.publish(AppEvent::JobCompleted {
                id: "1".into(),
                message: "fertig".into()
            }),
            0
        );
        assert_eq!(bus.subscriber_count(), 0);
    }
}
