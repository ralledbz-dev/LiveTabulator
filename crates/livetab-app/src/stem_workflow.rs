/*
===============================================================================
Division By Zeros (DBZS) - lokaler Stem-Workflow
Modul: stem_workflow
Zweck: Startet den gepinnten Python-Separator sicher, fordert bei fehlenden
       Modelldateien Zustimmung an und übernimmt vier geprüfte WAV-Stems.
Sprache: Rust
Hinweise:
- Vor Bestätigung bleiben sämtliche Paket-Downloader im Python-Worker gesperrt.
- Der Unterprozess ist abbrechbar; stdout ist ein JSON-Lines-Protokoll.
- Nur lokale WAVs und Modelldateien werden verwendet.
===============================================================================
*/
use crate::application_data_directory;
use livetab_core::AppController;
use livetab_core::audio::read_wav;
use livetab_core::jobs::{CancellationToken, JobHandle, JobKind, JobReporter};
use livetab_core::project_model::{SongProject, StemKind, StemTrack};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_MODEL_FILENAME: &str = "htdemucs.yaml";
const MAX_STEM_WAV_BYTES: u64 = 2_000_000_000;
const MAX_WORKER_DIAGNOSTIC_BYTES: usize = 32 * 1024;

/// Kontext der einen Trennungsanfrage; wird nur für genau deren Modellzustimmung aufbewahrt.
#[derive(Debug, Clone)]
pub(crate) struct StemRunContext {
    input_path: PathBuf,
    model_directory: PathBuf,
    output_directory: PathBuf,
    model_filename: String,
    input_duration_seconds: f64,
    /// Text des unveränderlichen, zuvor angezeigten Downloadbestätigungsdialogs.
    pub(crate) download_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StemWorkerOutput {
    kind: String,
    path: PathBuf,
    sample_rate: u32,
    channels: u16,
    duration_seconds: f64,
    model_filename: String,
    model_version: Option<String>,
}

enum StemWorkerOutcome {
    NeedsDownload(String),
    Completed(Vec<StemWorkerOutput>),
}

/// Zweck: Bereitet absolute, lokale Eingabe-, Modell- und Ausgabeordner für einen Separatorjob vor.
/// Eingaben: `project` – aktueller Song mit WAV-Quelle und bereits gemessener Dauer.
/// Ausgabe: Nutzbarer Jobkontext oder eine konkrete Pfad-/Dateifehlermeldung.
pub(crate) fn prepare_context(project: &SongProject) -> Result<StemRunContext, String> {
    let input_path = project
        .audio_path
        .as_ref()
        .ok_or_else(|| "Bitte zuerst eine lokale WAV-Datei öffnen.".to_string())?;
    let input_metadata = fs::symlink_metadata(input_path)
        .map_err(|error| format!("Audioquelle kann nicht geprüft werden: {error}"))?;
    if input_metadata.file_type().is_symlink()
        || !input_metadata.is_file()
        || input_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("wav"))
    {
        return Err(
            "Die Stem-Trennung benötigt eine vorhandene lokale WAV-Datei ohne Symlink.".into(),
        );
    }
    if !project.duration_seconds.is_finite() || project.duration_seconds <= 0.0 {
        return Err("Die geöffnete WAV-Datei hat keine gültige positive Dauer.".into());
    }
    let input_path = fs::canonicalize(input_path)
        .map_err(|error| format!("Audioquelle kann nicht aufgelöst werden: {error}"))?;
    let app_data = application_data_directory().map_err(|error| error.to_string())?;
    let model_directory = match std::env::var_os("LIVETAB_STEM_MODEL_DIR") {
        Some(path) => PathBuf::from(path),
        None => app_data.join("models").join("source-separation"),
    };
    if !model_directory.is_absolute() {
        return Err("LIVETAB_STEM_MODEL_DIR muss ein absoluter lokaler Pfad sein.".into());
    }
    fs::create_dir_all(&model_directory).map_err(|error| {
        format!("Lokaler Stem-Modellordner kann nicht angelegt werden: {error}")
    })?;
    if fs::symlink_metadata(&model_directory)
        .map_err(|error| format!("Stem-Modellordner kann nicht geprüft werden: {error}"))?
        .file_type()
        .is_symlink()
    {
        return Err("Der Stem-Modellordner darf kein Symlink sein.".into());
    }
    let model_directory = fs::canonicalize(&model_directory)
        .map_err(|error| format!("Stem-Modellordner kann nicht aufgelöst werden: {error}"))?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let output_directory = app_data.join("stem-jobs").join(format!("job-{unique}"));
    fs::create_dir_all(&output_directory)
        .map_err(|error| format!("Stem-Ausgabeordner kann nicht angelegt werden: {error}"))?;
    if fs::symlink_metadata(&output_directory)
        .map_err(|error| format!("Stem-Ausgabeordner kann nicht geprüft werden: {error}"))?
        .file_type()
        .is_symlink()
    {
        return Err("Der Stem-Ausgabeordner darf kein Symlink sein.".into());
    }
    let output_directory = fs::canonicalize(&output_directory)
        .map_err(|error| format!("Stem-Ausgabeordner kann nicht aufgelöst werden: {error}"))?;
    Ok(StemRunContext {
        input_path,
        model_directory,
        output_directory,
        model_filename: DEFAULT_MODEL_FILENAME.into(),
        input_duration_seconds: project.duration_seconds,
        download_prompt: None,
    })
}

