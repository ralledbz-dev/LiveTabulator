/*
===============================================================================
Division By Zeros (DBZS) - Native Playback-Ausgabe
Modul: playback_output
Zweck: Gibt eine lokal dekodierte WAV- oder MP3-Datei plus Metronom über den nativen
       Standard-Audioausgang aus, unabhängig vom Slint- und Analyse-Thread.
Sprache: Rust
Hinweise:
- CPAL hält den nativen Audiocallback; Samples werden aus einem unveränderlichen Arc gelesen.
- Tempo, Pause, Loop und Abbruch laufen atomar; der Audiocallback sperrt keine Mutex.
- Die musikalische Cursorposition kommt zusätzlich vom Core-PlaybackScheduler.
===============================================================================
*/
use livetab_core::audio::WaveFile;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// Höchstbudget für f32-Samples aller parallel verfügbaren synchronen WAV-/MP3-Quellen.
const MAX_SYNCHRONIZED_DECODED_BYTES: usize = 768 * 1024 * 1024;

/// Laufende native Wiedergabe, steuerbar ohne Zugriff auf den plattformspezifischen Stream.
pub struct PlaybackOutputSession {
    state: Arc<PlaybackState>,
    shutdown: Arc<AtomicBool>,
}

impl PlaybackOutputSession {
    /// Zweck: Öffnet den Standardausgang für eine einzelne lokale WAV-/MP3-Quelle.
    /// Eingaben: Dekodierte lokale WAV-Datei; Ausgabegerät/-format stammen vom Audiohost.
    /// Ausgabe: Kontrollhandle oder ein klarer Geräte-/Format-/Streamfehler.
    pub fn start(wave: WaveFile) -> Result<Self, String> {
        Self::start_sources(vec![("source".into(), wave)], "source")
    }

