/*
===============================================================================
Division By Zeros (DBZS) - Rust-Core-Projektimport und -Persistenz
Modul: project
Zweck: Versionierte BANDMAP-Container validieren, sicher importieren und lokal
       projektbezogen wiederherstellen.
Herkunft: Neu in Rust formuliert; lokale BANDMAP-Dateien dienten nur als
          Formatreferenz und wurden weder kopiert noch als Quellcode übernommen.
===============================================================================
*/

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::OsStr;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use zip::read::ZipArchive;
use zip::result::ZipError;

const MAX_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
const PROJECT_METADATA_FILE: &str = "project.json";
const PROJECT_DATA_DIRECTORY: &str = "data";
const LAST_PROJECT_FILE: &str = "last-project";
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Fehler, die beim Prüfen, Importieren oder Wiederherstellen eines Projekts auftreten.
#[derive(Debug)]
pub enum ProjectError {
    /// Der Quelldateiname hat nicht die erwartete Endung `.bandmap`.
    InvalidExtension,
    /// Ein Dateisystemfehler ist beim Lesen oder Schreiben aufgetreten.
    Io(io::Error),
    /// Die ZIP-Struktur oder eine ZIP-Datei ist ungültig oder nicht lesbar.
    Zip(ZipError),
    /// Das lokale Projektverzeichnis enthält ungültige JSON-Metadaten.
    Json(serde_json::Error),
    /// Das Archiv besitzt kein unterstütztes BANDMAP-Manifest oder ist inkonsistent.
    InvalidManifest(String),
    /// Die Archivversion oder das Datenbankschema wird von diesem Importer nicht unterstützt.
    UnsupportedVersion { format: u32, schema: u32 },
    /// Ein Archivpfad ist unsicher, kollidiert auf Windows oder benennt einen Sonderdateityp.
    UnsafeEntry(String),
    /// Ein ZIP-Archiv überschreitet eine definierte Größen- oder Eintragsgrenze.
    ArchiveLimitExceeded,
    /// Eine für das erkannte Format erforderliche Datei fehlt.
    MissingEntry(&'static str),
    /// Die Projektdatei enthält keinen gültigen SQLite-3-Dateikopf.
    InvalidSqliteHeader,
    /// Eine angeforderte lokale Projektkennung ist ungültig oder fehlt.
    InvalidProjectId,
    /// Das Projektverzeichnis oder dessen persistente Daten sind nicht vorhanden.
    ProjectNotFound(String),
    /// Ein bereits vorhandenes lokales Projektverzeichnis ist inkonsistent.
    ProjectConflict(String),
}

impl Display for ProjectError {
    /// Zweck: Formuliert einen Projektfehler als kurze deutschsprachige Meldung.
    /// Eingaben: `self` – die konkrete Fehlerursache; `formatter` – das Ausgabeziel.
    /// Ausgabe: `Ok(())` nach erfolgreicher Textformatierung oder ein Formatierungsfehler.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExtension => {
                formatter.write_str("Die Eingabedatei muss die Endung .bandmap haben.")
            }
            Self::Io(error) => write!(formatter, "Dateizugriff fehlgeschlagen: {error}"),
            Self::Zip(error) => write!(
                formatter,
                "BANDMAP-ZIP ist ungültig oder nicht lesbar: {error}"
            ),
            Self::Json(error) => write!(formatter, "Projektmetadaten sind ungültig: {error}"),
            Self::InvalidManifest(message) => {
                write!(formatter, "BANDMAP-Manifest ist ungültig: {message}")
            }
            Self::UnsupportedVersion { format, schema } => write!(
                formatter,
                "BANDMAP-Formatversion {format} mit Schema {schema} wird nicht unterstützt."
            ),
            Self::UnsafeEntry(name) => write!(
                formatter,
                "Unsicherer oder kollidierender ZIP-Eintrag: {name}"
            ),
            Self::ArchiveLimitExceeded => formatter.write_str(
                "Das BANDMAP-Archiv überschreitet die erlaubten Größen- oder Eintragsgrenzen.",
            ),
            Self::MissingEntry(name) => {
                write!(formatter, "Erforderlicher BANDMAP-Eintrag fehlt: {name}")
            }
            Self::InvalidSqliteHeader => {
                formatter.write_str("project.sqlite besitzt keinen gültigen SQLite-3-Dateikopf.")
            }
            Self::InvalidProjectId => {
                formatter.write_str("Die lokale Projektkennung ist ungültig.")
            }
            Self::ProjectNotFound(id) => write!(
                formatter,
                "Lokales Projekt {id} oder dessen Projektdaten wurden nicht gefunden."
            ),
            Self::ProjectConflict(id) => write!(
                formatter,
                "Das lokale Projektverzeichnis {id} ist bereits vorhanden, aber inkonsistent."
            ),
        }
    }
}

impl Error for ProjectError {
    /// Zweck: Liefert die eingebettete Ursache für Fehler, die von einer anderen Bibliothek stammen.
    /// Eingaben: `self` – der konkrete Projektfehler.
    /// Ausgabe: Referenz auf die ursprüngliche I/O-, ZIP- oder JSON-Ursache, falls vorhanden.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Zip(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

/// Beschreibt ein lokal importiertes BANDMAP-Projekt und seine unveränderte Containerwahrheit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    /// Lokale, dateisystemtaugliche Kennung einschließlich Inhalts-Fingerabdruck.
    pub id: String,
    /// Projektkennung aus dem BANDMAP-Manifest.
    pub project_id: String,
    /// Lesbarer Projekttitel.
    pub title: String,
    /// Version des BANDMAP-Containerformats.
    pub format_version: u32,
    /// Version des kanonischen Projekt-Datenschemas; bei V1/V2 nicht vorhanden und daher null.
    pub schema_version: Option<u32>,
    /// Medientransportmodus des Containers, zum Beispiel `REFERENCE` oder `EMBEDDED`.
    pub media_mode: String,
    /// Optionaler Erstellungszeitpunkt aus dem V3-Manifest in Unix-Millisekunden.
    pub created_at_unix_ms: Option<u64>,
    /// Optionaler Änderungszeitpunkt aus dem V3-Manifest in Unix-Millisekunden.
    pub updated_at_unix_ms: Option<u64>,
    /// Zeitpunkt dieses lokalen Imports in Unix-Millisekunden.
    pub imported_at_unix_ms: u64,
    /// SHA-256 des ursprünglichen `.bandmap`-Containers zur Wiedererkennung identischer Importe.
    pub archive_sha256: String,
}

