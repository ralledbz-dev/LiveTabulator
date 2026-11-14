/*
===============================================================================
Division By Zeros (DBZS) - Audioaufnahme-Codierung
Modul: audio_encoding
Zweck: Schreibt abgeschlossene lokale Mono-Capture-Samples nach ausdrücklicher
       Endungsauswahl als PCM-WAV oder lokal mit LAME als MP3.
Sprache: Rust
Hinweise:
- Es wird keine Aufnahme gestartet; die Funktionen verarbeiten nur übergebene Samples.
- MP3 verwendet den gebündelten LAME-Encoder und erzeugt 192-kbit/s-CBR-Mono.
===============================================================================
*/
use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, MonoPcm, Quality};
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Gewähltes Ausgabeformat einer abgeschlossenen lokalen Liveaufnahme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingFormat {
    /// Unkomprimiertes PCM mit vorhandener WAV-Kompatibilität.
    Wav,
    /// Lokal mit dem gebündelten LAME-Encoder erzeugtes MPEG Layer III.
    Mp3,
}

impl RecordingFormat {
    /// Zweck: Liefert das im UI und in Jobmeldungen angezeigte Formatkürzel.
    /// Eingaben: `self` – das über den Zieldateinamen gewählte Format.
    /// Ausgabe: Statisches Kürzel `WAV` oder `MP3`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Wav => "WAV",
            Self::Mp3 => "MP3",
        }
    }
}

/// Zweck: Ermittelt ausschließlich anhand einer `.wav`- oder `.mp3`-Endung das Aufnahmeformat.
/// Eingaben: `path` – vom nativen Dateidialog zurückgegebener Speicherpfad.
/// Ausgabe: `Some` für unterstützte Endungen (ohne Beachtung der Groß-/Kleinschreibung), sonst `None`.
pub(crate) fn recording_format(path: &Path) -> Option<RecordingFormat> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("wav") => Some(RecordingFormat::Wav),
        Some(extension) if extension.eq_ignore_ascii_case("mp3") => Some(RecordingFormat::Mp3),
        _ => None,
    }
}

/// Zweck: Schreibt abgeschlossene Mono-Capture-Samples gemäß echter Dateiendung als WAV oder MP3.
/// Eingaben: `path` – `.wav`-/`.mp3`-Ziel; `samples` – Mono-Samples; `sample_rate` – Hz; `progress` – Messcallback.
/// Ausgabe: `Ok(())` nach gültigem lokalen Audioexport oder eine verständliche Format-/Codec-/Dateimeldung.
pub(crate) fn write_recording_audio<F>(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    mut progress: F,
) -> Result<(), String>
where
    F: FnMut(f32),
{
    match recording_format(path) {
        Some(RecordingFormat::Wav) => livetab_core::audio::write_wav_with_progress(
            path,
            samples,
            sample_rate,
            1,
            &mut progress,
        )
        .map_err(|error| error.to_string()),
        Some(RecordingFormat::Mp3) => {
            write_mp3_recording(path, samples, sample_rate, &mut progress)
        }
        None => {
            Err("Aufnahmen können nur mit `.wav`- oder `.mp3`-Endung gespeichert werden.".into())
        }
    }
}