    /// Zweck: Startet einen nativen Stream mit umschaltbaren, zeitlich ausgerichteten WAV-Quellen.
    /// Eingaben: `(id, WAV-/MP3-)`-Quellen und die anfänglich aktive ID; maximal sechs Quellen.
    /// Ausgabe: Kontrollhandle oder ein Validierungs-/Geräte-/Streamfehler.
    pub fn start_sources(
        sources: Vec<(String, WaveFile)>,
        initial_source_id: &str,
    ) -> Result<Self, String> {
        if sources.is_empty() || sources.len() > 6 {
            return Err("Playback benötigt eine bis sechs Audioquellen.".into());
        }
        let decoded_bytes = sources.iter().fold(0usize, |total, (_, wave)| {
            total.saturating_add(
                wave.samples
                    .len()
                    .saturating_mul(std::mem::size_of::<f32>()),
            )
        });
        validate_multitrack_buffer_budget(sources.len(), decoded_bytes)?;
        let mut ids = std::collections::HashSet::new();
        let mut durations = Vec::with_capacity(sources.len());
        let mut active_index = None;
        for (index, (id, wave)) in sources.iter().enumerate() {
            if id.is_empty() || !ids.insert(id.clone()) {
                return Err("Playback-Quellen benötigen eindeutige IDs.".into());
            }
            if wave.format.sample_rate == 0 || wave.format.channels == 0 || wave.frame_count() == 0
            {
                return Err(format!(
                    "Playback-Audioquelle `{id}` ist leer oder ungültig."
                ));
            }
            durations.push(wave.duration_seconds());
            if id == initial_source_id {
                active_index = Some(index);
            }
        }
        if durations
            .iter()
            .any(|duration| (*duration - durations[0]).abs() > 0.020)
        {
            return Err("Playback-Quellen sind nicht innerhalb von 20 ms synchronisiert.".into());
        }
        let active_index =
            active_index.ok_or_else(|| format!("Playback-Quelle `{initial_source_id}` fehlt."))?;
        let state = Arc::new(PlaybackState::new_sources(sources, active_index));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_shutdown = Arc::clone(&shutdown);
        let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let worker_ready_sender = ready_sender.clone();
        thread::Builder::new()
            .name("livetab-native-playback".into())
            .spawn(move || {
                let result = run_output_stream(
                    Arc::clone(&worker_state),
                    Arc::clone(&worker_shutdown),
                    worker_ready_sender.clone(),
                );
                if let Err(error) = result {
                    let _ = worker_ready_sender.try_send(Err(error));
                }
            })
            .map_err(|error| format!("Playback-Worker konnte nicht gestartet werden: {error}"))?;
        match ready_receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                shutdown.store(true, Ordering::Release);
                return Err(error);
            }
            Err(error) => {
                shutdown.store(true, Ordering::Release);
                return Err(format!(
                    "Nativer Audioausgang meldete sich nicht rechtzeitig: {error}"
                ));
            }
        }
        Ok(Self { state, shutdown })
    }

    /// Zweck: Wechselt auf eine dekodierte Original- oder Stem-WAV, ohne den Playhead zurückzusetzen.
    /// Eingaben: `source_id` – stabile Projektkennung der gewünschten Quelle.
    /// Ausgabe: `Ok(())` bei geladener Quelle oder ein Auswahlfehler.
    pub fn select_source(&self, source_id: &str) -> Result<(), String> {
        self.state.select_source(source_id)
    }

    /// Zweck: Setzt die Wiedergabe auf Pause, erhält aber die aktuelle Audioausgabeposition.
    /// Eingaben: `self` – laufende Ausgabe.
    /// Ausgabe: Keine; Audiocallback gibt bis zum Fortsetzen Stille aus.
    pub fn pause(&self) {
        self.state.playing.store(false, Ordering::Release);
    }

    /// Zweck: Setzt die Wiedergabe fort.
    /// Eingaben: `self` – geöffnete Audioausgabe.
    /// Ausgabe: Keine; der Stream setzt die Ausgabe an der gehaltenen Position fort.
    pub fn resume(&self) {
        self.state.playing.store(true, Ordering::Release);
    }

    /// Zweck: Stoppt die Ausgabe und setzt die Audioquelle an ihren Anfang zurück.
    /// Eingaben: `self` – geöffnete Audioausgabe.
    /// Ausgabe: Keine; nächste Wiedergabe startet bei null Sekunden.
    pub fn stop(&self) {
        self.state.playing.store(false, Ordering::Release);
        self.state
            .position_bits
            .store(0.0f64.to_bits(), Ordering::Release);
    }

    /// Zweck: Setzt die atomare Quellposition des nativen Sampleplayers ohne Callback-Sperre.
    /// Eingaben: Endliche Zielsekunden innerhalb der Audiodauer und des optionalen Loopbereichs.
    /// Ausgabe: `Ok(())` oder Validierungsfehler; laufende Ausgabe setzt am nächsten Frame dort fort.
    pub fn seek(&self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() || seconds < 0.0 || seconds > self.state.duration_seconds() {
            return Err("Playback-Seek muss innerhalb der Audiodauer liegen.".into());
        }
        let loop_end = f64::from_bits(self.state.loop_end_bits.load(Ordering::Acquire));
        if loop_end > 0.0 {
            let loop_start = f64::from_bits(self.state.loop_start_bits.load(Ordering::Acquire));
            if seconds < loop_start || seconds >= loop_end {
                return Err("Playback-Seek liegt außerhalb des aktiven Loopbereichs.".into());
            }
        }
        self.state.position_bits.store(seconds.to_bits(), Ordering::Release);
        Ok(())
    }

    /// Zweck: Setzt das relative Wiedergabetempo des Sampleplayers.
    /// Eingaben: Prozentwert 50 bis 150 gegenüber der Quellgeschwindigkeit.
    /// Ausgabe: `Ok(())` oder eine verständliche Bereichsmeldung.
    pub fn set_tempo_percent(&self, percent: u16) -> Result<(), String> {
        if !(50..=150).contains(&percent) {
            return Err("Playbacktempo muss zwischen 50 und 150 Prozent liegen.".into());
        }
        self.state
            .tempo_bits
            .store((f32::from(percent) / 100.0).to_bits(), Ordering::Release);
        Ok(())
    }

    /// Zweck: Setzt oder deaktiviert die Endpunkt-Loopgrenzen.
    /// Eingaben: `range` – `Some((start,end))` in Sekunden oder `None` für einmalige Wiedergabe.
    /// Ausgabe: `Ok(())` oder ein Hinweis bei ungültigen Zeitgrenzen.
    pub fn set_loop(&self, range: Option<(f64, f64)>) -> Result<(), String> {
        match range {
            Some((start, end))
                if start.is_finite()
                    && end.is_finite()
                    && start >= 0.0
                    && end > start
                    && end <= self.state.duration_seconds() =>
            {
                self.state
                    .loop_start_bits
                    .store(start.to_bits(), Ordering::Release);
                self.state
                    .loop_end_bits
                    .store(end.to_bits(), Ordering::Release);
            }
            Some(_) => {
                return Err("Audio-Loop muss innerhalb der positiven Songdauer liegen.".into());
            }
            None => {
                self.state
                    .loop_end_bits
                    .store(0.0f64.to_bits(), Ordering::Release);
            }
        }
        Ok(())
    }

    /// Zweck: Ändert das Metronom und dessen Taktart für künftige Ausgabezyklen.
    /// Eingaben: `enabled` – Click an/aus; `bpm` – Viertel pro Minute; `beats_per_bar` – Zähler des Takts.
    /// Ausgabe: Keine; nichtendliche/ungültige BPM werden ignoriert und bleiben stumm.
    pub fn configure_metronome(&self, enabled: bool, bpm: f64, beats_per_bar: u8) {
        let valid_bpm = if bpm.is_finite() {
            bpm.clamp(30.0, 300.0) as f32
        } else {
            0.0
        };
        self.state
            .metronome_bpm_bits
            .store(valid_bpm.to_bits(), Ordering::Release);
        self.state
            .beats_per_bar
            .store(usize::from(beats_per_bar.max(1)), Ordering::Release);
        self.state
            .metronome_enabled
            .store(enabled && valid_bpm > 0.0, Ordering::Release);
    }

    /// Zweck: Liefert die seit dem Wiedergabestart erreichte Quellposition.
    /// Eingaben: `self` – laufende Ausgabe.
    /// Ausgabe: Nichtnegative Songzeit in Sekunden.
    pub fn position_seconds(&self) -> f64 {
        f64::from_bits(self.state.position_bits.load(Ordering::Acquire)).max(0.0)
    }
}