/// Dateibasierter Speicher für importierte BANDMAP-Projekte und den zuletzt geöffneten Eintrag.
#[derive(Debug, Clone)]
pub struct ProjectStore {
    root: PathBuf,
}

impl ProjectStore {
    /// Zweck: Erstellt einen Projektspeicher mit einem vom Aufrufer gewählten App-Datenstamm.
    /// Eingaben: `root` – beschreibbarer App-Datenordner; Projekte werden darunter in `projects/` abgelegt.
    /// Ausgabe: Neuer Speicherwert; das Dateisystem wird erst beim ersten Lese-/Schreibzugriff berührt.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Zweck: Importiert einen V1/V2-Recovery- oder V3-Portable-Container in einen lokalen Projektordner.
    /// Eingaben: `archive_path` – vorhandener `.bandmap`-ZIP-Container; das Archiv wird formatgeprüft und sicher entpackt.
    /// Ausgabe: Persistenter Projektdatensatz; identische Container werden anhand ihres SHA-256 wiederverwendet.
    pub fn import_bandmap(
        &self,
        archive_path: impl AsRef<Path>,
    ) -> Result<ProjectRecord, ProjectError> {
        let archive_path = archive_path.as_ref();
        if !has_bandmap_extension(archive_path) {
            return Err(ProjectError::InvalidExtension);
        }
        let archive_size = fs::metadata(archive_path).map_err(ProjectError::Io)?.len();
        if archive_size == 0 || archive_size > MAX_ARCHIVE_BYTES {
            return Err(ProjectError::ArchiveLimitExceeded);
        }
        let archive_sha256 = sha256_file(archive_path)?;
        let file = File::open(archive_path).map_err(ProjectError::Io)?;
        let mut archive = ZipArchive::new(file).map_err(ProjectError::Zip)?;
        let entries = inspect_archive_entries(&mut archive)?;
        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<HashSet<_>>();
        let metadata = read_archive_manifest(&mut archive, &names)?;
        if metadata.format_version == 3 {
            validate_sqlite_entry(&mut archive, &names)?;
        } else if !names.contains("project.properties") {
            return Err(ProjectError::MissingEntry("project.properties"));
        }

        let imported_at_unix_ms = current_unix_ms();
        let id = local_project_id(&metadata.project_id, &archive_sha256);
        let record = ProjectRecord {
            id: id.clone(),
            project_id: metadata.project_id,
            title: metadata.title,
            format_version: metadata.format_version,
            schema_version: metadata.schema_version,
            media_mode: metadata.media_mode,
            created_at_unix_ms: metadata.created_at_unix_ms,
            updated_at_unix_ms: metadata.updated_at_unix_ms,
            imported_at_unix_ms,
            archive_sha256,
        };
        let projects_root = self.projects_root();
        fs::create_dir_all(&projects_root).map_err(ProjectError::Io)?;
        let destination = projects_root.join(&record.id);
        if destination.exists() {
            let existing = self.read_project_record(&destination, &record.id)?;
            if existing.archive_sha256 != record.archive_sha256 {
                return Err(ProjectError::ProjectConflict(record.id));
            }
            self.save_last_project(&existing.id)?;
            return Ok(existing);
        }

        let staging = unique_staging_directory(&projects_root);
        fs::create_dir(&staging).map_err(ProjectError::Io)?;
        let mut staging_guard = StagingDirectory::new(staging.clone());
        extract_archive(
            &mut archive,
            &entries,
            &staging.join(PROJECT_DATA_DIRECTORY),
        )?;
        write_project_record(&staging, &record)?;
        match fs::rename(&staging, &destination) {
            Ok(()) => staging_guard.commit(),
            Err(error) if destination.exists() => {
                let existing = self.read_project_record(&destination, &record.id)?;
                if existing.archive_sha256 != record.archive_sha256 {
                    return Err(ProjectError::ProjectConflict(record.id));
                }
                let _ = error;
                self.save_last_project(&existing.id)?;
                return Ok(existing);
            }
            Err(error) => return Err(ProjectError::Io(error)),
        }
        self.save_last_project(&record.id)?;
        Ok(record)
    }

    /// Zweck: Liest persistente Metadaten eines lokalen Projekts und prüft, dass dessen Datenordner existiert.
    /// Eingaben: `id` – lokale Projektkennung, nicht die fremde Kennung aus dem BANDMAP-Manifest.
    /// Ausgabe: Projektdatensatz oder `ProjectNotFound`/`InvalidProjectId`.
    pub fn load_project(&self, id: &str) -> Result<ProjectRecord, ProjectError> {
        validate_local_project_id(id)?;
        self.read_project_record(&self.projects_root().join(id), id)
    }

    /// Zweck: Liefert importierte Projekte in absteigender Importzeit-Reihenfolge.
    /// Eingaben: `self` – Projektspeicher mit optional bereits vorhandenem `projects/`-Ordner.
    /// Ausgabe: Vollständige Liste lesbarer Projekte oder ein I/O-/JSON-Fehler.
    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>, ProjectError> {
        let projects_root = self.projects_root();
        if !projects_root.is_dir() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for entry in fs::read_dir(&projects_root).map_err(ProjectError::Io)? {
            let entry = entry.map_err(ProjectError::Io)?;
            if !entry.file_type().map_err(ProjectError::Io)?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.starts_with(".staging-") {
                continue;
            }
            validate_local_project_id(&id)?;
            records.push(self.read_project_record(&entry.path(), &id)?);
        }
        records.sort_by(|left, right| right.imported_at_unix_ms.cmp(&left.imported_at_unix_ms));
        Ok(records)
    }

    /// Zweck: Lädt das zuletzt importierte/geöffnete Projekt oder wählt bei einem veralteten Zeiger den neuesten Eintrag.
    /// Eingaben: `self` – Projektspeicher mit optionalem `last-project`-Zeiger.
    /// Ausgabe: Letzter gültiger Projektdatensatz oder `None`, wenn noch kein Projekt gespeichert wurde.
    pub fn load_last_project(&self) -> Result<Option<ProjectRecord>, ProjectError> {
        let pointer = self.root.join(LAST_PROJECT_FILE);
        if let Ok(id) = fs::read_to_string(&pointer) {
            let id = id.trim();
            if !id.is_empty() && validate_local_project_id(id).is_ok() {
                if let Ok(record) = self.load_project(id) {
                    return Ok(Some(record));
                }
            }
        }
        let fallback = self.list_projects()?.into_iter().next();
        if let Some(record) = &fallback {
            self.save_last_project(&record.id)?;
        }
        Ok(fallback)
    }