/// Zweck: Kodiert Mono-Floats mit LAME und meldet Sample-Encode plus tatsächlichen MP3-Dateischreibfortschritt.
/// Eingaben: `.mp3`-Zielpfad, nichtleere Mono-Samples, unterstützte Rate und Fortschrittscallback.
/// Ausgabe: `Ok(())` nach vollständigem 192-kbit/s-CBR-Export oder ein Codec-/Dateifehler ohne WAV-Falschdeklaration.
fn write_mp3_recording<F>(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(f32),
{
    if samples.is_empty() {
        return Err("Eine leere Audioaufnahme kann nicht als MP3 kodiert werden.".into());
    }
    let mut builder =
        Builder::new().ok_or_else(|| "LAME-Encoder konnte nicht angelegt werden.".to_string())?;
    builder
        .set_num_channels(1)
        .map_err(|error| format!("LAME-Monoeinstellung fehlgeschlagen: {error}"))?;
    builder.set_sample_rate(sample_rate).map_err(|error| {
        format!("LAME unterstützt die Capture-Sample-Rate {sample_rate} Hz nicht: {error}")
    })?;
    builder
        .set_brate(Bitrate::Kbps192)
        .map_err(|error| format!("LAME-192-kbit/s-Einstellung fehlgeschlagen: {error}"))?;
    builder
        .set_quality(Quality::Best)
        .map_err(|error| format!("LAME-Qualitätseinstellung fehlgeschlagen: {error}"))?;
    let mut encoder = builder
        .build()
        .map_err(|error| format!("LAME-Encoder konnte nicht initialisiert werden: {error}"))?;

    const ENCODE_CHUNK_SAMPLES: usize = 1152 * 8;
    let mut output = Vec::<u8>::new();
    let mut samples_encoded = 0_usize;
    let mut last_progress = 0.0_f32;
    for chunk in samples.chunks(ENCODE_CHUNK_SAMPLES) {
        let cleaned = chunk
            .iter()
            .map(|sample| {
                if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        output.reserve(mp3lame_encoder::max_required_buffer_size(cleaned.len()));
        let written = encoder
            .encode(MonoPcm(&cleaned), output.spare_capacity_mut())
            .map_err(|error| format!("LAME-MP3-Kodierung fehlgeschlagen: {error}"))?;
        append_initialized_output(&mut output, written)?;
        samples_encoded += chunk.len();
        let fraction = 0.9 * samples_encoded as f32 / samples.len() as f32;
        if fraction - last_progress >= 0.005 {
            progress(fraction);
            last_progress = fraction;
        }
    }
    output.reserve(8192);
    let flushed = encoder
        .flush::<FlushNoGap>(output.spare_capacity_mut())
        .map_err(|error| format!("LAME-MP3-Abschluss fehlgeschlagen: {error}"))?;
    append_initialized_output(&mut output, flushed)?;
    if output.is_empty() {
        return Err("LAME hat keine MP3-Audiodaten erzeugt.".into());
    }
    let mut file = File::create(path)
        .map_err(|error| format!("MP3-Datei konnte nicht angelegt werden: {error}"))?;
    let mut bytes_written = 0_usize;
    for chunk in output.chunks(64 * 1024) {
        file.write_all(chunk)
            .map_err(|error| format!("MP3-Datei konnte nicht geschrieben werden: {error}"))?;
        bytes_written += chunk.len();
        progress(0.9 + 0.1 * bytes_written as f32 / output.len() as f32);
    }
    file.flush()
        .map_err(|error| format!("MP3-Datei konnte nicht abgeschlossen werden: {error}"))?;
    progress(1.0);
    Ok(())
}

/// Zweck: Überführt die vom Encoder initialisierten Bytes aus der Vec-Reserve in deren sichtbare Länge.
/// Eingaben: `output` – Bytepuffer mit freier Reserve; `written` – Encoder-Rückgabewert für initialisierte Bytes.
/// Ausgabe: `Ok(())` oder ein Fehler, wenn der Encoder mehr Bytes meldet als die Reserve aufnehmen kann.
fn append_initialized_output(output: &mut Vec<u8>, written: usize) -> Result<(), String> {
    let new_len = output
        .len()
        .checked_add(written)
        .filter(|length| *length <= output.capacity())
        .ok_or_else(|| "LAME-Ausgabepuffer meldete eine ungültige Byteanzahl.".to_string())?;
    // SAFETY: LAME hat bei encode/flush genau `written` Bytes in die zuvor übergebene Vec-Reserve geschrieben.
    unsafe { output.set_len(new_len) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft die MP3-Aufnahme mit dem LAME-Encoder und den gemeinsamen Core-Importpfad.
    /// Eingaben: Keine; kodiert einen synthetischen Ton in einer temporären MP3-Datei ohne Capture oder Playback.
    /// Ausgabe: Keine Rückgabe; Assertions sichern Endung, Dekodierbarkeit, Monoformat und nichtstille Samples.
    #[test]
    fn mp3_recording_decodes_through_common_audio_reader() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.MP3");
        let sample_rate = 44_100;
        let samples = (0..sample_rate / 2)
            .map(|index| {
                (std::f32::consts::TAU * 440.0 * index as f32 / sample_rate as f32).sin() * 0.35
            })
            .collect::<Vec<_>>();

        let mut encode_progress = Vec::new();
        write_recording_audio(&path, &samples, sample_rate, |value| {
            encode_progress.push(value)
        })
        .unwrap();
        let mut decode_progress = Vec::new();
        let decoded = livetab_core::audio::read_audio_with_progress(&path, |value| {
            decode_progress.push(value)
        })
        .unwrap();

        assert_eq!(recording_format(&path), Some(RecordingFormat::Mp3));
        assert_eq!(decoded.format.sample_rate, sample_rate);
        assert_eq!(decoded.format.channels, 1);
        assert!(!decoded.samples.is_empty());
        assert!(decoded.samples.iter().any(|sample| sample.abs() > 0.02));
        assert!(
            encode_progress
                .windows(2)
                .all(|values| values[0] <= values[1])
        );
        assert_eq!(encode_progress.last(), Some(&1.0));
        assert!(
            decode_progress
                .windows(2)
                .all(|values| values[0] <= values[1])
        );
        assert_eq!(decode_progress.last(), Some(&1.0));
    }

    /// Zweck: Stellt sicher, dass unbekannte oder fehlende Recorder-Endungen abgewiesen werden.
    /// Eingaben: Keine; prüft zwei temporäre Zielpfade ohne Schreibvorgang.
    /// Ausgabe: Keine Rückgabe; Assertions erwarten nur für `.wav`/`.mp3` ein Aufnahmeformat.
    #[test]
    fn recording_format_requires_wav_or_mp3_extension() {
        assert_eq!(
            recording_format(Path::new("take.wav")),
            Some(RecordingFormat::Wav)
        );
        assert_eq!(
            recording_format(Path::new("take.Mp3")),
            Some(RecordingFormat::Mp3)
        );
        assert_eq!(recording_format(Path::new("take.flac")), None);
        assert_eq!(recording_format(Path::new("take")), None);
    }

    /// Zweck: Prüft, dass die vorhandene WAV-Ausgabe beim Hinzufügen von MP3 unverändert funktioniert.
    /// Eingaben: Keine; schreibt kurze synthetische PCM-Samples ohne Capture in ein temporäres WAV.
    /// Ausgabe: Keine Rückgabe; Assertions sichern WAV-Endung, Sample-Rate und Sampleanzahl nach Einlesen.
    #[test]
    fn wav_recording_still_uses_existing_pcm_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("still-wav.wav");
        let samples = vec![0.0_f32, 0.25, -0.25, 0.0];

        let mut progress = Vec::new();
        write_recording_audio(&path, &samples, 48_000, |value| progress.push(value)).unwrap();
        let decoded = livetab_core::audio::read_audio(&path).unwrap();

        assert_eq!(recording_format(&path), Some(RecordingFormat::Wav));
        assert_eq!(decoded.format.sample_rate, 48_000);
        assert_eq!(decoded.format.channels, 1);
        assert_eq!(decoded.samples.len(), samples.len());
        assert!(progress.windows(2).all(|values| values[0] <= values[1]));
        assert_eq!(progress.last(), Some(&1.0));
    }
}
