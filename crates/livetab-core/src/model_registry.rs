use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

pub const MODEL_PATHS_ENV: &str = "LIVETAB_MODEL_PATHS";
pub const DEFAULT_ASSISTANT_MODEL_FILE: &str = "Qwen3.5-4B-Q4_K_M.gguf";
#[cfg(test)]
const DEFAULT_ASSISTANT_MODEL_ID: &str = "qwen3_5_4b_q4_k_m";
const GGUF_HEADER_BYTES: u64 = 24;
const MAX_METADATA_KV: u64 = 1_000_000;
const MAX_ARRAY_ITEMS: u64 = 1_000_000;
const MAX_STRING_BYTES: u64 = 1_048_576;
const MAX_METADATA_BYTES: u64 = 256 * 1024 * 1024;
const PROGRESS_INTERVAL: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    Gguf,
    ExecuTorch,
    Onnx,
    PyTorchCheckpoint,
    TensorFlowLite,
    KerasCheckpoint,
    VampPlugin,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    LanguageModel,
    SpeechRecognition,
    SourceSeparation,
    PitchTracking,
    VoiceActivityDetection,
    ChordAnalysis,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    /// GGUF-Header/Architektur oder ein unterstütztes lokales Modellbundle wurden strukturell geprüft.
    Validated,
    /// The file extension/path is relevant, but its format/runtime has not been validated.
    DetectedUnverified,
    /// The path looks like a model but the GGUF header/metadata is malformed or unsupported.
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub root: PathBuf,
    pub root_order: usize,
    pub size_bytes: u64,
    pub format: ModelFormat,
    pub role: ModelRole,
    pub status: ModelStatus,
    pub architecture: Option<String>,
    pub detail: String,
}