    /// Zweck: Liefert den lokalen Verzeichnis-Pfad eines geprüften Projekts für spätere App-Consumers.
    /// Eingaben: `id` – lokale Kennung eines gespeicherten Projekts.
    /// Ausgabe: Pfad zum Projektordner oder `InvalidProjectId` bei ungültiger Kennung.
    pub fn project_directory(&self, id: &str) -> Result<PathBuf, ProjectError> {
        validate_local_project_id(id)?;
        Ok(self.projects_root().join(id))
    }

    /// Zweck: Liefert den Ordner mit den original entpackten BANDMAP-Einträgen eines Projekts.
    /// Eingaben: `id` – lokale Kennung eines gespeicherten Projekts.
    /// Ausgabe: Pfad zum `data/`-Ordner; die Existenz wird durch `load_project` geprüft.
    pub fn project_data_directory(&self, id: &str) -> Result<PathBuf, ProjectError> {
        Ok(self.project_directory(id)?.join(PROJECT_DATA_DIRECTORY))
    }

    /// Zweck: Ermittelt den gemeinsamen Speicherordner für lokale BANDMAP-Projekte.
    /// Eingaben: `self` – Projektspeicher.
    /// Ausgabe: `root/projects` als absoluter oder relativ zum Aufrufer übergebener Pfad.
    fn projects_root(&self) -> PathBuf {
        self.root.join("projects")
    }

    /// Zweck: Liest lokale Projektdaten und verweigert Einträge ohne intakten `data/`-Ordner.
    /// Eingaben: Projektordner und erwartete lokale Kennung.
    /// Ausgabe: Validierter Datensatz oder ein Persistenzfehler.
    fn read_project_record(
        &self,
        project_directory: &Path,
        expected_id: &str,
    ) -> Result<ProjectRecord, ProjectError> {
        let metadata_path = project_directory.join(PROJECT_METADATA_FILE);
        let metadata_file = File::open(metadata_path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                ProjectError::ProjectNotFound(expected_id.to_owned())
            } else {
                ProjectError::Io(error)
            }
        })?;
        let record: ProjectRecord =
            serde_json::from_reader(BufReader::new(metadata_file)).map_err(ProjectError::Json)?;
        if record.id != expected_id || !project_directory.join(PROJECT_DATA_DIRECTORY).is_dir() {
            return Err(ProjectError::ProjectNotFound(expected_id.to_owned()));
        }
        Ok(record)
    }

    /// Zweck: Schreibt den zuletzt verwendeten Projektbezeichner als kleinen lokalen Wiederherstellungszeiger.
    /// Eingaben: `id` – geprüfte lokale Projektkennung.
    /// Ausgabe: `Ok(())` nach Speicherung oder ein Dateisystemfehler.
    fn save_last_project(&self, id: &str) -> Result<(), ProjectError> {
        validate_local_project_id(id)?;
        fs::create_dir_all(&self.root).map_err(ProjectError::Io)?;
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = self.root.join(format!(
            ".last-project-{}-{sequence}.tmp",
            std::process::id()
        ));
        let target = self.root.join(LAST_PROJECT_FILE);
        {
            let mut file = File::create(&temporary).map_err(ProjectError::Io)?;
            file.write_all(id.as_bytes()).map_err(ProjectError::Io)?;
            file.sync_all().map_err(ProjectError::Io)?;
        }
        if target.exists() {
            fs::remove_file(&target).map_err(ProjectError::Io)?;
        }
        fs::rename(&temporary, &target).map_err(ProjectError::Io)
    }
}

/// Einzelner, bereits sicher geprüfter Pfad und seine angekündigte ZIP-Größe.
#[derive(Debug, Clone)]
struct ArchiveEntry {
    index: usize,
    name: String,
    relative_path: PathBuf,
    size: u64,
    is_directory: bool,
}

/// Geprüfte, vom Manifest gelesene Container-Metadaten.
#[derive(Debug)]
struct ArchiveMetadata {
    project_id: String,
    title: String,
    format_version: u32,
    schema_version: Option<u32>,
    media_mode: String,
    created_at_unix_ms: Option<u64>,
    updated_at_unix_ms: Option<u64>,
}

/// Entfernt ein unvollständiges Staging-Verzeichnis, sofern es nicht atomar übernommen wurde.
#[derive(Debug)]
struct StagingDirectory {
    path: PathBuf,
    committed: bool,
}

