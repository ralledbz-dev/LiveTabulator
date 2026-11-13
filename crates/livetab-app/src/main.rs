mod app_workflows;
mod audio_capture;
mod llama_runtime;
mod playback_output;
mod stem_workflow;

use crate::llama_runtime::LoadedModel;
use livetab_core::AppController;
use livetab_core::model_registry::{ModelRegistry, RegistrySnapshot};
use livetab_core::project::{ProjectRecord, ProjectStore};
#[cfg(windows)]
use rfd::FileDialog;
use slint::{ComponentHandle, Weak};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

slint::include_modules!();

enum RuntimeCommand {
    Generate(String),
}

type RuntimeSender = mpsc::Sender<RuntimeCommand>;
type RuntimeState = Arc<Mutex<Option<RuntimeSender>>>;
type ModelPathState = Arc<Mutex<Option<PathBuf>>>;
type ModelCatalog = Arc<Mutex<Option<RegistrySnapshot>>>;

/// Zweck: Startet das Slint-Fenster, verbindet UI-Callbacks mit Scan und Runtime und führt die Ereignisschleife aus.
/// Eingaben: Keine Argumente; verwendet den ausführbaren App-Pfad sowie verbundene Modellpfad-Umgebungswerte.
/// Ausgabe: `Ok(())` nach regulärem Schließen oder ein Initialisierungs-/UI-Fehler.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ui = MainWindow::new()?;
    ui.set_bandmap_file_picker_available(cfg!(windows));
    let app_models_dir = app_models_directory();
    let current_model: ModelPathState = Arc::new(Mutex::new(None));
    let runtime_state: RuntimeState = Arc::new(Mutex::new(None));
    let model_catalog: ModelCatalog = Arc::new(Mutex::new(None));

    {
        let weak = ui.as_weak();
        let app_models_dir = app_models_dir.clone();
        let current_model = Arc::clone(&current_model);
        let runtime_state = Arc::clone(&runtime_state);
        let model_catalog = Arc::clone(&model_catalog);
        // Zweck: Verarbeitet den Klick auf „Modelle scannen“.
        // Eingaben: Keine expliziten Argumente; verwendet die gespeicherten UI-/Scan-Zustände.
        // Ausgabe: Kein Rückgabewert; startet den Scan einschließlich externer Wurzeln im Hintergrund.
        ui.on_scan_models_requested(move || {
            start_scan(
                weak.clone(),
                app_models_dir.clone(),
                Arc::clone(&current_model),
                Arc::clone(&runtime_state),
                Arc::clone(&model_catalog),
                true,
            );
        });
    }

    {
        let weak = ui.as_weak();
        let current_model = Arc::clone(&current_model);
        let runtime_state = Arc::clone(&runtime_state);
        // Zweck: Verarbeitet die Anforderung, das validierte lokale Modell zu laden.
        // Eingaben: Keine Callback-Argumente; liest den aktuell ausgewählten Modellpfad.
        // Ausgabe: Kein Rückgabewert; aktualisiert den UI-Status und startet den Runtime-Worker.
        ui.on_load_model_requested(move || {
            let model_path = current_model.lock().ok().and_then(|path| path.clone());
            let Some(model_path) = model_path else {
                set_runtime_status(&weak, "Kein validiertes GGUF-Textmodell gefunden.");
                return;
            };
            if let Some(handle) = weak.upgrade() {
                handle.set_busy(true);
                handle.set_model_status_text(
                    "llama.cpp lädt das lokale Modell im Hintergrund …".into(),
                );
            }
            start_runtime_worker(model_path, weak.clone(), Arc::clone(&runtime_state));
        });
    }

    {
        let weak = ui.as_weak();
        let runtime_state = Arc::clone(&runtime_state);
        // Zweck: Übergibt den aktuellen Prompt an die bereits geladene Runtime.
        // Eingaben: Keine Callback-Argumente; liest Prompt und Runtime-Sender aus dem UI-Zustand.
        // Ausgabe: Kein Rückgabewert; sendet einen Worker-Auftrag und aktualisiert die UI.
        ui.on_generate_requested(move || {
            let Some(handle) = weak.upgrade() else { return };
            let prompt = handle.get_prompt_text().to_string();
            let sender = runtime_state
                .lock()
                .ok()
                .and_then(|state| state.as_ref().cloned());
            let Some(sender) = sender else {
                handle.set_model_status_text("Runtime ist noch nicht geladen.".into());
                return;
            };
            handle.set_busy(true);
            handle.set_assistant_output_text("Lokale Inferenz läuft …".into());
            if sender.send(RuntimeCommand::Generate(prompt)).is_err() {
                handle.set_busy(false);
                handle.set_runtime_loaded(false);
                handle.set_model_status_text(
                    "Runtime-Worker wurde beendet; lade das Modell erneut.".into(),
                );
            }
        });
    }

    {
        let weak = ui.as_weak();
        // Zweck: Übernimmt eine Dateiauswahl und startet den gemeinsamen BANDMAP-Prüf-/Importablauf.
        // Eingaben: Keine Callback-Argumente; der Dateipfad wird vom nativen Dialog geliefert.
        // Ausgabe: Kein Rückgabewert; startet bei Auswahl den Importworker oder meldet Abbruch/Plattformgrenze.
        ui.on_select_bandmap_requested(move || {
            select_and_import_bandmap(weak.clone());
        });
    }

    {
        let weak = ui.as_weak();
        // Zweck: Verarbeitet den Import eines manuell eingetragenen BANDMAP-Pfads.
        // Eingaben: Keine Callback-Argumente; der Pfad stammt aus `bandmap_path_text`.
        // Ausgabe: Kein Rückgabewert; übergibt den Pfad an den gemeinsamen Importworker.
        ui.on_import_bandmap_requested(move || {
            let Some(handle) = weak.upgrade() else { return };
            let archive_path = PathBuf::from(handle.get_project_path_text().to_string());
            if archive_path.as_os_str().is_empty() {
                handle.set_project_status_text(
                    "Bitte zuerst den Pfad zu einer .bandmap-Datei eintragen.".into(),
                );
                return;
            }
            start_bandmap_import(weak.clone(), archive_path);
        });
    }

    let controller = AppController::new("Unbenannt", "");
    let lifecycle = app_workflows::wire(&ui, controller, Arc::clone(&model_catalog));
    start_restore_last_project(ui.as_weak());
    start_scan(
        ui.as_weak(),
        app_models_dir,
        Arc::clone(&current_model),
        Arc::clone(&runtime_state),
        Arc::clone(&model_catalog),
        true,
    );
    ui.run()?;
    lifecycle.stop();
    Ok(())
}