impl Drop for PlaybackOutputSession {
    /// Zweck: Fordert beim Verwerfen der Ausgabe das Freigeben des nativen Streams an.
    /// Eingaben: `self` – zu verwerfendes Handle.
    /// Ausgabe: Keine; der Streamthread beendet sich am nächsten kurzen Pollintervall.
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

struct PlaybackState {
    sources: Vec<PlaybackSource>,
    active_source_index: AtomicUsize,
    position_bits: AtomicU64,
    loop_start_bits: AtomicU64,
    loop_end_bits: AtomicU64,
    tempo_bits: AtomicU32,
    metronome_bpm_bits: AtomicU32,
    beats_per_bar: AtomicUsize,
    metronome_enabled: AtomicBool,
    playing: AtomicBool,
}

struct PlaybackSource {
    id: String,
    wave: Arc<WaveFile>,
}

impl PlaybackState {
    /// Zweck: Erstellt den atomaren Audiozustand mit pausierter Ausgabe und deaktiviertem Metronom.
    /// Eingaben: Validierte WAV-/MP3-Quelle.
    /// Ausgabe: Thread-sicherer Zustand für das native Audiogerät.
    fn new(wave: WaveFile) -> Self {
        Self::new_sources(vec![("source".into(), wave)], 0)
    }