impl StagingDirectory {
    /// Zweck: Registriert ein neu angelegtes Import-Staging-Verzeichnis für die Fehlerbereinigung.
    /// Eingaben: `path` – exklusiv vom aktuellen Import verwendeter Staging-Pfad.
    /// Ausgabe: Bereinigungsschutz, der bis zum erfolgreichen Umbenennen aktiv bleibt.
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    /// Zweck: Markiert das Staging-Verzeichnis nach erfolgreichem Commit als dauerhaftes Projekt.
    /// Eingaben: `self` – Bereinigungsschutz des abgeschlossenen Imports.
    /// Ausgabe: Keine Rückgabe; `Drop` lässt den übernommenen Ordner bestehen.
    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for StagingDirectory {
    /// Zweck: Entfernt bei fehlgeschlagenem Import alle teilweise entpackten Dateien.
    /// Eingaben: `self` – Staging-Status mit Zielpfad und Commit-Kennzeichen.
    /// Ausgabe: Keine Rückgabe; Fehler beim Aufräumen werden bewusst nicht weitergereicht.
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Zweck: Prüft, ob der übergebene Pfad die Dateiendung `.bandmap` trägt.
/// Eingaben: `path` – Eingabepfad, unabhängig von der Groß-/Kleinschreibung der Erweiterung.
/// Ausgabe: `true`, wenn die Erweiterung exakt `bandmap` lautet.
fn has_bandmap_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bandmap"))
}

/// Zweck: Berechnet den SHA-256-Fingerabdruck einer Archivdatei mit begrenztem Arbeitsspeicher.
/// Eingaben: `path` – lesbare Datei; Daten werden in einem festen Puffer gestreamt.
/// Ausgabe: 64-stellige hexadezimale Prüfsumme oder ein I/O-Fehler.
fn sha256_file(path: &Path) -> Result<String, ProjectError> {
    let file = File::open(path).map_err(ProjectError::Io)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(ProjectError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// Zweck: Berechnet den SHA-256-Hash einer lokalen Datei mit fest begrenztem Arbeitsspeicher.
/// Eingaben: Lesbarer lokaler Dateipfad, beispielsweise ein GGUF-Modell.
/// Ausgabe: 64-stellige kleingeschriebene Hex-Prüfsumme oder ein dateibezogener `ProjectError`.
pub fn sha256_local_file(path: impl AsRef<Path>) -> Result<String, ProjectError> {
    sha256_file(path.as_ref())
}

/// Zweck: Formatiert Bytes als kleingeschriebene Hexadezimalzeichenfolge.
/// Eingaben: `bytes` – beliebige Bytefolge, typischerweise das SHA-256-Ergebnis.
/// Ausgabe: Zwei ASCII-Hexziffern je Eingabebyte.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

/// Zweck: Ermittelt den aktuellen Unix-Zeitpunkt in Millisekunden ohne Panik bei Systemuhrfehlern.
/// Eingaben: Keine expliziten Argumente; liest die Systemuhr.
/// Ausgabe: Nichtnegative Unix-Millisekunden, bei nicht verfügbarer Uhr `0`.
fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// Zweck: Erzeugt eine speicherstabile lokale Projektkennung aus Fremd-ID und Archivfingerabdruck.
/// Eingaben: `project_id` – ungeprüfte Kennung aus dem Container; `sha256` – berechneter Archivfingerabdruck.
/// Ausgabe: Pfadkomponente aus normalisiertem Kennungsanteil und den ersten 16 Prüfsummenzeichen.
fn local_project_id(project_id: &str, sha256: &str) -> String {
    let mut slug = String::new();
    for character in project_id.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 48 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("bandmap");
    }
    format!("{slug}-{}", &sha256[..sha256.len().min(16)])
}

/// Zweck: Prüft eine externe lokale Projektkennung vor jeder Pfadbildung.
/// Eingaben: `id` – Kennung aus API-Aufruf oder lokaler Projektliste.
/// Ausgabe: `Ok(())` nur für begrenzte ASCII-Buchstaben, Ziffern, Bindestriche und Unterstriche.
fn validate_local_project_id(id: &str) -> Result<(), ProjectError> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ProjectError::InvalidProjectId);
    }
    Ok(())
}

/// Zweck: Reserviert einen eindeutigen Namen für einen atomar vorbereiteten Projektimport.
/// Eingaben: `projects_root` – existierender lokaler Projektordner.
/// Ausgabe: Nicht vorhandener Staging-Pfad mit Prozess- und Sequenzkennung.
fn unique_staging_directory(projects_root: &Path) -> PathBuf {
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    projects_root.join(format!(".staging-{}-{sequence}", std::process::id()))
}

/// Zweck: Prüft alle ZIP-Einträge auf Grenzen, Pfadtreue, Sonderdateien und Windows-Kollisionen.
/// Eingaben: `archive` – geöffnetes BANDMAP-ZIP; liest ausschließlich den zentralen Eintragskatalog.
/// Ausgabe: Geordnete Eintragsliste für Manifestprüfung und sicheres Entpacken.
fn inspect_archive_entries<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<Vec<ArchiveEntry>, ProjectError> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ProjectError::ArchiveLimitExceeded);
    }
    let mut entries = Vec::with_capacity(archive.len());
    let mut total_size = 0_u64;
    let mut seen = HashMap::<String, bool>::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(ProjectError::Zip)?;
        if entry.is_symlink() {
            return Err(ProjectError::UnsafeEntry(entry.name().to_owned()));
        }
        if let Some(mode) = entry.unix_mode() {
            let file_kind = mode & 0o170000;
            if file_kind != 0 && file_kind != 0o100000 && file_kind != 0o040000 {
                return Err(ProjectError::UnsafeEntry(entry.name().to_owned()));
            }
        }
        let name = entry.name().to_owned();
        let relative_path = safe_relative_path(&name, entry.is_dir())?;
        let path_key = relative_path
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
            .join("/");
        let is_directory = entry.is_dir();
        if seen.insert(path_key.clone(), is_directory).is_some() {
            return Err(ProjectError::UnsafeEntry(name));
        }
        for parent in relative_path.ancestors().skip(1) {
            if parent.as_os_str().is_empty() {
                continue;
            }
            let parent_key = parent
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
                .collect::<Vec<_>>()
                .join("/");
            if seen.get(&parent_key).is_some_and(|is_dir| !is_dir) {
                return Err(ProjectError::UnsafeEntry(name));
            }
        }
        if !is_directory
            && seen
                .keys()
                .any(|existing| existing.starts_with(&(path_key.clone() + "/")))
        {
            return Err(ProjectError::UnsafeEntry(name));
        }
        let size = entry.size();
        if size > MAX_ENTRY_BYTES {
            return Err(ProjectError::ArchiveLimitExceeded);
        }
        total_size = total_size
            .checked_add(size)
            .ok_or(ProjectError::ArchiveLimitExceeded)?;
        if total_size > MAX_UNPACKED_BYTES {
            return Err(ProjectError::ArchiveLimitExceeded);
        }
        entries.push(ArchiveEntry {
            index,
            name,
            relative_path,
            size,
            is_directory,
        });
    }
    Ok(entries)
}

/// Zweck: Wandelt einen ZIP-Eintragsnamen in einen erlaubten relativen Projektpfad um.
/// Eingaben: `name` – ZIP-Pfad; `is_directory` – kennzeichnet ein Ordnerende mit Schrägstrich.
/// Ausgabe: Relative `PathBuf` ohne übergeordnete, absolute oder Windows-Sonderpfadkomponenten.
fn safe_relative_path(name: &str, is_directory: bool) -> Result<PathBuf, ProjectError> {
    let invalid = || ProjectError::UnsafeEntry(name.to_owned());
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains(':')
        || name.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    let normalized = if is_directory {
        name.trim_end_matches('/')
    } else {
        name
    };
    if normalized.is_empty() {
        return Err(invalid());
    }
    let mut result = PathBuf::new();
    for component in normalized.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > 240
            || component.ends_with('.')
            || component.ends_with(' ')
            || component
                .chars()
                .any(|character| matches!(character, '<' | '>' | '"' | '|' | '?' | '*'))
            || is_reserved_windows_name(component)
        {
            return Err(invalid());
        }
        result.push(component);
    }
    if result
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid());
    }
    Ok(result)
}

