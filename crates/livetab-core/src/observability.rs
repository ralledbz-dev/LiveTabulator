/*
===============================================================================
Division By Zeros (DBZS) - Observability
Modul: observability
Zweck: Strukturierte, begrenzte Logs und Zustandswerte für Audio, DSP, Jobs,
       Modelle, Playback, Export und nutzerverständliche Fehler.
Sprache: Rust
Hinweise:
- CPU-/RAM-/GPU-Werte bleiben `None`, bis sie tatsächlich gemessen wurden.
- Der Ringpuffer begrenzt Diagnose-Speicherverbrauch und verschluckt Fehler nicht.
===============================================================================
*/
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Schweregrad eines strukturierten Observer-Eintrags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

/// Fachliche Observability-Kategorie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LogCategory {
    Audio,
    Dsp,
    BeatTracking,
    ChordDetection,
    KeyDetection,
    StemSeparation,
    Lyrics,
    Pitch,
    TabGeneration,
    Prediction,
    Playback,
    Export,
    Ui,
    Jobs,
    Settings,
    Models,
    Errors,
}

/// Maschinenlesbarer Logeintrag samt Kontext und optionaler Messdauer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub timestamp_unix_ms: u64,
    pub level: LogLevel,
    pub category: LogCategory,
    pub technical_message: String,
    pub user_message: String,
    pub context: BTreeMap<String, String>,
    pub elapsed_ms: Option<u128>,
}

/// Tatsächlich gemessene Ressourcenwerte; unbekannte Werte bleiben ausdrücklich `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareMetrics {
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub gpu_name: Option<String>,
    pub gpu_memory_used_bytes: Option<u64>,
    pub gpu_utilization_percent: Option<f32>,
    pub measured_at_unix_ms: Option<u64>,
}

#[derive(Debug, Default)]
struct ObserverState {
    logs: VecDeque<LogEntry>,
    max_entries: usize,
    audio_source: String,
    worker_status: String,
    model_status: String,
    hardware: HardwareMetrics,
}

/// Begrenzter, klonbarer Observability-Speicher für UI und Diagnoseexport.
#[derive(Debug, Clone)]
pub struct Observer {
    state: Arc<Mutex<ObserverState>>,
}