    /// Zweck: Erstellt den atomaren Zustand aus einer Quellenliste und einem gültigen Index.
    /// Eingaben: `(id, WAV-/MP3-)`-Quellen sowie Index der anfänglich aktiven WAV-/MP3-Quelle.
    /// Ausgabe: Pausierter Playbackzustand mit gemeinsamem Zeit-/Loop-/Tempo-Cursor.
    fn new_sources(sources: Vec<(String, WaveFile)>, active_source_index: usize) -> Self {
        Self {
            sources: sources
                .into_iter()
                .map(|(id, wave)| PlaybackSource {
                    id,
                    wave: Arc::new(wave),
                })
                .collect(),
            active_source_index: AtomicUsize::new(active_source_index),
            position_bits: AtomicU64::new(0.0f64.to_bits()),
            loop_start_bits: AtomicU64::new(0.0f64.to_bits()),
            loop_end_bits: AtomicU64::new(0.0f64.to_bits()),
            tempo_bits: AtomicU32::new(1.0f32.to_bits()),
            metronome_bpm_bits: AtomicU32::new(120.0f32.to_bits()),
            beats_per_bar: AtomicUsize::new(4),
            metronome_enabled: AtomicBool::new(true),
            playing: AtomicBool::new(false),
        }
    }

    /// Zweck: Gibt die Audiodauer aus Sampleframes und Sample-Rate zurück.
    /// Eingaben: `self` – immutable WAV-Quelle.
    /// Ausgabe: Positive oder nichtnegative Dauer in Sekunden.
    fn duration_seconds(&self) -> f64 {
        self.sources
            .get(self.active_source_index.load(Ordering::Acquire))
            .map(|source| source.wave.duration_seconds())
            .unwrap_or(0.0)
    }

    /// Zweck: Wählt eine bereits dekodierte Quelle und erhält die gemeinsame Audiozeit.
    /// Eingaben: `id` – Original- oder Stem-Kennung.
    /// Ausgabe: `Ok(())` bei vorhandenem Buffer oder ein Fehler bei unbekannter Kennung.
    fn select_source(&self, id: &str) -> Result<(), String> {
        let index = self
            .sources
            .iter()
            .position(|source| source.id == id)
            .ok_or_else(|| format!("Playback-Quelle `{id}` ist nicht geladen."))?;
        self.active_source_index.store(index, Ordering::Release);
        Ok(())
    }