/// Zweck: Erkennt reservierte Windows-Gerätenamen, die keine sicheren Projektdateinamen sind.
/// Eingaben: `component` – ein einzelnes bereits normalisiertes Pfadsegment.
/// Ausgabe: `true` für CON, PRN, AUX, NUL, COM1–9 oder LPT1–9 einschließlich Erweiterung.
fn is_reserved_windows_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
        || stem
            .strip_prefix("LPT")
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

/// Zweck: Liest und validiert das zum Archivformat passende BANDMAP-Manifest.
/// Eingaben: Geöffnetes ZIP und zuvor geprüfte Eintragsnamen.
/// Ausgabe: Formatversion, Kennung, Titel, Medientyp und optionale V3-Zeitwerte.
fn read_archive_manifest<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    names: &HashSet<&str>,
) -> Result<ArchiveMetadata, ProjectError> {
    if names.contains("manifest.json") {
        read_v3_manifest(archive)
    } else if names.contains("manifest.properties") {
        read_legacy_manifest(archive)
    } else {
        Err(ProjectError::MissingEntry(
            "manifest.json oder manifest.properties",
        ))
    }
}

/// Zweck: Liest das portable V3-JSON-Manifest und erzwingt die bekannte Format-/Schema-Version.
/// Eingaben: Geöffnetes ZIP mit `manifest.json`.
/// Ausgabe: Geprüfte Projektmetadaten oder ein verständlicher Manifest-/Versionsfehler.
fn read_v3_manifest<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<ArchiveMetadata, ProjectError> {
    let bytes = read_named_entry(archive, "manifest.json", MAX_MANIFEST_BYTES)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(ProjectError::Json)?;
    let object = value
        .as_object()
        .ok_or_else(|| ProjectError::InvalidManifest("JSON-Wurzel muss ein Objekt sein".into()))?;
    let format = object
        .get("format")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if !format.eq_ignore_ascii_case("BANDMAP") {
        return Err(ProjectError::InvalidManifest(
            "format muss BANDMAP lauten".into(),
        ));
    }
    let format_version = object
        .get("formatVersion")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            ProjectError::InvalidManifest(
                "formatVersion fehlt oder ist keine positive Ganzzahl".into(),
            )
        })?;
    let schema_version = object
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            ProjectError::InvalidManifest(
                "schemaVersion fehlt oder ist keine positive Ganzzahl".into(),
            )
        })?;
    if format_version != 3 || schema_version != 3 {
        return Err(ProjectError::UnsupportedVersion {
            format: format_version,
            schema: schema_version,
        });
    }
    let project_id = required_json_string(object, "projectId")?;
    let title = required_json_string(object, "title")?;
    let media_mode = object
        .get("mediaMode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("REFERENCE")
        .to_ascii_uppercase();
    validate_media_mode(&media_mode)?;
    Ok(ArchiveMetadata {
        project_id,
        title,
        format_version,
        schema_version: Some(schema_version),
        media_mode,
        created_at_unix_ms: object
            .get("createdAtMs")
            .and_then(serde_json::Value::as_u64),
        updated_at_unix_ms: object
            .get("updatedAtMs")
            .and_then(serde_json::Value::as_u64),
    })
}

/// Zweck: Liest eine verpflichtende, nicht leere Zeichenketteneigenschaft aus einem V3-Manifest.
/// Eingaben: Manifestobjekt und erwarteter Feldname.
/// Ausgabe: Bereinigte Zeichenkette oder `InvalidManifest` bei fehlendem/leerem Wert.
fn required_json_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, ProjectError> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ProjectError::InvalidManifest(format!("{key} fehlt oder ist leer")))
}