impl ModelEntry {
    /// Zweck: Prüft fail-closed, ob ein Eintrag als validiertes GGUF-Sprachmodell für llama.cpp zugelassen ist.
    /// Eingaben: `self` – der zu prüfende Modelleintrag.
    /// Ausgabe: `true` nur für ein validiertes GGUF-Sprachmodell, sonst `false`.
    pub fn can_load_with_llama_cpp(&self) -> bool {
        self.format == ModelFormat::Gguf
            && self.role == ModelRole::LanguageModel
            && self.status == ModelStatus::Validated
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootScan {
    pub path: PathBuf,
    pub exists: bool,
    pub files_scanned: usize,
    pub model_files_found: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    LocalAssistant,
    SpeechRecognition,
    SourceSeparation,
    PitchTracking,
    VoiceActivityDetection,
    ChordRecognition,
    KeyEstimation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    CoreImplemented,
    ModelAvailable,
    ModelMissing,
    RuntimePending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerStatus {
    pub capability: Capability,
    pub state: CapabilityState,
    pub model_id: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub roots: Vec<RootScan>,
    pub entries: Vec<ModelEntry>,
    pub warnings: Vec<String>,
    pub elapsed_ms: u128,
}

impl RegistrySnapshot {
    /// Zweck: Ermittelt den Integrationsstatus der Assistant-, Sprach-, Trennungs-, Pitch-, VAD-, Akkord- und Tonart-Verbraucher.
    /// Eingaben: `self` – Registry-Snapshot mit geprüften Modellen.
    /// Ausgabe: Eine Liste aus Fähigkeiten, Status, optionaler Modell-ID und Erläuterung.
    pub fn consumers(&self) -> Vec<ConsumerStatus> {
        let find_role = |role| self.entries.iter().find(|entry| entry.role == role);
        let assistant = self.assistant_model();
        let status_for = |capability, role, missing_detail: &str| match find_role(role) {
            Some(entry) => ConsumerStatus {
                capability,
                state: CapabilityState::RuntimePending,
                model_id: Some(entry.id.clone()),
                detail: format!(
                    "{}; der passende Analyse-Runtime-Pfad ist noch nicht eingebunden.",
                    entry.detail
                ),
            },
            None => ConsumerStatus {
                capability,
                state: CapabilityState::ModelMissing,
                model_id: None,
                detail: missing_detail.to_string(),
            },
        };

        vec![
            match assistant {
                Some(entry) => ConsumerStatus {
                    capability: Capability::LocalAssistant,
                    state: CapabilityState::ModelAvailable,
                    model_id: Some(entry.id.clone()),
                    detail: format!(
                        "Festes Assistant-Standardmodell {} validiert; llama.cpp-Laden erfolgt im Hintergrund.",
                        entry.name
                    ),
                },
                None => ConsumerStatus {
                    capability: Capability::LocalAssistant,
                    state: CapabilityState::ModelMissing,
                    model_id: None,
                    detail: format!(
                        "Festes Assistant-Standardmodell {} fehlt oder ist nicht validiert; andere Textmodelle werden nicht ersatzweise ausgewählt.",
                        DEFAULT_ASSISTANT_MODEL_FILE
                    ),
                },
            },
            status_for(Capability::SpeechRecognition, ModelRole::SpeechRecognition, "Kein Whisper-/ASR-Modell gefunden."),
            status_for(Capability::SourceSeparation, ModelRole::SourceSeparation, "Kein Demucs-/Stem-Separation-Modell gefunden."),
            status_for(Capability::PitchTracking, ModelRole::PitchTracking, "Kein Pitch-/BasicPitch-/CREPE-Modell gefunden."),
            status_for(Capability::VoiceActivityDetection, ModelRole::VoiceActivityDetection, "Kein VAD-Modell gefunden."),
            ConsumerStatus {
                capability: Capability::ChordRecognition,
                state: CapabilityState::CoreImplemented,
                model_id: None,
                detail: "Rust-Core enthält einen chroma-/templatebasierten Akkordklassifizierer; er benötigt kein LLM.".into(),
            },
            ConsumerStatus {
                capability: Capability::KeyEstimation,
                state: CapabilityState::CoreImplemented,
                model_id: None,
                detail: "Rust-Core enthält eine Krumhansl-Schmuckler-Tonartenschätzung; sie benötigt kein LLM.".into(),
            },
        ]
    }

    /// Zweck: Wählt ausschließlich das fest konfigurierte Qwen3.5-4B-Q4_K_M als Assistant-Standardmodell.
    /// Eingaben: `self` – Registry-Snapshot mit geprüften Modell-Einträgen.
    /// Ausgabe: Eine Referenz auf die validierte Standarddatei oder `None`; andere Modelle sind kein Ersatz.
    pub fn assistant_model(&self) -> Option<&ModelEntry> {
        self.entries.iter().find(|entry| {
            entry.name == DEFAULT_ASSISTANT_MODEL_FILE && entry.can_load_with_llama_cpp()
        })
    }
}

pub struct ModelRegistry;

impl ModelRegistry {
    /// Zweck: Erstellt die geordnete Liste der App-, Nutzer- und plattformspezifischen Modellwurzeln.
    /// Eingaben: `app_models_dir` – Modellordner der Anwendung; optional `LIVETAB_MODEL_PATHS`.
    /// Ausgabe: Deduplizierte Suchpfade; unter Windows gehört `D:\Models` dazu.
    pub fn default_roots(app_models_dir: &Path) -> Vec<PathBuf> {
        let mut roots = vec![app_models_dir.to_path_buf()];
        if let Some(extra) = std::env::var_os(MODEL_PATHS_ENV) {
            roots.extend(std::env::split_paths(&extra));
        }
        #[cfg(windows)]
        roots.push(PathBuf::from(r"D:\Models"));
        deduplicate_roots(roots)
    }

    /// Zweck: Durchsucht die angegebenen Wurzeln rekursiv ohne Fortschrittsmeldungen.
    /// Eingaben: `roots` – Iterator aus Pfaden oder pfadähnlichen Werten.
    /// Ausgabe: Ein `RegistrySnapshot` mit Modellen, Wurzelstatistiken, Warnungen und Dauer.
    pub fn scan<I, P>(roots: I) -> RegistrySnapshot
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        Self::scan_with_progress(roots, |_, _, _| {})
    }

    /// Zweck: Durchsucht Modellwurzeln rekursiv und meldet regelmäßig Datei-/Kandidatenfortschritt.
    /// Eingaben: `roots` – Suchpfade; `on_progress` – Callback mit Wurzel, Dateizahl und Kandidatenzahl.
    /// Ausgabe: Ein `RegistrySnapshot`; der Callback wird während des Scans aufgerufen.
    pub fn scan_with_progress<I, P, F>(roots: I, mut on_progress: F) -> RegistrySnapshot
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
        F: FnMut(&Path, usize, usize),
    {
        let started = Instant::now();
        let mut root_reports = Vec::new();
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        let mut seen = HashSet::new();

        for (root_order, root_value) in roots.into_iter().enumerate() {
            let root = root_value.as_ref().to_path_buf();
            let exists = root.is_dir();
            let mut report = RootScan {
                path: root.clone(),
                exists,
                files_scanned: 0,
                model_files_found: 0,
            };
            if !exists {
                warnings.push(format!(
                    "Modellwurzel nicht vorhanden oder nicht lesbar: {}",
                    root.display()
                ));
                root_reports.push(report);
                continue;
            }

            on_progress(&root, 0, 0);
            for item in WalkDir::new(&root)
                .follow_links(false)
                .into_iter()
                .filter_entry(should_visit_directory)
            {
                let entry = match item {
                    Ok(entry) => entry,
                    Err(error) => {
                        if let Some(path) = error.path() {
                            warnings
                                .push(format!("Übersprungen (Dateizugriff): {}", path.display()));
                        }
                        continue;
                    }
                };
                let path = entry.path();
                let candidate = is_model_candidate(path);
                let file_type = entry.file_type();
                if file_type.is_dir() {
                    if let Some(model) = inspect_faster_whisper_directory(path, &root, root_order) {
                        let key = normalized_path_key(&model.path);
                        if seen.insert(key) {
                            report.model_files_found += 1;
                            entries.push(model);
                        }
                    }
                    continue;
                }
                if !file_type.is_file()
                    && !(candidate
                        && file_type.is_symlink()
                        && std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
                {
                    continue;
                }
                report.files_scanned += 1;
                if report.files_scanned % PROGRESS_INTERVAL == 0 {
                    on_progress(&root, report.files_scanned, report.model_files_found);
                }
                if !candidate {
                    continue;
                }
                let key = normalized_path_key(path);
                if !seen.insert(key) {
                    continue;
                }
                report.model_files_found += 1;
                match inspect_model(path, &root, root_order) {
                    Ok(model) => entries.push(model),
                    Err(error) => warnings.push(format!(
                        "Modell nicht lesbar ({}): {}",
                        error,
                        path.display()
                    )),
                }
            }
            on_progress(&root, report.files_scanned, report.model_files_found);
            root_reports.push(report);
        }

        entries.sort_by(|left, right| {
            left.root_order
                .cmp(&right.root_order)
                .then_with(|| left.path.cmp(&right.path))
        });
        RegistrySnapshot {
            roots: root_reports,
            entries,
            warnings,
            elapsed_ms: started.elapsed().as_millis(),
        }
    }
}

/// Zweck: Verhindert das Betreten bekannter Build-, Cache-, Venv- und Abhängigkeitsordner.
/// Eingaben: `entry` – aktueller WalkDir-Eintrag.
/// Ausgabe: `true`, wenn der Scanner den Eintrag besuchen soll, sonst `false`.
fn should_visit_directory(entry: &walkdir::DirEntry) -> bool {
    const SKIP_DIRECTORIES: &[&str] = &[
        ".git",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        ".venv",
        "__pycache__",
        "build",
        "dist",
        "node_modules",
        "target",
        "venv",
    ];
    if !entry.file_type().is_dir() {
        return true;
    }
    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
    !SKIP_DIRECTORIES.contains(&name.as_str())
}

/// Zweck: Entfernt doppelte Suchwurzeln anhand plattformnormalisierter Pfadschlüssel.
/// Eingaben: `roots` – geordnete Liste zu prüfender Pfade.
/// Ausgabe: Eine Liste eindeutiger Pfade mit erhaltener Erstfund-Reihenfolge.
fn deduplicate_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter(|path| seen.insert(normalized_path_key(path)))
        .collect()
}

/// Zweck: Erzeugt einen vergleichbaren Deduplizierungsschlüssel für einen Pfad.
/// Eingaben: `path` – beliebiger Dateisystempfad.
/// Ausgabe: Unter Windows kleingeschriebene, sonst unveränderte Pfadzeichenkette.
fn normalized_path_key(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// Zweck: Filtert anhand der Dateiendung und begrenzter Namenshinweise mögliche Modellartefakte.
/// Eingaben: `path` – Pfad des gefundenen Dateieintrags.
/// Ausgabe: `true`, wenn die Datei zur Formatprüfung vorgemerkt wird, sonst `false`.
fn is_model_candidate(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "gguf" | "pte" | "onnx" | "pt" | "pth" | "th" | "tflite" | "h5" | "vamp" => true,
        "bin" => path_hints_role(path) == ModelRole::SpeechRecognition,
        "dll" => path.to_string_lossy().to_ascii_lowercase().contains("vamp"),
        _ => false,
    }
}

/// Zweck: Prüft die minimale CTranslate2-/faster-whisper-Verzeichnisstruktur für lokale Worttranskription.
/// Eingaben: Verzeichnis, Suchwurzel und deren Reihenfolge aus der Modellregistry.
/// Ausgabe: Validierter ASR-Verzeichniseintrag oder `None`; Runtime-Verfügbarkeit wird nicht vorgetäuscht.
fn inspect_faster_whisper_directory(
    path: &Path,
    root: &Path,
    root_order: usize,
) -> Option<ModelEntry> {
    let model_file = path.join("model.bin");
    let config_file = path.join("config.json");
    let tokenizer_file = path.join("tokenizer.json");
    if !model_file.is_file() || !config_file.is_file() || !tokenizer_file.is_file() {
        return None;
    }
    let size_bytes = std::fs::metadata(&model_file).ok()?.len();
    if size_bytes == 0 {
        return None;
    }
    let config = std::fs::read(&config_file).ok()?;
    let parsed: serde_json::Value = serde_json::from_slice(&config).ok()?;
    if !parsed.is_object() {
        return None;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("faster-whisper")
        .to_string();
    Some(ModelEntry {
        id: format!("faster-whisper:{}", path.display()),
        name,
        path: path.to_path_buf(),
        root: root.to_path_buf(),
        root_order,
        size_bytes,
        format: ModelFormat::Other,
        role: ModelRole::SpeechRecognition,
        status: ModelStatus::Validated,
        architecture: Some("ctranslate2-whisper".into()),
        detail: "Lokales faster-whisper-Modellbundle strukturell geprüft; Python-/Runtime-Verfügbarkeit wird erst beim Jobstart geprüft.".into(),
    })
}

/// Zweck: Prüft Dateimetadaten, erkennt Format/Rolle und validiert GGUF-Header vor der Runtime-Freigabe.
/// Eingaben: Kandidatenpfad, zugehörige Suchwurzel und deren Reihenfolge.
/// Ausgabe: `ModelEntry` in `Ok` oder ein Dateisystemfehler.
fn inspect_model(path: &Path, root: &Path, root_order: usize) -> io::Result<ModelEntry> {
    let metadata = std::fs::metadata(path)?;
    let size_bytes = metadata.len();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model")
        .to_string();
    let mut architecture = None;

    let (format, role, status, detail) = if extension == "gguf" {
        match read_gguf_info(path) {
            Ok(info) => {
                architecture = Some(info.architecture.clone());
                let role = role_from_gguf_architecture(&info.architecture, path);
                let detail = match role {
                    ModelRole::LanguageModel => format!(
                        "GGUF v{}; Textmodell-Architektur {} erkannt.",
                        info.version, info.architecture
                    ),
                    _ => format!(
                        "GGUF v{}; Architektur {} erkannt, nicht als Chatmodell freigegeben.",
                        info.version, info.architecture
                    ),
                };
                (ModelFormat::Gguf, role, ModelStatus::Validated, detail)
            }
            Err(error) => (
                ModelFormat::Gguf,
                path_hints_role(path),
                ModelStatus::Invalid,
                format!("GGUF-Datei abgelehnt: {error}"),
            ),
        }
    } else {
        let format = match extension.as_str() {
            "pte" => ModelFormat::ExecuTorch,
            "onnx" => ModelFormat::Onnx,
            "pt" | "pth" | "th" | "bin" => ModelFormat::PyTorchCheckpoint,
            "tflite" => ModelFormat::TensorFlowLite,
            "h5" => ModelFormat::KerasCheckpoint,
            "vamp" | "dll" => ModelFormat::VampPlugin,
            _ => ModelFormat::Other,
        };
        let role = if format == ModelFormat::VampPlugin {
            ModelRole::ChordAnalysis
        } else {
            path_hints_role(path)
        };
        (
            format,
            role,
            ModelStatus::DetectedUnverified,
            "Datei/Modellname rekursiv entdeckt; Modellformat, Lizenz und Ziel-Runtime noch nicht geladen/verifiziert.".into(),
        )
    };

    Ok(ModelEntry {
        id: format!("{}", path.display()),
        name,
        path: path.to_path_buf(),
        root: root.to_path_buf(),
        root_order,
        size_bytes,
        format,
        role,
        status,
        architecture,
        detail,
    })
}

#[derive(Debug)]
struct GgufInfo {
    version: u32,
    architecture: String,
    _name: Option<String>,
}

/// Zweck: Liest und validiert GGUF-Kennung, Version und begrenzte Architektur-/Namensmetadaten.
/// Eingaben: `path` – GGUF-Datei.
/// Ausgabe: Version und Architektur als `GgufInfo` oder eine Ablehnungsbegründung.
fn read_gguf_info(path: &Path) -> Result<GgufInfo, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let file_len = file.metadata().map_err(|error| error.to_string())?.len();
    if file_len < GGUF_HEADER_BYTES {
        return Err("Datei ist kürzer als ein GGUF-Header".into());
    }
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic)
        .map_err(|error| error.to_string())?;
    if &magic != b"GGUF" {
        return Err("GGUF-Kennung fehlt".into());
    }
    let version = read_u32(&mut file).map_err(|error| error.to_string())?;
    if !(2..=3).contains(&version) {
        return Err(format!("nicht unterstützte GGUF-Version {version}"));
    }
    let tensor_count = read_u64(&mut file).map_err(|error| error.to_string())?;
    let kv_count = read_u64(&mut file).map_err(|error| error.to_string())?;
    if tensor_count == 0 || kv_count > MAX_METADATA_KV {
        return Err("unplausible Tensor-/Metadatenanzahl".into());
    }

    let mut architecture = None;
    let mut model_name = None;
    for _ in 0..kv_count {
        let position = file.stream_position().map_err(|error| error.to_string())?;
        if position > GGUF_HEADER_BYTES + MAX_METADATA_BYTES {
            return Err("GGUF-Metadaten überschreiten das Sicherheitslimit".into());
        }
        let key = read_gguf_string(&mut file, file_len).map_err(|error| error.to_string())?;
        let value_type = read_u32(&mut file).map_err(|error| error.to_string())?;
        if key == "general.architecture" && value_type == 8 {
            architecture =
                Some(read_gguf_string(&mut file, file_len).map_err(|error| error.to_string())?);
        } else if key == "general.name" && value_type == 8 {
            model_name =
                Some(read_gguf_string(&mut file, file_len).map_err(|error| error.to_string())?);
        } else {
            skip_gguf_value(&mut file, value_type, file_len, 0)
                .map_err(|error| error.to_string())?;
        }
    }
    let architecture = architecture.ok_or_else(|| "general.architecture fehlt".to_string())?;
    Ok(GgufInfo {
        version,
        architecture,
        _name: model_name,
    })
}

/// Zweck: Liest einen vorzeichenlosen 32-Bit-Wert im GGUF-Little-Endian-Format.
/// Eingaben: Veränderbarer Dateihandle an der aktuellen Leseposition.
/// Ausgabe: Der gelesene `u32`-Wert oder ein E/A-Fehler.
fn read_u32(file: &mut File) -> io::Result<u32> {
    let mut bytes = [0_u8; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// Zweck: Liest einen vorzeichenlosen 64-Bit-Wert im GGUF-Little-Endian-Format.
/// Eingaben: Veränderbarer Dateihandle an der aktuellen Leseposition.
/// Ausgabe: Der gelesene `u64`-Wert oder ein E/A-Fehler.
fn read_u64(file: &mut File) -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    file.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

/// Zweck: Liest einen längenpräfixierten GGUF-UTF-8-Textwert mit Größen- und Dateigrenzenprüfung.
/// Eingaben: Veränderbarer Dateihandle und tatsächliche Dateilänge.
/// Ausgabe: Der UTF-8-Text oder ein E/A-/Formatfehler.
fn read_gguf_string(file: &mut File, file_len: u64) -> io::Result<String> {
    let length = read_u64(file)?;
    if length > MAX_STRING_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GGUF string exceeds limit",
        ));
    }
    ensure_range(file, length, file_len)?;
    let mut bytes = vec![0_u8; length as usize];
    file.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid UTF-8 in GGUF metadata"))
}

