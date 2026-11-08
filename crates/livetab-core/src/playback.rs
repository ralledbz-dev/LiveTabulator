/*
===============================================================================
Division By Zeros (DBZS) - Rust-Playback-Engine
Modul: playback
Zweck: Ereignisse zeitgenau planen, Play/Pause/Stop/Loop/Tempo sowie Cursor-,
       Noten- und Lyrics-Synchronisierung und Metronompegel bereitstellen.
Sprache: Rust
Hinweise:
- Der Scheduler arbeitet ohne UI-/Audio-Gerätezugriff und blockiert keine Echtzeit-Callbacks.
- Hardwareausgabe wird plattformgebunden vom App-Audioadapter geroutet.
===============================================================================
*/
use crate::domain::{MusicalPosition, TimeSignature};
use crate::song::{LyricsWord, NoteEvent};
use std::collections::BTreeMap;

/// Zustand eines Playback-Ticks, den Rendering und UI synchron darstellen.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackTick {
    pub position_seconds: f64,
    pub position: MusicalPosition,
    pub active_midi_notes: Vec<u8>,
    pub highlighted_lyric: Option<String>,
    pub wrapped_loop: bool,
    pub ended: bool,
}

/// Playback-Scheduler für Song-Noten, Beatposition, Loopbereiche und Lyrics-Hervorhebung.
#[derive(Debug, Clone)]
pub struct PlaybackScheduler {
    duration_seconds: f64,
    bpm: f64,
    time_signature: TimeSignature,
    notes: Vec<NoteEvent>,
    lyrics: Vec<LyricsWord>,
    position_seconds: f64,
    tempo_percent: u16,
    playing: bool,
    loop_region: Option<(f64, f64)>,
}

impl PlaybackScheduler {
    /// Zweck: Erstellt einen Scheduler aus Songdauer, Tempo, Taktart, Noten und Lyrics.
    /// Eingaben: Dauer, BPM, gültige Taktart sowie Ereignisse; Dauer/BPM müssen endlich und positiv sein.
    /// Ausgabe: Neuer gestoppter Scheduler oder eine verständliche Validierungsmeldung.
    pub fn new(
        duration_seconds: f64,
        bpm: f64,
        time_signature: TimeSignature,
        notes: Vec<NoteEvent>,
        lyrics: Vec<LyricsWord>,
    ) -> Result<Self, String> {
        if !duration_seconds.is_finite()
            || duration_seconds <= 0.0
            || !bpm.is_finite()
            || bpm <= 0.0
        {
            return Err("Playback benötigt positive, endliche Songdauer und BPM.".into());
        }
        Ok(Self {
            duration_seconds,
            bpm,
            time_signature,
            notes,
            lyrics,
            position_seconds: 0.0,
            tempo_percent: 100,
            playing: false,
            loop_region: None,
        })
    }

    /// Zweck: Startet die Wiedergabe und springt bei Songende zur Anfangsposition.
    /// Eingaben: `self` – Scheduler.
    /// Ausgabe: Keine Rückgabe; setzt den Zustand auf laufend.
    pub fn play(&mut self) {
        if let Some((start, end)) = self.loop_region {
            if self.position_seconds < start || self.position_seconds >= end {
                self.position_seconds = start;
            }
        } else if self.position_seconds >= self.duration_seconds {
            self.position_seconds = 0.0;
        }
        self.playing = true;
    }

    /// Zweck: Pausiert Wiedergabe, ohne den Cursor zu verändern.
    /// Eingaben: `self` – Scheduler.
    /// Ausgabe: Keine Rückgabe; der nächste `advance`-Aufruf bleibt am Cursor stehen.
    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Zweck: Stoppt Wiedergabe und setzt den Cursor auf null oder den Loopanfang.
    /// Eingaben: `self` – Scheduler.
    /// Ausgabe: Keine Rückgabe; Zustand ist gestoppt.
    pub fn stop(&mut self) {
        self.playing = false;
        self.position_seconds = self.loop_region.map(|region| region.0).unwrap_or(0.0);
    }

    /// Zweck: Ändert das relative Playbacktempo innerhalb eines geschützten Trainerbereichs.
    /// Eingaben: `percent` – Prozentwert von 50 bis 150.
    /// Ausgabe: `Ok(())` bei gültigem Tempo oder eine Fehlermeldung ohne Änderung.
    pub fn set_tempo_percent(&mut self, percent: u16) -> Result<(), String> {
        if !(50..=150).contains(&percent) {
            return Err("Playbacktempo muss zwischen 50 und 150 Prozent liegen.".into());
        }
        self.tempo_percent = percent;
        Ok(())
    }