/// Zweck: Liest die Java-Properties-Manifeste der älteren BANDMAP-V1/V2-Container.
/// Eingaben: Geöffnetes ZIP mit `manifest.properties` und `project.properties`.
/// Ausgabe: Normalisierte Metadaten; ältere Projekte werden als referenzierte Medien behandelt.
fn read_legacy_manifest<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<ArchiveMetadata, ProjectError> {
    let manifest = parse_java_properties(&read_named_entry(
        archive,
        "manifest.properties",
        MAX_MANIFEST_BYTES,
    )?)?;
    let project = parse_java_properties(&read_named_entry(
        archive,
        "project.properties",
        MAX_MANIFEST_BYTES,
    )?)?;
    let format = manifest
        .get("format")
        .map(String::as_str)
        .unwrap_or_default();
    if !format.eq_ignore_ascii_case("BANDMAP") {
        return Err(ProjectError::InvalidManifest(
            "format muss BANDMAP lauten".into(),
        ));
    }
    let format_version = manifest
        .get("formatVersion")
        .map(String::as_str)
        .unwrap_or("1")
        .parse::<u32>()
        .map_err(|_| ProjectError::InvalidManifest("formatVersion ist keine Ganzzahl".into()))?;
    if !matches!(format_version, 1 | 2) {
        return Err(ProjectError::UnsupportedVersion {
            format: format_version,
            schema: 0,
        });
    }
    let project_id = manifest
        .get("projectId")
        .or_else(|| project.get("id"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProjectError::InvalidManifest("projectId/id fehlt".into()))?
        .to_owned();
    let title = manifest
        .get("title")
        .or_else(|| project.get("title"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProjectError::InvalidManifest("title fehlt".into()))?
        .to_owned();
    let media_mode = manifest
        .get("mediaMode")
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_else(|| "REFERENCE".into());
    validate_media_mode(&media_mode)?;
    Ok(ArchiveMetadata {
        project_id,
        title,
        format_version,
        schema_version: None,
        media_mode,
        created_at_unix_ms: None,
        updated_at_unix_ms: None,
    })
}

/// Zweck: Beschränkt den Medienmodus auf die bekannten, portablen BANDMAP-Werte.
/// Eingaben: `media_mode` – normalisierte Containerangabe.
/// Ausgabe: `Ok(())` für REFERENCE oder EMBEDDED; sonst ein Manifestfehler.
fn validate_media_mode(media_mode: &str) -> Result<(), ProjectError> {
    if matches!(media_mode, "REFERENCE" | "EMBEDDED") {
        Ok(())
    } else {
        Err(ProjectError::InvalidManifest(format!(
            "unbekannter mediaMode {media_mode}"
        )))
    }
}

/// Zweck: Liest einen begrenzten ZIP-Eintrag vollständig in den Arbeitsspeicher.
/// Eingaben: Geöffnetes ZIP, exakter Eintragsname und maximale Manifestgröße.
/// Ausgabe: Eintragsbytes; Einträge oberhalb der Grenze werden abgewiesen.
fn read_named_entry<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    maximum_bytes: usize,
) -> Result<Vec<u8>, ProjectError> {
    let mut entry = archive.by_name(name).map_err(|error| match error {
        ZipError::FileNotFound => ProjectError::MissingEntry(match name {
            "manifest.json" => "manifest.json",
            "manifest.properties" => "manifest.properties",
            "project.properties" => "project.properties",
            _ => "ZIP-Eintrag",
        }),
        other => ProjectError::Zip(other),
    })?;
    let size = usize::try_from(entry.size()).map_err(|_| ProjectError::ArchiveLimitExceeded)?;
    if size > maximum_bytes {
        return Err(ProjectError::ArchiveLimitExceeded);
    }
    let mut bytes = Vec::with_capacity(size);
    entry.read_to_end(&mut bytes).map_err(ProjectError::Io)?;
    if bytes.len() != size {
        return Err(ProjectError::InvalidManifest(format!(
            "ZIP-Eintrag {name} hat eine falsche Länge"
        )));
    }
    Ok(bytes)
}

/// Zweck: Prüft, ob eine V3-Projektdatei im ZIP vorhanden ist und mit dem SQLite-3-Kopf beginnt.
/// Eingaben: Geöffnetes ZIP und geprüfte Eintragsnamen.
/// Ausgabe: `Ok(())` für eine plausible `project.sqlite`, sonst ein Formatfehler.
fn validate_sqlite_entry<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    names: &HashSet<&str>,
) -> Result<(), ProjectError> {
    if !names.contains("project.sqlite") {
        return Err(ProjectError::MissingEntry("project.sqlite"));
    }
    let mut entry = archive
        .by_name("project.sqlite")
        .map_err(ProjectError::Zip)?;
    if entry.size() < 100 {
        return Err(ProjectError::InvalidSqliteHeader);
    }
    let mut header = [0_u8; 16];
    entry.read_exact(&mut header).map_err(ProjectError::Io)?;
    if &header != SQLITE_HEADER {
        return Err(ProjectError::InvalidSqliteHeader);
    }
    Ok(())
}

/// Zweck: Entschlüsselt Java-Properties-Schlüssel und -Werte einschließlich Zeilenfortsetzung und Unicode-Escapes.
/// Eingaben: Bytes aus `Properties.store`; UTF-8 wird bevorzugt, sonst wird das Java-ISO-8859-1-Format gelesen.
/// Ausgabe: Schlüssel-Wert-Tabelle oder ein Manifestfehler bei ungültiger Escape-Sequenz.
fn parse_java_properties(bytes: &[u8]) -> Result<HashMap<String, String>, ProjectError> {
    let source = match String::from_utf8(bytes.to_vec()) {
        Ok(text) => text,
        Err(_) => bytes.iter().copied().map(char::from).collect(),
    };
    let mut logical_lines = Vec::new();
    let mut pending = String::new();
    for line in source.lines() {
        let trimmed = if pending.is_empty() {
            line.trim_start()
        } else {
            line.trim_start()
        };
        pending.push_str(trimmed);
        let trailing_slashes = pending
            .chars()
            .rev()
            .take_while(|character| *character == '\\')
            .count();
        if trailing_slashes % 2 == 1 {
            pending.pop();
        } else {
            logical_lines.push(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        logical_lines.push(pending);
    }
    let mut properties = HashMap::new();
    for line in logical_lines {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let characters = line.chars().collect::<Vec<_>>();
        let mut escaped = false;
        let mut separator = characters.len();
        for (index, character) in characters.iter().enumerate() {
            if !escaped && (*character == '=' || *character == ':' || character.is_whitespace()) {
                separator = index;
                break;
            }
            if *character == '\\' {
                escaped = !escaped;
            } else {
                escaped = false;
            }
        }
        let key_end = separator;
        let mut value_start = separator;
        while value_start < characters.len() && characters[value_start].is_whitespace() {
            value_start += 1;
        }
        if value_start < characters.len() && matches!(characters[value_start], '=' | ':') {
            value_start += 1;
        }
        while value_start < characters.len() && characters[value_start].is_whitespace() {
            value_start += 1;
        }
        let key = unescape_java_property(&characters[..key_end])?;
        let value = unescape_java_property(&characters[value_start..])?;
        properties.insert(key, value);
    }
    Ok(properties)
}

/// Zweck: Löst Backslash-, Steuerzeichen- und Unicode-Escapes eines einzelnen Java-Properties-Feldes auf.
/// Eingaben: Zeichen eines Schlüssel- oder Werteabschnitts ohne Trennzeichen.
/// Ausgabe: Unicode-String oder `InvalidManifest` bei unvollständigen/ungültigen Escape-Sequenzen.
fn unescape_java_property(characters: &[char]) -> Result<String, ProjectError> {
    let mut units = Vec::<u16>::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        index += 1;
        if character != '\\' {
            let mut encoded = [0_u16; 2];
            units.extend_from_slice(character.encode_utf16(&mut encoded));
            continue;
        }
        let escaped = *characters.get(index).ok_or_else(|| {
            ProjectError::InvalidManifest("unvollständiger Backslash-Escape".into())
        })?;
        index += 1;
        match escaped {
            't' => units.push('\t' as u16),
            'n' => units.push('\n' as u16),
            'r' => units.push('\r' as u16),
            'f' => units.push('\u{000c}' as u16),
            'u' => {
                let end = index.checked_add(4).ok_or_else(|| {
                    ProjectError::InvalidManifest("Unicode-Escape ist zu lang".into())
                })?;
                let digits = characters.get(index..end).ok_or_else(|| {
                    ProjectError::InvalidManifest("Unicode-Escape benötigt vier Hexziffern".into())
                })?;
                let mut unit = 0_u16;
                for digit in digits {
                    unit = unit
                        .checked_mul(16)
                        .and_then(|value| {
                            digit
                                .to_digit(16)
                                .and_then(|digit| value.checked_add(digit as u16))
                        })
                        .ok_or_else(|| {
                            ProjectError::InvalidManifest(
                                "Unicode-Escape enthält eine ungültige Hexziffer".into(),
                            )
                        })?;
                }
                units.push(unit);
                index = end;
            }
            other => {
                let mut encoded = [0_u16; 2];
                units.extend_from_slice(other.encode_utf16(&mut encoded));
            }
        }
    }
    String::from_utf16(&units).map_err(|_| {
        ProjectError::InvalidManifest("Unicode-Escape enthält ein ungültiges Surrogatpaar".into())
    })
}

/// Zweck: Entpackt alle vorab geprüften ZIP-Einträge ausschließlich in ein neu angelegtes Staging-Verzeichnis.
/// Eingaben: Geöffnetes Archiv, geprüfte Eintragsliste und Zielordner für Projektdaten.
/// Ausgabe: `Ok(())` nach vollständigem Entpacken; falsche Längen, CRC-/I/O-Fehler oder Pfadkollisionen brechen ab.
fn extract_archive<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    entries: &[ArchiveEntry],
    data_directory: &Path,
) -> Result<(), ProjectError> {
    fs::create_dir_all(data_directory).map_err(ProjectError::Io)?;
    let mut copied_total = 0_u64;
    for spec in entries {
        let target = data_directory.join(&spec.relative_path);
        if spec.is_directory {
            fs::create_dir_all(&target).map_err(ProjectError::Io)?;
            continue;
        }
        let parent = target
            .parent()
            .ok_or_else(|| ProjectError::UnsafeEntry(spec.name.clone()))?;
        fs::create_dir_all(parent).map_err(ProjectError::Io)?;
        let mut entry = archive.by_index(spec.index).map_err(ProjectError::Zip)?;
        let mut output = File::create(&target).map_err(ProjectError::Io)?;
        let copied = io::copy(&mut entry, &mut output).map_err(ProjectError::Io)?;
        if copied != spec.size {
            return Err(ProjectError::InvalidManifest(format!(
                "ZIP-Eintrag {} hat eine falsche Länge",
                spec.name
            )));
        }
        copied_total = copied_total
            .checked_add(copied)
            .ok_or(ProjectError::ArchiveLimitExceeded)?;
        if copied_total > MAX_UNPACKED_BYTES {
            return Err(ProjectError::ArchiveLimitExceeded);
        }
        output.sync_all().map_err(ProjectError::Io)?;
    }
    Ok(())
}