/// Zweck: Überspringt einen GGUF-String, ohne dessen Inhalt in den Speicher zu laden.
/// Eingaben: Veränderbarer Dateihandle und tatsächliche Dateilänge.
/// Ausgabe: `Ok(())` nach sicherem Überspringen oder ein Grenz-/E/A-Fehler.
fn skip_gguf_string(file: &mut File, file_len: u64) -> io::Result<()> {
    let length = read_u64(file)?;
    if length > MAX_METADATA_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GGUF metadata string exceeds limit",
        ));
    }
    skip_checked(file, length, file_len)
}

/// Zweck: Überspringt typisierte GGUF-Metadatenwerte und Arrays rekursiv mit Größen-/Tiefenlimits.
/// Eingaben: Dateihandle, GGUF-Werttyp, Dateilänge und aktuelle Array-Verschachtelung.
/// Ausgabe: `Ok(())` nach sicherem Überspringen oder ein Format-/E/A-Fehler.
fn skip_gguf_value(file: &mut File, value_type: u32, file_len: u64, depth: u8) -> io::Result<()> {
    if depth > 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GGUF array nesting exceeds limit",
        ));
    }
    let fixed_width = match value_type {
        0 | 1 | 7 => Some(1_u64),
        2 | 3 => Some(2),
        4 | 5 | 6 => Some(4),
        10 | 11 | 12 => Some(8),
        8 => return skip_gguf_string(file, file_len),
        9 => {
            let element_type = read_u32(file)?;
            let count = read_u64(file)?;
            if count > MAX_ARRAY_ITEMS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "GGUF array exceeds item limit",
                ));
            }
            if let Some(width) = metadata_width(element_type) {
                let bytes = count.checked_mul(width).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "GGUF array size overflow")
                })?;
                return skip_checked(file, bytes, file_len);
            }
            for _ in 0..count {
                skip_gguf_value(file, element_type, file_len, depth + 1)?;
            }
            return Ok(());
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown GGUF metadata type {value_type}"),
            ));
        }
    };
    let width = fixed_width.expect("fixed width set for numeric metadata type");
    skip_checked(file, width, file_len)
}