    /// Zweck: Rendert einen einzelnen Outputkanal und verschiebt die Audiozeit einmal pro Outputframe.
    /// Eingaben: Native Ausgaberate, Outputkanalzahl und nullbasierter Kanalindex.
    /// Ausgabe: Endlicher Original-/Metronommixsample; bei Pause/Auslauf Stille.
    fn render_frame(&self, output_rate: u32, output_channels: usize, output_channel: usize) -> f32 {
        if output_rate == 0
            || output_channels == 0
            || output_channel >= output_channels
            || !self.playing.load(Ordering::Acquire)
        {
            return 0.0;
        }
        let duration = self.duration_seconds();
        let mut position = f64::from_bits(self.position_bits.load(Ordering::Relaxed));
        let loop_start = f64::from_bits(self.loop_start_bits.load(Ordering::Acquire));
        let loop_end = f64::from_bits(self.loop_end_bits.load(Ordering::Acquire));
        if loop_end > loop_start && position >= loop_end {
            position = loop_start + (position - loop_start) % (loop_end - loop_start);
        }
        if position >= duration {
            self.playing.store(false, Ordering::Release);
            self.position_bits
                .store(duration.to_bits(), Ordering::Release);
            return 0.0;
        }
        let source = self
            .sources
            .get(self.active_source_index.load(Ordering::Acquire))
            .unwrap_or(&self.sources[0]);
        let wave = &source.wave;
        let channels = usize::from(wave.format.channels);
        let frames = wave.frame_count();
        let source_position = position * f64::from(wave.format.sample_rate);
        let left_index = (source_position.floor() as usize).min(frames.saturating_sub(1));
        let right_index = (left_index + 1).min(frames.saturating_sub(1));
        let fraction = (source_position - left_index as f64) as f32;
        let mut original = 0.0f32;
        if output_channels == 1 && channels > 1 {
            for channel in 0..channels {
                original += interpolate_sample(
                    &wave.samples,
                    channels,
                    left_index,
                    right_index,
                    channel,
                    fraction,
                );
            }
            original /= channels as f32;
        } else {
            let source_channel = if channels == 1 {
                0
            } else {
                output_channel.min(channels - 1)
            };
            original = interpolate_sample(
                &wave.samples,
                channels,
                left_index,
                right_index,
                source_channel,
                fraction,
            );
        }
        let bpm = f32::from_bits(self.metronome_bpm_bits.load(Ordering::Relaxed));
        let click = if self.metronome_enabled.load(Ordering::Relaxed) && bpm > 0.0 {
            let beat_length = 60.0 / f64::from(bpm);
            let beat_index = (position / beat_length).floor() as usize;
            let within_beat = position - beat_index as f64 * beat_length;
            let click_duration = 0.022;
            if within_beat < click_duration {
                let accent = if beat_index % self.beats_per_bar.load(Ordering::Relaxed).max(1) == 0
                {
                    0.35
                } else {
                    0.2
                };
                (std::f64::consts::TAU * 1760.0 * within_beat).sin() as f32
                    * (1.0 - (within_beat / click_duration) as f32)
                    * accent
            } else {
                0.0
            }
        } else {
            0.0
        };
        let sample = (original + click).clamp(-1.0, 1.0);
        let speed =
            f64::from(f32::from_bits(self.tempo_bits.load(Ordering::Relaxed)).clamp(0.5, 1.5));
        let next = position + speed / f64::from(output_rate);
        let next = if loop_end > loop_start && next >= loop_end {
            loop_start + (next - loop_end) % (loop_end - loop_start)
        } else {
            next
        };
        if output_channel + 1 == output_channels {
            self.position_bits
                .store(next.min(duration).to_bits(), Ordering::Release);
        }
        sample
    }
}

/// Zweck: Liest einen oder mehrere WAV-Kanäle mit linearer Interpolation.
/// Eingaben: Interleaved Samplefolge, Kanalzahl, linkes/rechtes Frame, Kanalindex und Bruchanteil.
/// Ausgabe: Ein normierter Samplewert; ungültige Indizes liefern Stille.
fn interpolate_sample(
    samples: &[f32],
    channels: usize,
    left: usize,
    right: usize,
    channel: usize,
    fraction: f32,
) -> f32 {
    if channels == 0 || channel >= channels {
        return 0.0;
    }
    let first = samples
        .get(left.saturating_mul(channels).saturating_add(channel))
        .copied()
        .unwrap_or(0.0);
    let second = samples
        .get(right.saturating_mul(channels).saturating_add(channel))
        .copied()
        .unwrap_or(first);
    first + (second - first) * fraction.clamp(0.0, 1.0)
}

/// Zweck: Verhindert, dass synchronisierte WAV-Quellen einen unbegrenzten f32-Puffer anlegen.
/// Eingaben: `source_count` – Anzahl parallel geladener WAVs; `decoded_bytes` – deren Sample-Speicher.
/// Ausgabe: `Ok(())` innerhalb des Budgets oder eine klare Meldung bei zu großem Mehrspurset.
fn validate_multitrack_buffer_budget(
    source_count: usize,
    decoded_bytes: usize,
) -> Result<(), String> {
    if source_count > 1 && decoded_bytes > MAX_SYNCHRONIZED_DECODED_BYTES {
        return Err(
            "Original und Stem-WAVs benötigen zusammen mehr als 768 MiB Audiospeicher; laden Sie kürzere oder weniger Quellen."
                .into(),
        );
    }
    Ok(())
}

/// Zweck: Öffnet und hält den nativen Standardausgabestream, bis das Handle beendet wird.
/// Eingaben: Shared Playbackstate und atomarer Shutdownmarker.
/// Ausgabe: `Ok(())` beim kontrollierten Beenden oder verständlicher Host-/Streamfehler.
fn run_output_stream(
    state: Arc<PlaybackState>,
    shutdown: Arc<AtomicBool>,
    ready_sender: std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "Kein nativer Standardaudioausgang verfügbar.".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|error| format!("Ausgabeformat konnte nicht ermittelt werden: {error}"))?;
    let config: cpal::StreamConfig = supported.clone().into();
    let sample_rate = config.sample_rate.0;
    let channels = usize::from(config.channels);
    let error_callback = |error| eprintln!("Lokaler Playback-Ausgabefehler: {error}");
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => { let state = Arc::clone(&state); device.build_output_stream(&config, move |data: &mut [f32], _| fill_f32(data, sample_rate, channels, &state), error_callback, None) }
        cpal::SampleFormat::I16 => { let state = Arc::clone(&state); device.build_output_stream(&config, move |data: &mut [i16], _| fill_i16(data, sample_rate, channels, &state), error_callback, None) }
        cpal::SampleFormat::U16 => { let state = Arc::clone(&state); device.build_output_stream(&config, move |data: &mut [u16], _| fill_u16(data, sample_rate, channels, &state), error_callback, None) }
        format => return Err(format!("Das native Ausgabeformat {format:?} wird für sichere PCM-Ausgabe nicht unterstützt.")),
    }.map_err(|error| format!("Nativem Ausgabestream konnte nicht erstellt werden: {error}"))?;
    stream
        .play()
        .map_err(|error| format!("Nativem Ausgabestream konnte nicht gestartet werden: {error}"))?;
    ready_sender
        .send(Ok(()))
        .map_err(|_| "Playback-Start wurde vom Aufrufer abgebrochen.".to_string())?;
    while !shutdown.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    Ok(())
}