/// Zweck: Öffnet den nativen BANDMAP-Dateidialog und startet bei Auswahl den geprüften Import.
/// Eingaben: `weak` – schwacher Slint-Handle zur Statusanzeige; der Dateipfad stammt aus dem Dialog.
/// Ausgabe: Keine Rückgabe; Auswahl wird importiert, Abbruch oder nicht unterstützte Plattform wird angezeigt.
fn select_and_import_bandmap(weak: Weak<MainWindow>) {
    {
        let Some(handle) = weak.upgrade() else { return };
        if handle.get_project_busy() {
            return;
        }
    }

    #[cfg(windows)]
    {
        let selected_path = FileDialog::new()
            .set_title("BANDMAP-Datei auswählen")
            .add_filter("BANDMAP-Projekt", &["bandmap"])
            .pick_file();
        match selected_path {
            Some(path) => start_bandmap_import(weak, path),
            None => {
                if let Some(handle) = weak.upgrade() {
                    handle.set_project_status_text(
                        "Dateiauswahl abgebrochen; es wurde nichts importiert.".into(),
                    );
                }
            }
        }
    }

    #[cfg(not(windows))]
    if let Some(handle) = weak.upgrade() {
        handle.set_project_status_text(
            "Die native Dateiauswahl ist in diesem Build nicht verfügbar; bitte den .bandmap-Pfad eintragen.".into(),
        );
    }
}