/// Zweck: Persistiert lokale Projektmetadaten als formatiertes UTF-8-JSON innerhalb des Staging-Ordners.
/// Eingaben: `directory` – Projekt-Staging-Ordner; `record` – vollständig geprüfte Importmetadaten.
/// Ausgabe: `Ok(())` nach Schreiben und Flush oder ein Datei-/Serialisierungsfehler.
fn write_project_record(directory: &Path, record: &ProjectRecord) -> Result<(), ProjectError> {
    let path = directory.join(PROJECT_METADATA_FILE);
    let mut file = File::create(path).map_err(ProjectError::Io)?;
    serde_json::to_writer_pretty(&mut file, record).map_err(ProjectError::Json)?;
    file.write_all(b"\n").map_err(ProjectError::Io)?;
    file.sync_all().map_err(ProjectError::Io)
}

#[cfg(test)]
mod tests {
    use super::{ProjectError, ProjectStore};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    /// Zweck: Erstellt ein minimales, formatplausibles V3-Archiv für Importtests.
    /// Eingaben: Zielpfad und Projekttitel.
    /// Ausgabe: Geschriebenes `.bandmap`-Archiv mit Manifest, SQLite-Kopf und einer Assetdatei.
    fn write_v3_archive(path: &Path, title: &str) -> PathBuf {
        let file = fs::File::create(path).expect("ZIP-Zieldatei muss angelegt werden");
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        let manifest = format!(
            "{{\"format\":\"BANDMAP\",\"formatVersion\":3,\"schemaVersion\":3,\"projectId\":\"v3-project\",\"title\":{},\"mediaMode\":\"REFERENCE\",\"createdAtMs\":42,\"updatedAtMs\":43}}",
            serde_json::to_string(title).expect("JSON-Titel muss kodierbar sein")
        );
        zip.start_file("manifest.json", options)
            .expect("Manifesteintrag muss startbar sein");
        zip.write_all(manifest.as_bytes())
            .expect("Manifest muss geschrieben werden");
        zip.start_file("project.sqlite", options)
            .expect("SQLite-Eintrag muss startbar sein");
        let mut database = [0_u8; 100];
        database[..16].copy_from_slice(b"SQLite format 3\0");
        zip.write_all(&database)
            .expect("SQLite-Testkopf muss geschrieben werden");
        zip.start_file("audio/analysis/normalized.wav", options)
            .expect("Asseteintrag muss startbar sein");
        zip.write_all(b"test audio asset")
            .expect("Asset muss geschrieben werden");
        zip.finish().expect("ZIP-Archiv muss abgeschlossen werden");
        path.to_path_buf()
    }

    /// Zweck: Erstellt ein lokales V2-Recovery-Archiv mit einfachen Properties-Einträgen.
    /// Eingaben: Zielpfad des Testcontainers.
    /// Ausgabe: Geschriebenes `.bandmap`-Archiv mit Manifest, Projektmetadaten und Analyse-TSV.
    fn write_v2_archive(path: &Path) -> PathBuf {
        let file = fs::File::create(path).expect("ZIP-Zieldatei muss angelegt werden");
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("manifest.properties", options)
            .expect("Manifesteintrag muss startbar sein");
        zip.write_all(b"format=BANDMAP\nformatVersion=2\nprojectId=legacy-42\ntitle=Legacy Test\nmediaMode=REFERENCE\n")
            .expect("Properties-Manifest muss geschrieben werden");
        zip.start_file("project.properties", options)
            .expect("Projektmetadaten müssen startbar sein");
        zip.write_all(b"id=legacy-42\ntitle=Legacy Test\nsourcePath=C:/music/source.mp3\n")
            .expect("Projektmetadaten müssen geschrieben werden");
        zip.start_file("analysis/quickmap.properties", options)
            .expect("Analyseeintrag muss startbar sein");
        zip.write_all(b"bpm=110\nkey=C major\n")
            .expect("Analyse muss geschrieben werden");
        zip.finish().expect("ZIP-Archiv muss abgeschlossen werden");
        path.to_path_buf()
    }