/// Zweck: Bestimmt die feste Bytebreite eines skalaren GGUF-Metadatentyps.
/// Eingaben: `value_type` – GGUF-Typkennung.
/// Ausgabe: Bytebreite als `Some`, bei variablem oder unbekanntem Typ `None`.
fn metadata_width(value_type: u32) -> Option<u64> {
    match value_type {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4 | 5 | 6 => Some(4),
        10 | 11 | 12 => Some(8),
        _ => None,
    }
}

/// Zweck: Prüft mit Überlaufkontrolle, ob eine angeforderte Dateispanne vollständig innerhalb der Datei liegt.
/// Eingaben: Dateihandle, benötigte Bytezahl und Dateilänge.
/// Ausgabe: `Ok(())` bei gültiger Spanne, sonst ein Bereichs-/Dateiende-Fehler.
fn ensure_range(file: &mut File, amount: u64, file_len: u64) -> io::Result<()> {
    let position = file.stream_position()?;
    let end = position
        .checked_add(amount)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "GGUF offset overflow"))?;
    if end > file_len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated GGUF metadata",
        ));
    }
    Ok(())
}

/// Zweck: Überspringt Bytes erst nach erfolgreicher Bereichsprüfung.
/// Eingaben: Dateihandle, zu überspringende Bytezahl und Dateilänge.
/// Ausgabe: `Ok(())` nach dem Seek oder ein E/A-/Grenzfehler.
fn skip_checked(file: &mut File, amount: u64, file_len: u64) -> io::Result<()> {
    ensure_range(file, amount, file_len)?;
    file.seek(SeekFrom::Current(amount as i64))?;
    Ok(())
}