/// Zweck: Prüft und persistiert ein BANDMAP-Archiv im Hintergrund und meldet das Ergebnis an die UI.
/// Eingaben: `weak` – Slint-Handle für Status/Rückmeldung; `archive_path` – zu prüfender `.bandmap`-Pfad.
/// Ausgabe: Keine Rückgabe; setzt den Busy-Zustand und zeigt nach Importerfolg oder Fehler den Status an.
fn start_bandmap_import(weak: Weak<MainWindow>, archive_path: PathBuf) {
    let Some(handle) = weak.upgrade() else { return };
    if handle.get_project_busy() {
        return;
    }
    handle.set_project_path_text(archive_path.to_string_lossy().into_owned().into());
    handle.set_project_busy(true);
    handle.set_project_status_text(
        "Dateiendung und BANDMAP-Archiv werden geprüft; das Projekt wird lokal gespeichert …"
            .into(),
    );

    thread::spawn(move || {
        let result = application_data_directory().and_then(|root| {
            ProjectStore::new(root.clone())
                .import_bandmap(&archive_path)
                .map(|record| (root, record))
                .map_err(|error| error.to_string())
        });
        let status = bandmap_import_status(result);
        let _ = weak.upgrade_in_event_loop(move |handle| {
            handle.set_project_status_text(status.into());
            handle.set_project_busy(false);
        });
    });
}

/// Zweck: Formatiert Importergebnis oder Fehler als verständliche BANDMAP-Rückmeldung für die Oberfläche.
/// Eingaben: `result` – Erfolg mit Ablagewurzel und persistiertem Projektdatensatz oder ein Fehlertext.
/// Ausgabe: Deutschsprachiger Status mit Formatversion und Speicherort oder mit der Fehlerursache.
fn bandmap_import_status(result: Result<(PathBuf, ProjectRecord), String>) -> String {
    match result {
        Ok((root, record)) => format!(
            "Import erfolgreich: BANDMAP v{} „{}“ geprüft und lokal gespeichert.\n{}",
            record.format_version,
            record.title,
            root.join("projects").join(record.id).display()
        ),
        Err(error) => format!("Import fehlgeschlagen: {error}"),
    }
}

/// Zweck: Ermittelt den dauerhaften App-Datenordner für importierte BANDMAP-Projekte.
/// Eingaben: Keine expliziten Argumente; liest unter Windows `LOCALAPPDATA`, sonst `HOME`/`XDG_DATA_HOME`.
/// Ausgabe: App-spezifischer beschreibbarer `PathBuf` oder eine verständliche Fehlermeldung.
fn application_data_directory() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(|path| PathBuf::from(path).join("DBZS").join("LiveTabulator"))
            .ok_or_else(|| "Windows-Appdatenordner LOCALAPPDATA ist nicht gesetzt.".to_string());
    }
    #[cfg(not(windows))]
    {
        if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(path).join("dbzs").join("livetabulator"));
        }
        std::env::var_os("HOME")
            .map(|path| {
                PathBuf::from(path)
                    .join(".local")
                    .join("share")
                    .join("dbzs")
                    .join("livetabulator")
            })
            .ok_or_else(|| {
                "Kein dauerhafter App-Datenordner über HOME/XDG_DATA_HOME verfügbar.".to_string()
            })
    }
}