/// Zweck: Startet den cancellable Separatorjob und übernimmt fehlende Modellfreigaben in den UI-Zustand.
/// Eingaben: Controller, unveränderter Jobkontext, Einwilligungsflag und gemeinsamer Freigabepuffer.
/// Ausgabe: `JobHandle` zum Statusabruf oder Abbruch des Python-Unterprozesses.
pub(crate) fn start_job(
    controller: AppController,
    context: StemRunContext,
    allow_model_download: bool,
    pending_download: Arc<Mutex<Option<StemRunContext>>>,
) -> JobHandle {
    if !allow_model_download {
        if let Ok(mut pending) = pending_download.lock() {
            *pending = None;
        }
    }
    let job_controller = controller.clone();
    let job_context = context.clone();
    controller.job_manager().start(JobKind::StemSeparation, move |reporter, cancellation| {
        cancellation.check()?;
        reporter.progress(0.02, "Lokale WAV- und Modellpfade werden geprüft");
        let request = json!({
            "inputPath": job_context.input_path.to_string_lossy(),
            "modelDirectory": job_context.model_directory.to_string_lossy(),
            "outputDirectory": job_context.output_directory.to_string_lossy(),
            "modelFilename": job_context.model_filename,
            "allowModelDownload": allow_model_download,
        });
        match run_worker(&request, &reporter, &cancellation)? {
            StemWorkerOutcome::NeedsDownload(prompt) => {
                let mut pending_context = job_context;
                pending_context.download_prompt = Some(prompt);
                *pending_download
                    .lock()
                    .map_err(|_| "Downloadfreigabe konnte nicht gespeichert werden.".to_string())? =
                    Some(pending_context);
                Ok("STEM_DOWNLOAD_REQUIRED".into())
            }
            StemWorkerOutcome::Completed(outputs) => {
                reporter.progress(0.90, "Vier WAV-Spuren werden validiert und ins Projekt übernommen");
                let tracks = validate_outputs(&job_context, outputs)?;
                job_controller.replace_stem_tracks(tracks)?;
                reporter.progress(0.98, "Projektspuren und synchronisierte WAVs sind bereit");
                Ok("Vier lokale Stem-Spuren bereit: Gesang, Schlagzeug, Bass und übrige Instrumente.".into())
            }
        }
    })
}