/// Zweck: Ordnet bekannte GGUF-Architekturen sicher einer Modellrolle zu.
/// Eingaben: Architekturkennung aus Metadaten und Dateipfad als ergänzender Hinweis.
/// Ausgabe: Erkannte `ModelRole`; unbekannte Architekturen bleiben unbekannt.
fn role_from_gguf_architecture(architecture: &str, path: &Path) -> ModelRole {
    let normalized = architecture.to_ascii_lowercase().replace(['-', '.'], "");
    if matches!(normalized.as_str(), "whisper" | "whisperencoder") {
        return ModelRole::SpeechRecognition;
    }
    if matches!(normalized.as_str(), "pulsevad" | "vad") {
        return ModelRole::VoiceActivityDetection;
    }
    const TEXT_ARCHITECTURES: &[&str] = &[
        "llama",
        "mistral",
        "mixtral",
        "qwen2",
        "qwen2moe",
        "qwen3",
        "qwen3moe",
        "qwen35",
        "qwen35moe",
        "gemma",
        "gemma2",
        "gemma3",
        "phi2",
        "phi3",
        "phi4",
        "phi4moe",
        "commandr",
        "commandrplus",
        "deepseek",
        "deepseek2",
        "deepseek3",
        "deepseek4",
        "granite",
        "jamba",
        "falcon",
        "openelm",
        "starcoder",
        "starcoder2",
        "bloom",
        "rwkv6",
        "dbrx",
        "nemotron",
        "olmo",
        "olmo2",
        "smollm",
        "exaone",
        "cohere2",
        "internlm2",
        "minicpm",
        "stablelm",
        "xverse",
        "plamo",
        "solar",
        "arctic",
        "chameleon",
        "marco",
        "hunyuan",
        "orion",
        "baichuan",
        "yi",
        "mamba",
        "mamba2",
        "granitemoe",
    ];
    if TEXT_ARCHITECTURES.contains(&normalized.as_str()) {
        ModelRole::LanguageModel
    } else {
        let path_role = path_hints_role(path);
        if path_role == ModelRole::Unknown {
            ModelRole::Unknown
        } else {
            path_role
        }
    }
}