/// Zweck: Stellt das zuletzt importierte BANDMAP-Projekt im UI nach einem App-Neustart wieder dar.
/// Eingaben: `weak` – schwacher Slint-Handle; Projektindex und Wiederherstellungszeiger stammen aus dem App-Datenordner.
/// Ausgabe: Keine Rückgabe; liest Metadaten im Hintergrund und aktualisiert nur die Projektstatusanzeige.
fn start_restore_last_project(weak: Weak<MainWindow>) {
    thread::spawn(move || {
        let status = match application_data_directory() {
            Ok(root) => match ProjectStore::new(root).load_last_project() {
                Ok(Some(record)) => format!(
                    "Letztes Projekt: {} · BANDMAP v{}\nContainer und Projektdaten sind lokal wiederhergestellt.",
                    record.title, record.format_version
                ),
                Ok(None) => "Noch kein BANDMAP-Projekt importiert.".to_string(),
                Err(error) => format!("Gespeichertes Projekt konnte nicht geladen werden: {error}"),
            },
            Err(error) => format!("Projektablage nicht verfügbar: {error}"),
        };
        let _ = weak.upgrade_in_event_loop(move |handle| {
            handle.set_project_status_text(status.into());
        });
    });
}

/// Zweck: Ermittelt den App-eigenen Modellordner neben der Anwendung oder als Projekt-Fallback.
/// Eingaben: Keine expliziten Parameter; liest den aktuellen Executable-Pfad.
/// Ausgabe: Der verwendbare Modellordner als `PathBuf`.
fn app_models_directory() -> PathBuf {
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            let adjacent = parent.join("models");
            if adjacent.is_dir() {
                return adjacent;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
}

/// Zweck: Setzt den Scanstatus, prüft App-Modelle sofort und durchsucht externe Wurzeln optional im Hintergrund.
/// Eingaben: Schwacher UI-Handle, App-Modellpfad, Modell-/Runtime-Zustände und `include_external`.
/// Ausgabe: Keine Rückgabe; startet einen Worker und aktualisiert UI sowie ausgewähltes Modell asynchron.
fn start_scan(
    weak: Weak<MainWindow>,
    app_models_dir: PathBuf,
    current_model: ModelPathState,
    runtime_state: RuntimeState,
    model_catalog: ModelCatalog,
    include_external: bool,
) {
    if let Some(handle) = weak.upgrade() {
        handle.set_scanning(true);
        handle.set_runtime_loaded(false);
        handle.set_model_found(false);
        handle.set_model_status_text("Runtime wird nach Auswahl eines Modells geladen.".into());
        handle.set_status_text(
            "App-Modellordner wird sofort geprüft; externe Modellwurzeln folgen im Hintergrund …"
                .into(),
        );
        handle.set_model_list_text(
            "App-Modelle werden indiziert; die Oberfläche bleibt bedienbar.".into(),
        );
    }
    if let Ok(mut path) = current_model.lock() {
        *path = None;
    }
    if let Ok(mut sender) = runtime_state.lock() {
        *sender = None;
    }

    thread::spawn(move || {
        let roots = ModelRegistry::default_roots(&app_models_dir);
        let quick_snapshot = ModelRegistry::scan(std::iter::once(app_models_dir.clone()));
        if let Ok(mut catalog) = model_catalog.lock() {
            *catalog = Some(quick_snapshot.clone());
        }
        let quick_model = quick_snapshot
            .assistant_model()
            .map(|entry| entry.path.clone());
        if let Ok(mut path) = current_model.lock() {
            *path = quick_model;
        }

        let external_roots = if include_external {
            roots.into_iter().skip(1).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let has_external_roots = !external_roots.is_empty();
        let quick_status = if has_external_roots {
            format!(
                "{} App-Dateien sofort geprüft · D:\\Models und weitere Wurzeln laufen parallel weiter",
                quick_snapshot
                    .roots
                    .iter()
                    .map(|root| root.files_scanned)
                    .sum::<usize>()
            )
        } else if include_external {
            scan_summary(&quick_snapshot)
        } else {
            format!(
                "{} · externe Wurzeln nur auf Anfrage",
                scan_summary(&quick_snapshot)
            )
        };
        let quick_list = model_list_text(&quick_snapshot);
        let quick_found = quick_snapshot.assistant_model().is_some();
        let _ = weak.upgrade_in_event_loop(move |handle| {
            handle.set_status_text(quick_status.into());
            handle.set_model_list_text(quick_list.into());
            handle.set_model_found(quick_found);
            handle.set_scanning(has_external_roots);
        });

        if !has_external_roots {
            return;
        }

        let progress_weak = weak.clone();
        let external_snapshot =
            ModelRegistry::scan_with_progress(external_roots, move |root, files, candidates| {
                let status = format!(
                    "Suche {} · {files} Modell-Dateien geprüft · {candidates} Kandidaten …",
                    root.display()
                );
                let _ = progress_weak.clone().upgrade_in_event_loop(move |handle| {
                    handle.set_status_text(status.into());
                });
            });
        let snapshot = merge_snapshots(quick_snapshot, external_snapshot);
        if let Ok(mut catalog) = model_catalog.lock() {
            *catalog = Some(snapshot.clone());
        }
        let selected_model = snapshot.assistant_model().map(|entry| entry.path.clone());
        if let Ok(mut path) = current_model.lock() {
            *path = selected_model;
        }
        let status = scan_summary(&snapshot);
        let model_list = model_list_text(&snapshot);
        let found = snapshot.assistant_model().is_some();
        let _ = weak.upgrade_in_event_loop(move |handle| {
            handle.set_status_text(status.into());
            handle.set_model_list_text(model_list.into());
            handle.set_model_found(found);
            handle.set_scanning(false);
        });
    });
}

/// Zweck: Fügt den schnellen App-Scan und den späteren externen Scan zu einem Ergebnis zusammen.
/// Eingaben: `quick` und `external` – zwei bereits erstellte Registry-Snapshots.
/// Ausgabe: Ein zusammengeführter `RegistrySnapshot` mit addierten Zeiten und kombinierten Einträgen.
fn merge_snapshots(
    mut quick: RegistrySnapshot,
    mut external: RegistrySnapshot,
) -> RegistrySnapshot {
    quick.roots.append(&mut external.roots);
    quick.entries.append(&mut external.entries);
    quick.warnings.append(&mut external.warnings);
    quick.elapsed_ms += external.elapsed_ms;
    quick
}

/// Zweck: Fasst Wurzeln, geprüfte Dateien, Artefakte, Dauer und Assistant-Verfügbarkeit für die UI zusammen.
/// Eingaben: `snapshot` – Ergebnis eines Registry-Scans.
/// Ausgabe: Eine kompakte deutschsprachige Statuszeichenkette.
fn scan_summary(snapshot: &RegistrySnapshot) -> String {
    let files = snapshot
        .roots
        .iter()
        .map(|root| root.files_scanned)
        .sum::<usize>();
    let roots = snapshot.roots.iter().filter(|root| root.exists).count();
    let model_count = snapshot.entries.len();
    let assistant = if snapshot.assistant_model().is_some() {
        "festes Qwen3.5-4B-Q4_K_M-Standardmodell validiert"
    } else if snapshot
        .entries
        .iter()
        .any(|entry| entry.can_load_with_llama_cpp())
    {
        "andere Textmodelle erkannt; festes 4B-Standardmodell fehlt"
    } else {
        "kein validiertes GGUF-Textmodell"
    };
    format!(
        "{roots} erreichbare Wurzeln · {files} Modell-Dateien geprüft · {model_count} Artefakte · {} ms · {assistant}",
        snapshot.elapsed_ms
    )
}

/// Zweck: Formatiert gefundene Modellartefakte samt Rolle, Validierung, Größe und Pfad.
/// Eingaben: `snapshot` – Ergebnis eines Registry-Scans.
/// Ausgabe: Eine UI-Zeichenkette mit höchstens 18 Einträgen und gegebenenfalls einem Restzähler.
fn model_list_text(snapshot: &RegistrySnapshot) -> String {
    if snapshot.entries.is_empty() {
        return "Keine Modellartefakte erkannt. Geprüfte Wurzeln: App-Modelle und D:\\Models."
            .into();
    }
    let default_id = snapshot.assistant_model().map(|entry| entry.id.as_str());
    let mut rows = snapshot
        .entries
        .iter()
        .take(18)
        .map(|entry| {
            let architecture = entry
                .architecture
                .as_deref()
                .unwrap_or("Architektur ungeprüft");
            let selection = if default_id == Some(entry.id.as_str()) {
                "STANDARD"
            } else {
                "optional"
            };
            format!(
                "{} · {:?} · {:?} · {} · {:.1} MiB\n{} · {}",
                selection,
                entry.role,
                entry.status,
                entry.name,
                entry.size_bytes as f64 / (1024.0 * 1024.0),
                architecture,
                entry.path.display()
            )
        })
        .collect::<Vec<_>>();
    if snapshot.entries.len() > rows.len() {
        rows.push(format!(
            "… und {} weitere",
            snapshot.entries.len() - rows.len()
        ));
    }
    rows.join("\n\n")
}

/// Zweck: Lädt das gewählte Modell und verarbeitet spätere Inferenzaufträge auf einem eigenen Worker-Thread.
/// Eingaben: GGUF-Dateipfad, schwacher Slint-Handle und gemeinsam genutzter Runtime-Zustand.
/// Ausgabe: Keine Rückgabe; meldet Lade-/Inferenzstatus und Antworten über den Slint-Ereignisloop.
fn start_runtime_worker(model_path: PathBuf, weak: Weak<MainWindow>, runtime_state: RuntimeState) {
    let (sender, receiver) = mpsc::channel();
    if let Ok(mut state) = runtime_state.lock() {
        *state = Some(sender);
    }
    thread::spawn(move || {
        let model = match LoadedModel::load(Path::new(&model_path)) {
            Ok(model) => model,
            Err(error) => {
                if let Ok(mut state) = runtime_state.lock() {
                    *state = None;
                }
                let _ = weak.upgrade_in_event_loop(move |handle| {
                    handle.set_busy(false);
                    handle.set_runtime_loaded(false);
                    handle.set_model_status_text(format!("Runtime-Fehler: {error}").into());
                });
                return;
            }
        };

        let filename = model_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("GGUF")
            .to_string();
        let threads = model.threads();
        let acceleration = if model.gpu_layers() > 0 {
            format!("CUDA · {} GPU-Layer", model.gpu_layers())
        } else {
            "CPU/OpenMP".to_string()
        };
        let loaded_status = format!("Geladen: {filename} · {acceleration} · {threads} CPU-Threads");
        let status_for_ui = loaded_status.clone();
        let _ = weak.upgrade_in_event_loop(move |handle| {
            handle.set_busy(false);
            handle.set_runtime_loaded(true);
            handle.set_model_status_text(status_for_ui.into());
            handle.set_assistant_output_text(
                "Runtime bereit. Prompt kann lokal ausgeführt werden.".into(),
            );
        });

        while let Ok(command) = receiver.recv() {
            match command {
                RuntimeCommand::Generate(prompt) => {
                    let result = model.generate(&prompt, 96);
                    let (output, status) = match result {
                        Ok(stats) => {
                            let tokens_per_second = stats.generated_tokens as f64
                                / stats.elapsed.as_secs_f64().max(0.001);
                            let status = format!(
                                "{loaded_status} · {} Tokens · {:.1} Token/s gesamt · {} ms gesamt",
                                stats.generated_tokens,
                                tokens_per_second,
                                stats.elapsed.as_millis()
                            );
                            (stats.text, status)
                        }
                        Err(error) => {
                            let message = format!("Inferenzfehler: {error}");
                            (message.clone(), message)
                        }
                    };
                    let _ = weak.upgrade_in_event_loop(move |handle| {
                        handle.set_assistant_output_text(output.into());
                        handle.set_model_status_text(status.into());
                        handle.set_busy(false);
                    });
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use livetab_core::model_registry::DEFAULT_ASSISTANT_MODEL_FILE;

    /// Zweck: Prüft die reale App-Modellwurzel und die feste Auswahl des mitgelieferten 4B-GGUF.
    /// Eingaben: Keine; verwendet das lokale App-Modellverzeichnis und dessen GGUF-Dateien.
    /// Ausgabe: Keine Rückgabe; Assertions bestätigen Pfad, Dateiname, Größe und erkannte Architektur.
    #[test]
    #[ignore = "requires the app-local Qwen3.5-4B Q4_K_M model asset"]
    fn app_local_registry_selects_fixed_qwen_4b_model() {
        let app_models_dir = app_models_directory();
        let snapshot = ModelRegistry::scan([&app_models_dir]);
        let selected = snapshot
            .assistant_model()
            .expect("the fixed Qwen3.5-4B model must exist in the app model folder");
        assert_eq!(selected.name, DEFAULT_ASSISTANT_MODEL_FILE);
        assert_eq!(selected.size_bytes, 2_783_447_520);
        assert_eq!(selected.architecture.as_deref(), Some("qwen35"));
        assert_eq!(selected.root, app_models_dir);
    }

    /// Zweck: Prüft, dass ein erfolgreicher BANDMAP-Import Version, Titel und lokalen Speicherort meldet.
    /// Eingaben: Keine; verwendet einen synthetischen Projektdatensatz und eine temporär wirkende Testwurzel.
    /// Ausgabe: Keine Rückgabe; Assertions bestätigen die für Nutzer sichtbaren Erfolgsdetails.
    #[test]
    fn bandmap_import_status_reports_success_and_saved_location() {
        let root = PathBuf::from("test-app-data");
        let record = ProjectRecord {
            id: "local-project-123".into(),
            project_id: "source-project-456".into(),
            title: "Probeprojekt".into(),
            format_version: 3,
            schema_version: Some(1),
            media_mode: "EMBEDDED".into(),
            created_at_unix_ms: Some(1),
            updated_at_unix_ms: None,
            imported_at_unix_ms: 2,
            archive_sha256: "abcd".into(),
        };

        let status = bandmap_import_status(Ok((root, record)));

        assert!(status.contains("Import erfolgreich"));
        assert!(status.contains("BANDMAP v3"));
        assert!(status.contains("Probeprojekt"));
        assert!(status.contains("local-project-123"));
    }

    /// Zweck: Prüft, dass die Oberfläche die Ursache eines fehlgeschlagenen BANDMAP-Imports anzeigt.
    /// Eingaben: Keine; verwendet einen repräsentativen Core-Validierungsfehler.
    /// Ausgabe: Keine Rückgabe; eine Assertion bestätigt die Fehlerart und deren Ursache im Status.
    #[test]
    fn bandmap_import_status_reports_validation_failure() {
        let status = bandmap_import_status(Err(
            "Die Eingabedatei muss die Endung .bandmap haben.".to_string()
        ));

        assert!(status.contains("Import fehlgeschlagen"));
        assert!(status.contains("Endung .bandmap"));
    }
}

/// Zweck: Aktualisiert den Runtime-Status, sofern das Slint-Fenster noch existiert.
/// Eingaben: Schwacher UI-Handle und anzuzeigende Statuszeichenkette.
/// Ausgabe: Keine Rückgabe; setzt bei gültigem Handle die UI-Statusanzeige.
fn set_runtime_status(weak: &Weak<MainWindow>, status: &str) {
    if let Some(handle) = weak.upgrade() {
        handle.set_model_status_text(status.into());
    }
}