/// Zweck: Füllt den f32-Ausgabepuffer mit Quelle und Metronom, ohne Locks anzufordern.
/// Eingaben: Mutabler Gerätepuffer, Ausgaberate/-kanäle und atomarer Playbackstate.
/// Ausgabe: Keine; jeder Samplewert ist endlich und auf den normierten Bereich begrenzt.
fn fill_f32(output: &mut [f32], sample_rate: u32, channels: usize, state: &PlaybackState) {
    if channels == 0 {
        output.fill(0.0);
        return;
    }
    for frame in output.chunks_exact_mut(channels) {
        for (channel, sample) in frame.iter_mut().enumerate() {
            *sample = state.render_frame(sample_rate, channels, channel);
        }
    }
    let complete = output.len() / channels * channels;
    output[complete..].fill(0.0);
}

/// Zweck: Füllt den i16-Ausgabepuffer mit konvertiertem normiertem PCM.
/// Eingaben: Mutabler Gerätepuffer, Ausgaberate/-kanäle und atomarer Playbackstate.
/// Ausgabe: Keine; Werte liegen im gültigen signed-16-bit-Bereich.
fn fill_i16(output: &mut [i16], sample_rate: u32, channels: usize, state: &PlaybackState) {
    if channels == 0 {
        output.fill(0);
        return;
    }
    for frame in output.chunks_exact_mut(channels) {
        for (channel, sample) in frame.iter_mut().enumerate() {
            *sample = (state.render_frame(sample_rate, channels, channel) * i16::MAX as f32).round()
                as i16;
        }
    }
    let complete = output.len() / channels * channels;
    output[complete..].fill(0);
}