/// Zweck: Startet den Python-Worker, liest JSON-Lines nebenläufig und beendet ihn beim Abbruch.
/// Eingaben: JSON-Request, JobReporter für Fortschritt und CancellationToken.
/// Ausgabe: Modellfreigabeanforderung, geprüfte Stemliste oder verständlicher Prozessfehler.
fn run_worker(
    request: &Value,
    reporter: &JobReporter,
    cancellation: &CancellationToken,
) -> Result<StemWorkerOutcome, String> {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../livetab-core/workers/stem_worker.py");
    if !script.is_file() {
        return Err(format!("Lokaler Stem-Worker fehlt: {}", script.display()));
    }
    let python = std::env::var_os("LIVETAB_PYTHON").unwrap_or_else(|| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    });
    let mut child = Command::new(&python)
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Lokaler Python-Worker konnte nicht gestartet werden: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Python-Worker besitzt keinen Eingabekanal.".to_string())?;
    let payload = serde_json::to_vec(request)
        .map_err(|error| format!("Stem-Anfrage kann nicht serialisiert werden: {error}"))?;
    if let Err(error) = stdin.write_all(&payload) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "Stem-Anfrage konnte nicht an Python übergeben werden: {error}"
        ));
    }
    drop(stdin);
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Python-Worker besitzt keinen Ausgabekanal.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Python-Worker besitzt keinen Diagnosekanal.".to_string())?;
    let (line_sender, line_receiver) = mpsc::channel::<String>();
    let stdout_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if line_sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = String::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if output.len() < MAX_WORKER_DIAGNOSTIC_BYTES {
                output.push_str(&line);
                output.push('\n');
            }
        }
        output
    });

    let mut outcome = None;
    let mut worker_error = None;
    let status = loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("Stem-Trennung abgebrochen.".into());
        }
        while let Ok(line) = line_receiver.try_recv() {
            collect_worker_event(&line, reporter, &mut outcome, &mut worker_error);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!(
                    "Python-Workerstatus konnte nicht geprüft werden: {error}"
                ));
            }
        }
    };
    let _ = stdout_reader.join();
    while let Ok(line) = line_receiver.try_recv() {
        collect_worker_event(&line, reporter, &mut outcome, &mut worker_error);
    }
    let diagnostics = stderr_reader.join().unwrap_or_default();
    if let Some(error) = worker_error {
        return Err(if diagnostics.trim().is_empty() {
            error
        } else {
            format!("{error} · Python: {}", diagnostics.trim())
        });
    }
    if !status.success() {
        return Err(format!(
            "Python-Worker endete mit {status}{}",
            if diagnostics.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", diagnostics.trim())
            }
        ));
    }
    outcome.ok_or_else(|| "Python-Worker lieferte weder Ergebnis noch Downloadstatus.".into())
}

/// Zweck: Verarbeitet ein einzelnes JSON-Lines-Ereignis und meldet Fortschritt an den Jobmanager.
/// Eingaben: `line` – Worker-JSON; Reporter und Ergebnis-/Fehlerakkumulatoren des aufrufenden Jobs.
/// Ausgabe: Keine; ein Ergebnisereignis wird zwischengespeichert, ein Workerfehler bleibt sichtbar.
fn collect_worker_event(
    line: &str,
    reporter: &JobReporter,
    outcome: &mut Option<StemWorkerOutcome>,
    worker_error: &mut Option<String>,
) {
    let event: Value = match serde_json::from_str(line) {
        Ok(event) => event,
        Err(error) => {
            *worker_error = Some(format!("Ungültige JSON-Nachricht vom Stem-Worker: {error}"));
            return;
        }
    };
    match event.get("event").and_then(Value::as_str) {
        Some("progress") => {
            let percent = event.get("percent").and_then(Value::as_f64).unwrap_or(0.0);
            let stage = event
                .get("stage")
                .and_then(Value::as_str)
                .unwrap_or("Stem-Worker");
            reporter.progress((percent / 100.0).clamp(0.0, 0.99) as f32, stage);
        }
        Some("needs_download") => {
            *outcome = Some(StemWorkerOutcome::NeedsDownload(download_prompt(&event)));
        }
        Some("result") => match serde_json::from_value::<Vec<StemWorkerOutput>>(
            event.get("stems").cloned().unwrap_or(Value::Null),
        ) {
            Ok(stems) => *outcome = Some(StemWorkerOutcome::Completed(stems)),
            Err(error) => *worker_error = Some(format!("Stem-Ergebnis ist ungültig: {error}")),
        },
        Some("error") => {
            *worker_error = Some(
                event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Python-Worker meldete einen unbekannten Fehler.")
                    .to_string(),
            );
        }
        _ => *worker_error = Some("Python-Worker lieferte einen unbekannten Ereignistyp.".into()),
    }
}