    /// Zweck: Setzt einen optionalen Loopbereich in Sekunden.
    /// Eingaben: Start und Ende; der Bereich muss positiv und innerhalb der Songdauer liegen.
    /// Ausgabe: `Ok(())` oder Validierungsfehler; `None` deaktiviert den Loop.
    pub fn set_loop(&mut self, region: Option<(f64, f64)>) -> Result<(), String> {
        if let Some((start, end)) = region {
            if !start.is_finite()
                || !end.is_finite()
                || start < 0.0
                || end <= start
                || end > self.duration_seconds
            {
                return Err("Loopbereich muss positiv und innerhalb der Songdauer liegen.".into());
            }
        }
        self.loop_region = region;
        Ok(())
    }

    /// Zweck: Verarbeitet vergangene Echtzeitdauer und berechnet Noten-/Lyrics-/Taktcursor.
    /// Eingaben: `elapsed_seconds` – vergangene Wandzeit seit dem letzten Tick.
    /// Ausgabe: Synchroner `PlaybackTick`; bei Loopüberschreitung wird der Cursor zurückgesetzt.
    pub fn advance(&mut self, elapsed_seconds: f64) -> Result<PlaybackTick, String> {
        if !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 {
            return Err("Playbackzeitfortschritt muss endlich und nichtnegativ sein.".into());
        }
        let mut wrapped = false;
        let mut ended = false;
        if self.playing {
            self.position_seconds += elapsed_seconds * f64::from(self.tempo_percent) / 100.0;
            if let Some((start, end)) = self.loop_region {
                if self.position_seconds >= end {
                    let span = end - start;
                    self.position_seconds =
                        start + (self.position_seconds - start).rem_euclid(span);
                    wrapped = true;
                }
            } else if self.position_seconds >= self.duration_seconds {
                self.position_seconds = self.duration_seconds;
                self.playing = false;
                ended = true;
            }
        }
        let active_midi_notes = self
            .notes
            .iter()
            .filter(|note| {
                self.position_seconds >= note.start_seconds()
                    && self.position_seconds < note.start_seconds() + note.duration_seconds()
            })
            .map(NoteEvent::midi)
            .collect();
        let highlighted_lyric = self
            .lyrics
            .iter()
            .find(|word| {
                self.position_seconds >= word.start_seconds()
                    && self.position_seconds < word.end_seconds()
            })
            .map(|word| word.text().to_string());
        let position = MusicalPosition::at(self.position_seconds, self.bpm, self.time_signature)
            .map_err(|error| error.to_string())?;
        Ok(PlaybackTick {
            position_seconds: self.position_seconds,
            position,
            active_midi_notes,
            highlighted_lyric,
            wrapped_loop: wrapped,
            ended,
        })
    }

    /// Zweck: Liefert den aktuellen Wiedergabecursor in Sekunden.
    /// Eingaben: `self` – Scheduler.
    /// Ausgabe: Nichtnegative Songzeit.
    pub fn position_seconds(&self) -> f64 {
        self.position_seconds
    }

    /// Zweck: Liefert den konfigurierten relativen Tempo-Prozentwert.
    /// Eingaben: `self` – Scheduler.
    /// Ausgabe: Wert zwischen 50 und 150.
    pub fn tempo_percent(&self) -> u16 {
        self.tempo_percent
    }

    /// Zweck: Meldet, ob Playback aktuell läuft.
    /// Eingaben: `self` – Scheduler.
    /// Ausgabe: `true` nach Play bis Pause, Stop oder Songende.
    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

/// Benannte Playback-Spuren für Mixer und Gain-Steuerung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrackKind {
    Original,
    Drums,
    Bass,
    Other,
    Vocals,
    GeneratedGuitar,
    Metronome,
}

/// Multiplikatorwerte pro Playback-Spur mit endlicher Gainprüfung.
#[derive(Debug, Clone, Default)]
pub struct TrackMixer {
    gains: BTreeMap<TrackKind, f32>,
    muted: BTreeMap<TrackKind, bool>,
}