    /// Zweck: Prüft V3-Import, sichere Persistenz der Assets und Wiederherstellung des letzten Projekts.
    /// Eingaben: Keine; erzeugt ein temporäres V3-Archiv und einen temporären App-Datenstamm.
    /// Ausgabe: Test besteht, wenn Metadaten, SQLite-Datei, Asset, Index und Last-Project-Zeiger erhalten sind.
    #[test]
    fn imports_v3_archive_and_restores_last_project() {
        let root = tempdir().expect("Temporärer Projektspeicher muss erstellt werden");
        let archive_path = write_v3_archive(&root.path().join("song.bandmap"), "Über den Wolken");
        let store = ProjectStore::new(root.path().join("app-data"));
        let imported = store
            .import_bandmap(&archive_path)
            .expect("V3-Projekt muss importiert werden");
        assert_eq!(imported.format_version, 3);
        assert_eq!(imported.schema_version, Some(3));
        assert_eq!(imported.title, "Über den Wolken");
        assert_eq!(
            store
                .load_project(&imported.id)
                .expect("Index muss ladbar sein"),
            imported
        );
        assert_eq!(
            store
                .load_last_project()
                .expect("letztes Projekt muss ladbar sein")
                .unwrap()
                .id,
            imported.id
        );
        let data = store
            .project_data_directory(&imported.id)
            .expect("Datenpfad muss berechenbar sein");
        assert!(data.join("project.sqlite").is_file());
        assert_eq!(
            fs::read(data.join("audio/analysis/normalized.wav")).expect("Asset muss entpackt sein"),
            b"test audio asset"
        );
        assert_eq!(
            store
                .list_projects()
                .expect("Projektindex muss auflistbar sein")
                .len(),
            1
        );
    }

    /// Zweck: Prüft den V1/V2-Kompatibilitätspfad für ältere Recovery-Container.
    /// Eingaben: Keine; erzeugt ein V2-Archiv mit Properties und Analyse-TSV.
    /// Ausgabe: Test besteht, wenn die V2-Metadaten und extrahierten Analyseinhalte erhalten sind.
    #[test]
    fn imports_legacy_v2_recovery_archive() {
        let root = tempdir().expect("Temporärer Projektspeicher muss erstellt werden");
        let archive_path = write_v2_archive(&root.path().join("legacy.bandmap"));
        let store = ProjectStore::new(root.path().join("app-data"));
        let imported = store
            .import_bandmap(&archive_path)
            .expect("V2-Projekt muss importiert werden");
        assert_eq!(imported.format_version, 2);
        assert_eq!(imported.schema_version, None);
        assert_eq!(imported.project_id, "legacy-42");
        assert!(
            store
                .project_data_directory(&imported.id)
                .unwrap()
                .join("analysis/quickmap.properties")
                .is_file()
        );
    }

    /// Zweck: Verhindert ZIP-Pfadtraversal und hinterlässt bei Ablehnung keine Projekt- oder Fremddatei.
    /// Eingaben: Keine; erstellt ein V3-Archiv mit dem bösartigen Pfad `../escape.txt`.
    /// Ausgabe: Test besteht, wenn Import mit `UnsafeEntry` fehlschlägt und kein Escape-Artefakt entsteht.
    #[test]
    fn rejects_path_traversal_without_leaving_staging_data() {
        let root = tempdir().expect("Temporärer Projektspeicher muss erstellt werden");
        let path = root.path().join("traversal.bandmap");
        let file = fs::File::create(&path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(b"{\"format\":\"BANDMAP\",\"formatVersion\":3,\"schemaVersion\":3,\"projectId\":\"unsafe\",\"title\":\"Unsafe\",\"mediaMode\":\"REFERENCE\"}").unwrap();
        zip.start_file("project.sqlite", options).unwrap();
        let mut database = [0_u8; 100];
        database[..16].copy_from_slice(b"SQLite format 3\0");
        zip.write_all(&database).unwrap();
        zip.start_file("../escape.txt", options).unwrap();
        zip.write_all(b"outside").unwrap();
        zip.finish().unwrap();
        let store = ProjectStore::new(root.path().join("app-data"));
        assert!(matches!(
            store.import_bandmap(&path),
            Err(ProjectError::UnsafeEntry(_))
        ));
        assert!(!root.path().join("escape.txt").exists());
        assert!(store.list_projects().unwrap().is_empty());
    }

    /// Zweck: Prüft, dass derselbe Container wiederholt dieselbe lokale Kennung und keinen Duplikatordner erzeugt.
    /// Eingaben: Keine; importiert denselben V3-Container zweimal in denselben Speicher.
    /// Ausgabe: Test besteht, wenn der zweite Import denselben Projektdatensatz zurückliefert.
    #[test]
    fn repeated_import_reuses_identical_project() {
        let root = tempdir().expect("Temporärer Projektspeicher muss erstellt werden");
        let archive_path = write_v3_archive(&root.path().join("repeat.bandmap"), "Repeat");
        let store = ProjectStore::new(root.path().join("app-data"));
        let first = store
            .import_bandmap(&archive_path)
            .expect("Erstimport muss gelingen");
        let second = store
            .import_bandmap(&archive_path)
            .expect("Wiederholungsimport muss gelingen");
        assert_eq!(first.id, second.id);
        assert_eq!(store.list_projects().unwrap().len(), 1);
    }

    /// Zweck: Verhindert, dass eine fremde Dateiendung als BANDMAP-Container akzeptiert wird.
    /// Eingaben: Keine; benennt eine vorhandene Datei mit `.zip` statt `.bandmap`.
    /// Ausgabe: Test besteht, wenn der Import vor dem ZIP-Lesen mit `InvalidExtension` endet.
    #[test]
    fn rejects_non_bandmap_extension() {
        let root = tempdir().expect("Temporärer Projektspeicher muss erstellt werden");
        let path = root.path().join("project.zip");
        fs::write(&path, b"not a zip").unwrap();
        let store = ProjectStore::new(root.path().join("app-data"));
        assert!(matches!(
            store.import_bandmap(&path),
            Err(ProjectError::InvalidExtension)
        ));
    }
}