/// Zweck: Formuliert die explizite Freigabe anhand der tatsächlichen Workerantwort.
/// Eingaben: `event` – `needs_download`-JSON mit Modell, fehlenden Namen, Quellen- und Lizenzstatus.
/// Ausgabe: Klartext für den Slint-Dialog, ohne nicht bestätigte Größen- oder Lizenzversprechen.
fn download_prompt(event: &Value) -> String {
    let model = event
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_MODEL_FILENAME);
    let missing = event
        .get("missing")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| "Modellressourcen werden vom Fork aufgelöst".into());
    let sources = event.get("sourceContext");
    let mirror = sources
        .and_then(|value| value.get("mirror"))
        .and_then(Value::as_str)
        .unwrap_or("vom gepinnten python-audio-separator-Fork festgelegter Mirror");
    let upstream = sources
        .and_then(|value| value.get("upstream"))
        .and_then(Value::as_str)
        .unwrap_or("vom gepinnten Fork ausgewählte Upstream-Quelle");
    let host = sources
        .and_then(|value| value.get("host"))
        .and_then(Value::as_str)
        .unwrap_or("GitHub/Modell-Upstream");
    let license = event
        .get("licenseStatus")
        .and_then(Value::as_str)
        .unwrap_or("unverified by app");
    format!(
        "Modell: {model} · Demucs v4 · 4 Spuren (Gesang, Schlagzeug, Bass, übrige Instrumente).\n\nFehlende Dateien: {missing}.\n\nQuelle: {mirror}; Fallback/Modellmetadaten: {upstream} ({host}). Die konkrete Downloadgröße wird vorab nicht zuverlässig ausgewiesen und kann groß sein.\n\nLizenzstatus: {license}. Die App hat die Lizenzbedingungen der Modellgewichte nicht unabhängig geprüft.\n\nNur „Herunterladen und fortfahren“ erlaubt diesem Auftrag, fehlende Dateien aus den vom Fork festgelegten Quellen lokal abzurufen. Ohne Bestätigung findet kein Modell-Download statt."
    )
}