/// Zweck: Füllt den u16-Ausgabepuffer mit verschobenem normiertem PCM.
/// Eingaben: Mutabler Gerätepuffer, Ausgaberate/-kanäle und atomarer Playbackstate.
/// Ausgabe: Keine; Werte liegen im gültigen unsigned-16-bit-Bereich.
fn fill_u16(output: &mut [u16], sample_rate: u32, channels: usize, state: &PlaybackState) {
    if channels == 0 {
        output.fill(u16::MAX / 2);
        return;
    }
    for frame in output.chunks_exact_mut(channels) {
        for (channel, sample) in frame.iter_mut().enumerate() {
            *sample = ((state
                .render_frame(sample_rate, channels, channel)
                .clamp(-1.0, 1.0)
                * 0.5
                + 0.5)
                * u16::MAX as f32)
                .round() as u16;
        }
    }
    let complete = output.len() / channels * channels;
    output[complete..].fill(u16::MAX / 2);
}

#[cfg(test)]
mod tests {
    use super::*;
    use livetab_core::audio::AudioFormat;

    /// Zweck: Erstellt ein kurzes deterministisches Testsignal.
    /// Eingaben: Keine; verwendet vier bekannte Mono-PCM-Werte bei 1 kHz.
    /// Ausgabe: WAV-Testquelle ohne Datei-I/O.
    fn test_wave() -> WaveFile {
        WaveFile {
            format: AudioFormat {
                sample_rate: 1000,
                channels: 1,
                bits_per_sample: 32,
                is_float: true,
            },
            samples: vec![0.25, 0.5, -0.25, 0.75],
        }
    }

    /// Zweck: Prüft Pause/Resume und die zeitbasierte inkrementelle Audioausgabe.
    /// Eingaben: Keine; testet den internen Renderer ohne echtes Hardwaregerät.
    /// Ausgabe: Keine; Assertions sichern Stille bei Pause und Samplefolge bei Play.
    #[test]
    fn renderer_pauses_and_resumes_without_device() {
        let state = PlaybackState::new(test_wave());
        state.metronome_enabled.store(false, Ordering::Release);
        assert_eq!(state.render_frame(1000, 1, 0), 0.0);
        state.playing.store(true, Ordering::Release);
        assert!((state.render_frame(1000, 1, 0) - 0.25).abs() < 1e-6);
        assert!((state.render_frame(1000, 1, 0) - 0.5).abs() < 1e-6);
        state.playing.store(false, Ordering::Release);
        assert_eq!(state.render_frame(1000, 1, 0), 0.0);
    }

    /// Zweck: Prüft, dass ein gesetzter Loop das Ende ohne Cursorüberschreitung wiederholt.
    /// Eingaben: Keine; setzt einen 1- bis 3-ms-Bereich bei 1 kHz.
    /// Ausgabe: Keine; nach dem Endpunkt startet die Position wieder im Loopbereich.
    #[test]
    fn renderer_wraps_inside_loop_bounds() {
        let state = PlaybackState::new(test_wave());
        state.metronome_enabled.store(false, Ordering::Release);
        state.playing.store(true, Ordering::Release);
        state
            .position_bits
            .store(0.002f64.to_bits(), Ordering::Release);
        state
            .loop_start_bits
            .store(0.001f64.to_bits(), Ordering::Release);
        state
            .loop_end_bits
            .store(0.003f64.to_bits(), Ordering::Release);
        let _ = state.render_frame(1000, 1, 0);
        assert!(f64::from_bits(state.position_bits.load(Ordering::Acquire)) < 0.003);
    }