impl TrackMixer {
    /// Zweck: Setzt den linearen Gainfaktor einer Spur zwischen Stille und doppelter Amplitude.
    /// Eingaben: Spur und endlicher `gain` im Bereich 0.0 bis 2.0.
    /// Ausgabe: `Ok(())` oder verständlicher Validierungsfehler.
    pub fn set_gain(&mut self, track: TrackKind, gain: f32) -> Result<(), String> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err("Spur-Gain muss endlich und zwischen 0 und 2 liegen.".into());
        }
        self.gains.insert(track, gain);
        Ok(())
    }

    /// Zweck: Schaltet eine Spur stumm oder aktiv.
    /// Eingaben: Spur und gewünschter Stummschaltungszustand.
    /// Ausgabe: Keine Rückgabe; speichert den Mute-Status.
    pub fn set_muted(&mut self, track: TrackKind, muted: bool) {
        self.muted.insert(track, muted);
    }

    /// Zweck: Liefert den wirksamen linearen Gain einer Spur.
    /// Eingaben: `track` – abzufragende Spur.
    /// Ausgabe: Null bei Mute, sonst gespeicherter Gain oder Standardwert 1.0.
    pub fn effective_gain(&self, track: TrackKind) -> f32 {
        if self.muted.get(&track).copied().unwrap_or(false) {
            0.0
        } else {
            self.gains.get(&track).copied().unwrap_or(1.0)
        }
    }
}

/// Zweck: Erzeugt einen kurzen abklingenden Metronom-Ton für die lokale Audioausgabe.
/// Eingaben: Zeit seit Beat in Sekunden, Sample-Rate und Downbeat-Markierung.
/// Ausgabe: Einzelnes PCM-Sample im Bereich -1 bis 1; außerhalb 55 ms ist der Wert null.
pub fn metronome_sample(seconds_since_beat: f64, sample_rate: u32, accented: bool) -> f32 {
    if !seconds_since_beat.is_finite()
        || seconds_since_beat < 0.0
        || sample_rate == 0
        || seconds_since_beat >= 0.055
    {
        return 0.0;
    }
    let frequency = if accented { 1_500.0 } else { 1_000.0 };
    let decay = (-seconds_since_beat * 65.0).exp() as f32;
    (std::f64::consts::TAU * frequency * seconds_since_beat).sin() as f32
        * decay
        * if accented { 0.7 } else { 0.45 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft Play/Pause/Stop, Tempoänderung und die synchronisierte Beat-/Lyrics-/Notenposition.
    /// Eingaben: Keine; verwendet Noten und ein Wort mit festen Zeitstempeln bei 120 BPM.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Tempo, Cursor, Noten und Lyricswort.
    #[test]
    fn playback_advances_and_synchronizes_events() {
        let note = NoteEvent::new(60, 0.5, 1.0).unwrap();
        let lyric = LyricsWord::new("fire", 0.5, 0.9).unwrap();
        let mut player = PlaybackScheduler::new(
            3.0,
            120.0,
            TimeSignature::default(),
            vec![note],
            vec![lyric],
        )
        .unwrap();
        player.set_tempo_percent(50).unwrap();
        player.play();
        let tick = player.advance(1.0).unwrap();
        assert_eq!(tick.position_seconds, 0.5);
        assert_eq!(tick.active_midi_notes, vec![60]);
        assert_eq!(tick.highlighted_lyric.as_deref(), Some("fire"));
        player.pause();
        assert_eq!(player.advance(1.0).unwrap().position_seconds, 0.5);
        player.stop();
        assert_eq!(player.position_seconds(), 0.0);
    }

    /// Zweck: Prüft Loop-Wiederholung, ungültige Loopgrenzen, Mixer-Gain und Metronombegrenzung.
    /// Eingaben: Keine; erstellt einen kurzen Scheduler und ändert Loop-/Mixerzustände.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Wrap, Lautstärkegrenzen und Click-Abklingen.
    #[test]
    fn playback_loops_and_controls_mixer() {
        let mut player =
            PlaybackScheduler::new(4.0, 120.0, TimeSignature::default(), vec![], vec![]).unwrap();
        player.set_loop(Some((1.0, 2.0))).unwrap();
        player.play();
        assert_eq!(player.position_seconds(), 1.0);
        let tick = player.advance(1.2).unwrap();
        assert!(tick.wrapped_loop);
        assert!((tick.position_seconds - 1.2).abs() < 1e-9);
        assert!(player.set_loop(Some((3.0, 2.0))).is_err());
        let mut mixer = TrackMixer::default();
        mixer.set_gain(TrackKind::Vocals, 0.5).unwrap();
        assert_eq!(mixer.effective_gain(TrackKind::Vocals), 0.5);
        mixer.set_muted(TrackKind::Vocals, true);
        assert_eq!(mixer.effective_gain(TrackKind::Vocals), 0.0);
        assert_eq!(metronome_sample(0.06, 48_000, false), 0.0);
    }
}