/// Zweck: Leitet eine vorsichtige Modellrollen-Vermutung aus Pfad-/Dateinamenshinweisen ab.
/// Eingaben: `path` – Modell- oder Artefaktpfad.
/// Ausgabe: Eine erkannte Rolle oder `ModelRole::Unknown`.
fn path_hints_role(path: &Path) -> ModelRole {
    let text = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('_', "-");
    if [
        "whisper",
        "moonshine",
        "faster-whisper",
        "asr",
        "speech-to-text",
    ]
    .iter()
    .any(|hint| text.contains(hint))
    {
        ModelRole::SpeechRecognition
    } else if ["demucs", "htdemucs", "stem-separation", "source-separation"]
        .iter()
        .any(|hint| text.contains(hint))
    {
        ModelRole::SourceSeparation
    } else if [
        "basicpitch",
        "basic-pitch",
        "crepe",
        "pitch-tracking",
        "pitch-model",
    ]
    .iter()
    .any(|hint| text.contains(hint))
    {
        ModelRole::PitchTracking
    } else if ["pulsevad", "voice-activity", "vad-model"]
        .iter()
        .any(|hint| text.contains(hint))
    {
        ModelRole::VoiceActivityDetection
    } else if ["chordino", "btc-chord", "chord-model"]
        .iter()
        .any(|hint| text.contains(hint))
    {
        ModelRole::ChordAnalysis
    } else {
        ModelRole::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Zweck: Erzeugt eine minimale synthetische GGUF-Datei als Registry-Testfixture.
    /// Eingaben: Zielpfad und Architekturkennung.
    /// Ausgabe: Keine Rückgabe; schreibt Fixture-Bytes in die Testdatei.
    fn write_gguf(path: &Path, architecture: &str) {
        let mut file = File::create(path).expect("create fixture");
        file.write_all(b"GGUF").unwrap();
        file.write_all(&3_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u64.to_le_bytes()).unwrap();
        file.write_all(&1_u64.to_le_bytes()).unwrap();
        write_gguf_string(&mut file, "general.architecture");
        file.write_all(&8_u32.to_le_bytes()).unwrap();
        write_gguf_string(&mut file, architecture);
    }

    /// Zweck: Schreibt einen längenpräfixierten UTF-8-String in eine Test-GGUF-Datei.
    /// Eingaben: Veränderbarer Dateihandle und zu serialisierender Text.
    /// Ausgabe: Keine Rückgabe; schreibt Länge und Textbytes in die Datei.
    fn write_gguf_string(file: &mut File, value: &str) {
        file.write_all(&(value.len() as u64).to_le_bytes()).unwrap();
        file.write_all(value.as_bytes()).unwrap();
    }

    /// Zweck: Prüft rekursive Suche, GGUF-Validierung und Freigabe des Textmodell-Verbrauchers.
    /// Eingaben: Keine; der Test erzeugt eine temporäre GGUF-Fixture.
    /// Ausgabe: Keine Rückgabe; Assertions verifizieren Eintrag, Auswahl und Verbraucherstatus.
    #[test]
    fn recursively_finds_and_validates_text_gguf() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("nested/llm");
        std::fs::create_dir_all(&nested).unwrap();
        let model = nested.join(DEFAULT_ASSISTANT_MODEL_FILE);
        write_gguf(&model, "qwen35");
        let snapshot = ModelRegistry::scan([temp.path()]);
        assert_eq!(snapshot.entries.len(), 1);
        assert!(snapshot.entries[0].can_load_with_llama_cpp());
        assert_eq!(snapshot.assistant_model().unwrap().path, model);
        assert_eq!(
            snapshot.consumers()[0].state,
            CapabilityState::ModelAvailable
        );
    }

    /// Zweck: Prüft die strukturelle Erkennung eines lokalen faster-whisper-Ordners ohne Python-Runtime-Fallback.
    /// Eingaben: Temporärer ASR-Ordner mit Modellbinary, JSON-Konfiguration und Tokenizerfixture.
    /// Ausgabe: Keine Rückgabe; Assertions trennen lokales Bundle von zur Laufzeit noch zu prüfender Workerabhängigkeit.
    #[test]
    fn recognizes_local_faster_whisper_directory_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let model = temp.path().join("asr/large-v3");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(model.join("model.bin"), b"local-model-fixture").unwrap();
        std::fs::write(model.join("config.json"), b"{}").unwrap();
        std::fs::write(model.join("tokenizer.json"), b"{}").unwrap();
        let snapshot = ModelRegistry::scan([temp.path()]);
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.path == model)
            .unwrap();
        assert_eq!(entry.role, ModelRole::SpeechRecognition);
        assert_eq!(entry.status, ModelStatus::Validated);
        assert_eq!(entry.architecture.as_deref(), Some("ctranslate2-whisper"));
        assert_eq!(
            snapshot.consumers()[1].state,
            CapabilityState::RuntimePending
        );
    }

    /// Zweck: Stellt sicher, dass ein anderes validiertes Qwen-Modell nicht anstelle des festen 4B-Defaults gewählt wird.
    /// Eingaben: Keine; der Test erzeugt eine temporäre GGUF-Fixture für ein alternatives Qwen-Modell.
    /// Ausgabe: Keine Rückgabe; Assertions bestätigen Validierung und fehlende Assistant-Standardauswahl.
    #[test]
    fn does_not_select_other_qwen_model_as_fixed_default() {
        let temp = tempfile::tempdir().unwrap();
        let model = temp.path().join("alternate-qwen-Q4_0.gguf");
        write_gguf(&model, "qwen35");
        let snapshot = ModelRegistry::scan([temp.path()]);
        assert!(snapshot.entries[0].can_load_with_llama_cpp());
        assert!(snapshot.assistant_model().is_none());
        assert_eq!(snapshot.consumers()[0].state, CapabilityState::ModelMissing);
    }

    /// Zweck: Prüft, dass Manifest-ID und Standarddateiname mit der fest codierten Registry-Auswahl übereinstimmen.
    /// Eingaben: Keine; verwendet das app-lokale Modellmanifest zur Compilezeit.
    /// Ausgabe: Keine Rückgabe; Assertions schlagen bei einer inkonsistenten Defaultkonfiguration fehl.
    #[test]
    fn fixed_default_matches_app_model_manifest() {
        let manifest = include_str!("../../livetab-app/models/manifest.json");
        assert!(manifest.contains(&format!(
            "\"default_model_id\": \"{DEFAULT_ASSISTANT_MODEL_ID}\""
        )));
        assert!(manifest.contains(&format!("\"filename\": \"{DEFAULT_ASSISTANT_MODEL_FILE}\"")));
        assert!(manifest.contains("\"is_default\": true"));
    }

    /// Zweck: Stellt sicher, dass ein VAD-GGUF nicht fälschlich als Chatmodell verwendet wird.
    /// Eingaben: Keine; der Test erzeugt eine temporäre PulseVAD-Fixture.
    /// Ausgabe: Keine Rückgabe; Assertions prüfen Rolle und fehlende Assistant-Auswahl.
    #[test]
    fn does_not_treat_non_text_gguf_as_chat_model() {
        let temp = tempfile::tempdir().unwrap();
        let model = temp.path().join("pulsevad-f32.gguf");
        write_gguf(&model, "pulsevad");
        let snapshot = ModelRegistry::scan([temp.path()]);
        assert_eq!(snapshot.entries[0].role, ModelRole::VoiceActivityDetection);
        assert!(snapshot.assistant_model().is_none());
    }

    /// Zweck: Prüft, dass fehlerhafte GGUF-Dateien nicht für llama.cpp freigegeben werden.
    /// Eingaben: Keine; der Test erzeugt eine temporäre Datei mit ungültigem Inhalt.
    /// Ausgabe: Keine Rückgabe; Assertions erwarten Status `Invalid` und keine Assistant-Auswahl.
    #[test]
    fn malformed_gguf_is_not_loadable() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("bad.gguf"), b"not-a-model").unwrap();
        let snapshot = ModelRegistry::scan([temp.path()]);
        assert_eq!(snapshot.entries[0].status, ModelStatus::Invalid);
        assert!(snapshot.assistant_model().is_none());
    }

    /// Zweck: Prüft die Erkennung eines ExecuTorch-Sprachmodells ohne eine nicht vorhandene Runtime vorzutäuschen.
    /// Eingaben: Keine; der Test erzeugt eine temporäre `.pte`-Fixture im ASR-Pfad.
    /// Ausgabe: Keine Rückgabe; Assertions erwarten erkannte Rolle, unverifizierten Eintrag und RuntimePending.
    #[test]
    fn recognizes_execu_torch_as_unverified_and_keeps_runtime_boundary_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let asr = temp.path().join("moonshine");
        std::fs::create_dir_all(&asr).unwrap();
        std::fs::write(asr.join("encoder.pte"), b"fixture").unwrap();
        let snapshot = ModelRegistry::scan([temp.path()]);
        assert_eq!(snapshot.entries[0].format, ModelFormat::ExecuTorch);
        assert_eq!(snapshot.entries[0].role, ModelRole::SpeechRecognition);
        assert_eq!(snapshot.entries[0].status, ModelStatus::DetectedUnverified);
        assert_eq!(
            snapshot.consumers()[1].state,
            CapabilityState::RuntimePending
        );
    }

    /// Zweck: Prüft Reihenfolge und Einbezug der plattformspezifischen Modellwurzeln.
    /// Eingaben: Keine; verwendet einen temporären App-Modellpfad.
    /// Ausgabe: Keine Rückgabe; Assertions bestätigen App-Pfad zuerst und unter Windows `D:\Models`.
    #[test]
    fn default_roots_include_external_windows_model_library() {
        let temp = tempfile::tempdir().unwrap();
        let roots = ModelRegistry::default_roots(temp.path());
        assert_eq!(roots[0], temp.path());
        #[cfg(windows)]
        assert!(roots.iter().any(|root| root == Path::new(r"D:\Models")));
    }
}