    /// Zweck: Prüft die lineare Sampleinterpolation zwischen benachbarten Quellframes.
    /// Eingaben: Keine; verwendet die deterministische Testwelle bei halber Frameposition.
    /// Ausgabe: Keine; interpolierter Wert liegt exakt zwischen den beiden Quellwerten.
    #[test]
    fn interpolation_uses_source_fraction() {
        let samples = [0.0, 1.0];
        assert!((interpolate_sample(&samples, 1, 0, 1, 0, 0.5) - 0.5).abs() < 1e-6);
    }

    /// Zweck: Prüft, dass Stereoquellen ihren linken und rechten Kanal getrennt ausgeben.
    /// Eingaben: Keine; rendert ein einzelnes Stereoframe ohne Metronom.
    /// Ausgabe: Keine; Assertions sichern Kanalreihenfolge und identische Zeitfortschreibung.
    #[test]
    fn renderer_preserves_stereo_channels() {
        let wave = WaveFile {
            format: AudioFormat {
                sample_rate: 1000,
                channels: 2,
                bits_per_sample: 32,
                is_float: true,
            },
            samples: vec![0.25, -0.5, 0.75, -0.25],
        };
        let state = PlaybackState::new(wave);
        state.metronome_enabled.store(false, Ordering::Release);
        state.playing.store(true, Ordering::Release);
        assert!((state.render_frame(1000, 2, 0) - 0.25).abs() < 1e-6);
        assert!((state.render_frame(1000, 2, 1) + 0.5).abs() < 1e-6);
        assert!((f64::from_bits(state.position_bits.load(Ordering::Acquire)) - 0.001).abs() < 1e-9);
    }

    /// Zweck: Prüft Quellenwechsel zwischen Original und Stem bei erhaltenem gemeinsamem Playhead.
    /// Eingaben: Keine; beide künstlichen Mono-WAVs haben identische Zeit- und Formatmetadaten.
    /// Ausgabe: Keine; der Cursor bleibt beim Wechsel erhalten und die neue Quelle wird hörbar.
    #[test]
    fn source_switch_preserves_shared_playhead() {
        let stem = WaveFile {
            format: AudioFormat {
                sample_rate: 1000,
                channels: 1,
                bits_per_sample: 32,
                is_float: true,
            },
            samples: vec![0.9, 0.75, 0.5, 0.25],
        };
        let state = PlaybackState::new_sources(
            vec![("source".into(), test_wave()), ("stem-vocals".into(), stem)],
            0,
        );
        state.metronome_enabled.store(false, Ordering::Release);
        state.playing.store(true, Ordering::Release);
        assert!((state.render_frame(1000, 1, 0) - 0.25).abs() < 1e-6);
        let before_switch = state.position_bits.load(Ordering::Acquire);
        state.select_source("stem-vocals").unwrap();
        assert_eq!(state.position_bits.load(Ordering::Acquire), before_switch);
        assert!((state.render_frame(1000, 1, 0) - 0.75).abs() < 1e-6);
        assert!((f64::from_bits(state.position_bits.load(Ordering::Acquire)) - 0.002).abs() < 1e-9);
        assert!(state.select_source("stem-missing").is_err());
    }

    /// Zweck: Prüft die Speichergrenze für mehrere dekodierte Playbackquellen ohne große Allokation.
    /// Eingaben: Keine; verwendet nur Grenzwerte der Validierungsfunktion.
    /// Ausgabe: Keine; Einzeltracks bleiben unberührt und übergroße Mehrspursets werden abgelehnt.
    #[test]
    fn multitrack_buffer_budget_is_bounded() {
        assert!(validate_multitrack_buffer_budget(5, MAX_SYNCHRONIZED_DECODED_BYTES).is_ok());
        assert!(validate_multitrack_buffer_budget(5, MAX_SYNCHRONIZED_DECODED_BYTES + 1).is_err());
        assert!(validate_multitrack_buffer_budget(1, MAX_SYNCHRONIZED_DECODED_BYTES + 1).is_ok());
    }
}