impl Observer {
    /// Zweck: Erstellt einen Observer mit begrenzter Log-Historie.
    /// Eingaben: Maximale Logeinträge; ein Wert null wird zu einer Mindestkapazität eins.
    /// Ausgabe: Neuer leerer Diagnosezustand.
    pub fn new(max_entries: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(ObserverState {
                logs: VecDeque::new(),
                max_entries: max_entries.max(1),
                audio_source: "unbekannt".into(),
                worker_status: "bereit".into(),
                model_status: "nicht geprüft".into(),
                hardware: HardwareMetrics::default(),
            })),
        }
    }

    /// Zweck: Fügt einen strukturierten Logeintrag hinzu und wirft nur den ältesten bei Kapazitätsüberschreitung weg.
    /// Eingaben: Level, Kategorie, technische Meldung, Nutzertext, Kontext und optionale Dauer.
    /// Ausgabe: Keine Rückgabe; Eintrag bleibt bis zur Ringpuffergrenze verfügbar.
    pub fn log(
        &self,
        level: LogLevel,
        category: LogCategory,
        technical_message: impl Into<String>,
        user_message: impl Into<String>,
        context: BTreeMap<String, String>,
        elapsed_ms: Option<u128>,
    ) {
        let entry = LogEntry {
            timestamp_unix_ms: unix_time_ms(),
            level,
            category,
            technical_message: technical_message.into(),
            user_message: user_message.into(),
            context,
            elapsed_ms,
        };
        if let Ok(mut state) = self.state.lock() {
            state.logs.push_back(entry);
            while state.logs.len() > state.max_entries {
                state.logs.pop_front();
            }
        }
    }

    /// Zweck: Aktualisiert die bekannte Audioquelle für Dev Observer und Diagnosepaket.
    /// Eingaben: Nutzerlesbarer Gerätestatus oder Dateiname.
    /// Ausgabe: Keine Rückgabe; überschreibt nur den Audioquellenstatus.
    pub fn set_audio_source(&self, source: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.audio_source = source.into();
        }
    }

    /// Zweck: Aktualisiert den Zustand lokaler Analyse-Worker.
    /// Eingaben: Kurzer Status ohne erfundene Hardwarewerte.
    /// Ausgabe: Keine Rückgabe; überschreibt den Workerstatus.
    pub fn set_worker_status(&self, status: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.worker_status = status.into();
        }
    }

    /// Zweck: Aktualisiert den validierten lokalen Modellstatus.
    /// Eingaben: Modell-, Runtime- oder Fehlermeldung.
    /// Ausgabe: Keine Rückgabe; speichert den Modellstatus für Observer.
    pub fn set_model_status(&self, status: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.model_status = status.into();
        }
    }

    /// Zweck: Speichert eine Messung und ergänzt den tatsächlichen Messzeitpunkt.
    /// Eingaben: `metrics` – nur vom Plattformadapter gemessene Werte; unbekannte Werte müssen `None` bleiben.
    /// Ausgabe: Keine Rückgabe; Messwerte sind anschließend abrufbar.
    pub fn set_hardware_metrics(&self, mut metrics: HardwareMetrics) {
        metrics.measured_at_unix_ms = Some(unix_time_ms());
        if let Ok(mut state) = self.state.lock() {
            state.hardware = metrics;
        }
    }

    /// Zweck: Liefert eine chronologische Kopie der begrenzten Loghistorie.
    /// Eingaben: `self` – Observer.
    /// Ausgabe: Logs vom ältesten zum neuesten Eintrag.
    pub fn logs(&self) -> Vec<LogEntry> {
        self.state
            .lock()
            .map(|state| state.logs.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Zweck: Formatiert die wichtigsten Diagnosebereiche als lesbares Textpanel.
    /// Eingaben: `self` – Observer.
    /// Ausgabe: Audio-, Worker-, Modell- und letzte Logmeldungen ohne erfundene Werte.
    pub fn panel_text(&self) -> String {
        let Ok(state) = self.state.lock() else {
            return "Observerstatus nicht verfügbar: Synchronisationsfehler.".into();
        };
        let mut rows = vec![
            format!("Audioquelle: {}", state.audio_source),
            format!("Worker: {}", state.worker_status),
            format!("Modelle: {}", state.model_status),
        ];
        if let Some(cpu) = state.hardware.cpu_percent {
            rows.push(format!("CPU: {:.1}% (gemessen)", cpu));
        } else {
            rows.push("CPU: unbekannt (kein Messwert)".into());
        }
        if let Some(memory) = state.hardware.memory_bytes {
            rows.push(format!(
                "RAM-Prozess: {} MiB (gemessen)",
                memory / 1_048_576
            ));
        } else {
            rows.push("RAM: unbekannt (kein Messwert)".into());
        }
        if let Some(gpu) = &state.hardware.gpu_name {
            rows.push(format!("GPU: {gpu}"));
        } else {
            rows.push("GPU: unbekannt (kein Messwert)".into());
        }
        for entry in state.logs.iter().rev().take(5).rev() {
            rows.push(format!(
                "{:?}/{:?}: {}",
                entry.category, entry.level, entry.user_message
            ));
        }
        rows.join("\n")
    }

    /// Zweck: Serialisiert Diagnosekontext für einen lokalen Diagnoseexport.
    /// Eingaben: `self` – Observerzustand.
    /// Ausgabe: JSON mit Zeitstempel, letzten Logs, Status und echten optionalen Messwerten.
    pub fn diagnostics_json(&self) -> Result<String, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Observerstatus konnte nicht gesperrt werden.".to_string())?;
        serde_json::to_string_pretty(&serde_json::json!({ "createdAtUnixMs": unix_time_ms(), "audioSource": state.audio_source, "workerStatus": state.worker_status, "modelStatus": state.model_status, "hardware": state.hardware, "logs": state.logs })).map_err(|error| format!("Diagnosepaket konnte nicht serialisiert werden: {error}"))
    }
}

impl Default for Observer {
    /// Zweck: Erstellt einen Observer mit 2.000 Logeinträgen.
    /// Eingaben: Keine.
    /// Ausgabe: Leerer, begrenzter Observer.
    fn default() -> Self {
        Self::new(2_000)
    }
}

/// Zweck: Ermittelt die aktuelle Unixzeit für Log- und Diagnoseeinträge.
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

    /// Zweck: Prüft begrenzte Loghistorie, Nutzerfehlertext und unbekannte Hardwarewerte.
    /// Eingaben: Keine; fügt drei Logs in einen Observer mit Kapazität zwei ein.
    /// Ausgabe: Keine Rückgabe; Assertions sichern die zwei neuesten Einträge und ehrliches Unknown.
    #[test]
    fn keeps_bounded_logs_and_does_not_invent_hardware_values() {
        let observer = Observer::new(2);
        for index in 0..3 {
            observer.log(
                LogLevel::Info,
                LogCategory::Audio,
                format!("tech {index}"),
                format!("user {index}"),
                BTreeMap::new(),
                None,
            );
        }
        let logs = observer.logs();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].technical_message, "tech 1");
        assert!(observer.panel_text().contains("CPU: unbekannt"));
        assert!(observer.diagnostics_json().unwrap().contains("user 2"));
    }
}