/// Zweck: Prüft vier erzeugte WAV-Dateien nochmals in Rust, bevor sie Projektspuren werden.
/// Eingaben: unveränderter Jobkontext und Workeroutputs mit Stemlabel, Dateipfad und gemessenen WAV-Werten.
/// Ausgabe: Vier validierte `StemTrack`-Metadaten oder ein sicherheits-/Formatfehler.
fn validate_outputs(
    context: &StemRunContext,
    outputs: Vec<StemWorkerOutput>,
) -> Result<Vec<StemTrack>, String> {
    if outputs.len() != 4 {
        return Err("Stem-Worker muss genau vier WAV-Ausgaben liefern.".into());
    }
    let output_root = fs::canonicalize(&context.output_directory)
        .map_err(|error| format!("Stem-Ausgabeordner fehlt: {error}"))?;
    let mut kinds = HashSet::new();
    let mut tracks = Vec::with_capacity(4);
    for output in outputs {
        let kind = parse_kind(&output.kind)
            .ok_or_else(|| format!("Nicht unterstützte Stem-Klasse `{}`.", output.kind))?;
        if !kinds.insert(kind) {
            return Err(format!("Stem `{}` wurde mehrfach geliefert.", output.kind));
        }
        if !output.path.is_absolute() {
            return Err("Stem-Worker muss absolute Ausgabewege liefern.".into());
        }
        let metadata = fs::symlink_metadata(&output.path)
            .map_err(|error| format!("Stem-WAV {} fehlt: {error}", output.path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
            return Err(format!(
                "Stem-WAV {} ist keine sichere, nichtleere Datei.",
                output.path.display()
            ));
        }
        if metadata.len() > MAX_STEM_WAV_BYTES {
            return Err(format!(
                "Stem-WAV {} überschreitet zwei GiB.",
                output.path.display()
            ));
        }
        let canonical = fs::canonicalize(&output.path)
            .map_err(|error| format!("Stem-WAV kann nicht aufgelöst werden: {error}"))?;
        if canonical.parent() != Some(output_root.as_path()) {
            return Err("Stem-WAV liegt außerhalb des jobbezogenen Ausgabeordners.".into());
        }
        let wave =
            read_wav(&canonical).map_err(|error| format!("Stem-WAV ist ungültig: {error}"))?;
        let duration = wave.duration_seconds();
        if (duration - context.input_duration_seconds).abs() > 0.020
            || (output.duration_seconds - duration).abs() > 0.020
        {
            return Err(format!(
                "Stem `{}` ist nicht innerhalb von 20 ms synchronisiert.",
                output.kind
            ));
        }
        if output.sample_rate != wave.format.sample_rate || output.channels != wave.format.channels
        {
            return Err(format!(
                "Stem-Metadaten für `{}` stimmen nicht mit der WAV-Datei überein.",
                output.kind
            ));
        }
        if output.model_filename != context.model_filename {
            return Err("Stem-Worker lieferte ein anderes als das bestätigte Modell.".into());
        }
        let track = StemTrack {
            id: format!("stem-{}", stem_kind_key(kind)),
            name: kind.display_name().into(),
            kind,
            file_path: canonical,
            sample_rate: wave.format.sample_rate,
            channels: wave.format.channels,
            duration_seconds: duration,
            model_filename: output.model_filename,
            model_version: output.model_version.unwrap_or_default(),
        };
        track.validate()?;
        tracks.push(track);
    }
    if kinds.len() != 4 {
        return Err("Stem-Ergebnis enthält nicht alle vier geplanten Spuren.".into());
    }
    tracks.sort_by_key(|track| match track.kind {
        StemKind::Vocals => 0,
        StemKind::Drums => 1,
        StemKind::Bass => 2,
        StemKind::Other => 3,
    });
    Ok(tracks)
}

/// Zweck: Wandelt den vom Python-Worker gesendeten Klassenschlüssel fail-closed in einen Core-Typ um.
/// Eingaben: `kind` – `vocals`, `drums`, `bass` oder `other`.
/// Ausgabe: Passender `StemKind`, sonst `None`.
fn parse_kind(kind: &str) -> Option<StemKind> {
    match kind {
        "vocals" => Some(StemKind::Vocals),
        "drums" => Some(StemKind::Drums),
        "bass" => Some(StemKind::Bass),
        "other" => Some(StemKind::Other),
        _ => None,
    }
}

/// Zweck: Erzeugt den stabilen Dateischlüssel einer Stem-Klasse.
/// Eingaben: `kind` – Kernklassifikation einer Audioquelle.
/// Ausgabe: Klein geschriebener JSON-/Dateikennwert.
fn stem_kind_key(kind: StemKind) -> &'static str {
    match kind {
        StemKind::Vocals => "vocals",
        StemKind::Drums => "drums",
        StemKind::Bass => "bass",
        StemKind::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zweck: Prüft, dass die Downloadfrage Modell, Dateiliste und ungeklärten Lizenzstatus offenlegt.
    /// Eingaben: Keine; verwendet eine künstliche `needs_download`-Antwort.
    /// Ausgabe: Keine Rückgabe; Assertions sichern die wesentlichen Freigabeinformationen.
    #[test]
    fn download_prompt_is_explicit_about_unknown_license_and_size() {
        let event = json!({
            "model": "htdemucs.yaml",
            "missing": ["htdemucs.yaml"],
            "sourceContext": {
                "mirror": "https://github.com/nomadkaraoke/python-audio-separator/releases/download/model-configs",
                "upstream": "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models; https://raw.githubusercontent.com/TRvlvr/application_data/main/filelists/download_checks.json; Demucs-Gewichte ggf. dl.fbaipublicfiles.com/demucs/...",
                "host": "GitHub und Modell-Upstream"
            },
            "licenseStatus": "unverified by app"
        });
        let text = download_prompt(&event);
        assert!(text.contains("htdemucs.yaml"));
        assert!(text.contains("nomadkaraoke/python-audio-separator"));
        assert!(text.contains("dl.fbaipublicfiles.com/demucs"));
        assert!(text.contains("unverified by app"));
        assert!(text.contains("Downloadgröße"));
        assert!(text.contains("Ohne Bestätigung"));
    }
}
