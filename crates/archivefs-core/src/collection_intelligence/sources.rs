//! Gathers the facts reconciliation needs and assembles the report.

use super::inventory::{
    DEFAULT_MAX_ENTRIES, EntryKind, InventoryError, InventoryRequest, take_inventory,
};
use super::links::audit_links;
use super::model::*;
use super::reconcile::{
    ContentVerdict, ContentVerifier, CurrentRow, JournalMove, OldRecord, ReconcileInputs,
    RecordedHashes, ScanRun, classify_records,
};
use crate::dat::archive::limits::ArchiveLimits;
use crate::dat::archive::zip::ZipArchiveSource;
use crate::dat::archive::{ArchiveMemberSource, ArchiveRunBudget};
use crate::dat::rename_apply::{
    LEGACY_RENAME_TRANSACTIONS_DIRECTORY, RENAME_TRANSACTIONS_DIRECTORY,
};
use crate::database::{CollectionRecordRow, CollectionSourceFolder, Database};
use crate::identity_source::hashing::hash_file;
use crate::safe_read::TrustedRoots;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::BufReader;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

/// A file larger than this is not read for a content comparison.
const MAX_CONTENT_CHECK_BYTES: u64 = 64 * 1024 * 1024;
/// A Forget Missing export larger than this is not read.
const MAX_EXPORT_BYTES: u64 = 256 * 1024 * 1024;
/// A recorded-hash cache larger than this is not read.
const MAX_HASH_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// A rename journal larger than this is not read.
const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
const ROMM_CACHE_RELATIVE: &str = "identity/romm/identity-cache.json";
const DIRECTORY_KINDS: [&str; 2] = ["arcade_set_directory", "scummvm_directory"];

#[derive(Debug, Clone)]
pub struct ReconcileRequest {
    pub database_path: PathBuf,
    /// Forget Missing exports to include besides the ones found next to the database.
    pub forget_receipts: Vec<PathBuf>,
    /// Read files that have an independently recorded hash for their old path.
    pub verify_content: bool,
    pub max_entries: u64,
}

impl ReconcileRequest {
    pub fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            forget_receipts: Vec::new(),
            verify_content: false,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub phase: &'static str,
    pub done: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileError {
    Database(String),
    Cancelled,
    NoLibraryFolders,
    /// A folder could not be listed completely. Reporting on a partial
    /// listing would call files missing that were simply not looked at.
    IncompleteListing(Vec<String>),
}

impl std::fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(detail) => write!(f, "the library database could not be read: {detail}"),
            Self::Cancelled => write!(f, "cancelled; nothing was changed"),
            Self::NoLibraryFolders => write!(f, "no library folder can be listed"),
            Self::IncompleteListing(details) => write!(
                f,
                "a library folder could not be listed completely, so nothing is reported: {}",
                details.join("; ")
            ),
        }
    }
}

impl std::error::Error for ReconcileError {}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One row of a Forget Missing export.
struct ForgottenRow {
    id: i64,
    path: PathBuf,
    kind: String,
    size: Option<u64>,
    mtime: Option<i64>,
    first_seen: Option<String>,
    last_seen: Option<String>,
    flagged: Option<String>,
    platform: Option<String>,
    source_folder_id: i64,
}

fn blob(value: &serde_json::Value) -> Option<Vec<u8>> {
    match value {
        serde_json::Value::String(text) => Some(text.as_bytes().to_vec()),
        serde_json::Value::Object(map) => {
            let hex = map.get("blob")?.as_str()?;
            if hex.len() % 2 != 0 {
                return None;
            }
            (0..hex.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
                .collect()
        }
        _ => None,
    }
}

/// Reads a Forget Missing export without trusting it: anything that does not
/// parse is skipped and reported, never guessed.
fn read_receipt(
    path: &Path,
    database: &Path,
) -> Result<(ReceiptSummary, Vec<ForgottenRow>), String> {
    let regular = fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_EXPORT_BYTES);
    if !regular {
        return Err("not a regular file, or too large to be an export".into());
    }
    let bytes = fs::read(path).map_err(|error| error.kind().to_string())?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("not valid JSON ({error})"))?;
    let tables = value
        .get("tables")
        .and_then(|tables| tables.as_array())
        .ok_or("no tables")?;
    let table = |name: &str| {
        tables
            .iter()
            .find(|table| table.get("table").and_then(|t| t.as_str()) == Some(name))
    };
    let columns = |table: &serde_json::Value| -> HashMap<String, usize> {
        table
            .get("columns")
            .and_then(|c| c.as_array())
            .map(|columns| {
                columns
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| Some((c.as_str()?.to_string(), i)))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut platforms: HashMap<i64, String> = HashMap::new();
    if let Some(assignments) = table("platform_assignments") {
        let at = columns(assignments);
        if let (Some(id), Some(platform), Some(current)) = (
            at.get("archive_id"),
            at.get("platform"),
            at.get("is_current"),
        ) {
            for row in assignments
                .get("rows")
                .and_then(|r| r.as_array())
                .into_iter()
                .flatten()
            {
                let (Some(id), Some(platform)) = (
                    row.get(*id).and_then(|v| v.as_i64()),
                    row.get(*platform).and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                let is_current = row.get(*current).and_then(|v| v.as_i64()) == Some(1);
                if is_current || !platforms.contains_key(&id) {
                    platforms.insert(id, platform.to_string());
                }
            }
        }
    }
    let archives = table("archives").ok_or("no library rows in the export")?;
    let at = columns(archives);
    let need = |name: &str| at.get(name).copied().ok_or(format!("no {name} column"));
    let (id, source, path_at, kind) = (
        need("id")?,
        need("source_folder_id")?,
        need("absolute_path_cached")?,
        need("archive_kind")?,
    );
    let text = |row: &serde_json::Value, name: &str| {
        at.get(name)
            .and_then(|i| row.get(*i))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let number = |row: &serde_json::Value, name: &str| {
        at.get(name)
            .and_then(|i| row.get(*i))
            .and_then(|v| v.as_i64())
    };
    let mut rows = Vec::new();
    for row in archives
        .get("rows")
        .and_then(|r| r.as_array())
        .into_iter()
        .flatten()
    {
        let (Some(id), Some(path)) = (
            row.get(id).and_then(|v| v.as_i64()),
            row.get(path_at).and_then(blob),
        ) else {
            continue;
        };
        rows.push(ForgottenRow {
            id,
            path: PathBuf::from(OsString::from_vec(path)),
            kind: row
                .get(kind)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            size: number(row, "size_bytes").map(|v| v.max(0) as u64),
            mtime: number(row, "modified_time_unix_seconds"),
            first_seen: text(row, "first_seen_at"),
            last_seen: text(row, "last_seen_at"),
            flagged: text(row, "last_verified_missing_at"),
            platform: platforms.get(&id).cloned(),
            source_folder_id: row.get(source).and_then(|v| v.as_i64()).unwrap_or(-1),
        });
    }
    let recorded_database = value.get("database_path").and_then(|v| v.as_str());
    Ok((
        ReceiptSummary {
            path: path.to_path_buf(),
            created_at: value
                .get("created_at")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            state: value
                .get("state")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            records: rows.len() as u64,
            written_for_this_database: recorded_database.map(Path::new) == Some(database),
        },
        rows,
    ))
}

/// Exports beside the database that were written for this database.
fn receipts_beside(database: &Path) -> Vec<PathBuf> {
    let (Some(parent), Some(name)) = (database.parent(), database.file_name()) else {
        return Vec::new();
    };
    let prefix = format!("{}.forget-missing-", name.to_string_lossy());
    let mut found: Vec<PathBuf> = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json"))
                && fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
        })
        .collect();
    found.sort();
    found
}

#[derive(Deserialize)]
struct LiteHash {
    algorithm: String,
    value: String,
}

#[derive(Deserialize)]
struct LiteRecord {
    #[serde(default)]
    archivefs_path: Option<PathBuf>,
    #[serde(default)]
    hashes: Vec<LiteHash>,
}

#[derive(Deserialize)]
struct LiteCache {
    #[serde(default)]
    records: Vec<LiteRecord>,
}

/// Hashes RomM recorded for paths as they were when it was last imported.
/// Only the path and hashes are kept, so the large cache is never held whole.
fn romm_hashes(database: &Path, wanted: &HashSet<&Path>) -> HashMap<PathBuf, RecordedHashes> {
    let Some(path) = database.parent().map(|dir| dir.join(ROMM_CACHE_RELATIVE)) else {
        return HashMap::new();
    };
    let regular = fs::symlink_metadata(&path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_HASH_CACHE_BYTES);
    if !regular {
        return HashMap::new();
    }
    let Ok(file) = fs::File::open(&path) else {
        return HashMap::new();
    };
    let Ok(cache) = serde_json::from_reader::<_, LiteCache>(BufReader::new(file)) else {
        return HashMap::new();
    };
    cache
        .records
        .into_iter()
        .filter_map(|record| {
            let path = record.archivefs_path?;
            if !wanted.contains(path.as_path()) {
                return None;
            }
            let pick = |name: &str| {
                record
                    .hashes
                    .iter()
                    .find(|hash| hash.algorithm == name)
                    .map(|hash| hash.value.to_ascii_lowercase())
            };
            let hashes = RecordedHashes {
                source: "RomM".into(),
                crc32: pick("crc32"),
                md5: pick("md5"),
                sha1: pick("sha1"),
            };
            (hashes.crc32.is_some() || hashes.md5.is_some() || hashes.sha1.is_some())
                .then_some((path, hashes))
        })
        .collect()
}

struct Verifier<'a> {
    cancel: &'a AtomicBool,
    summary: ContentCheckSummary,
}

fn agree(recorded: &RecordedHashes, crc32: &str, md5: &str, sha1: &str) -> Option<bool> {
    let pairs = [
        (recorded.sha1.as_deref(), sha1),
        (recorded.md5.as_deref(), md5),
        (recorded.crc32.as_deref(), crc32),
    ];
    let compared: Vec<bool> = pairs
        .iter()
        .filter_map(|(recorded, fresh)| recorded.map(|r| r.eq_ignore_ascii_case(fresh)))
        .collect();
    (!compared.is_empty()).then(|| compared.iter().all(|same| *same))
}

impl ContentVerifier for Verifier<'_> {
    fn compare(&mut self, target: &Path, recorded: &RecordedHashes) -> ContentVerdict {
        self.summary.compared += 1;
        let size = fs::symlink_metadata(target)
            .map(|m| m.len())
            .unwrap_or(u64::MAX);
        if size > MAX_CONTENT_CHECK_BYTES {
            self.summary.not_comparable += 1;
            return ContentVerdict::NotComparable;
        }
        let trusted = TrustedRoots::none();
        let mut verdict = None;
        if let Ok(hashes) = hash_file(target, &trusted, Some(self.cancel)) {
            self.summary.bytes_read += hashes.bytes_hashed;
            verdict = agree(recorded, &hashes.crc32, &hashes.md5, &hashes.sha1);
        }
        // RomM hashes what is inside a single-file ZIP, not the ZIP itself.
        let is_zip = target
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
        if verdict != Some(true)
            && is_zip
            && let Ok(mut archive) =
                ZipArchiveSource::open(target, &trusted, ArchiveLimits::default(), self.cancel)
            && archive.member_count() == 1
        {
            let mut budget = ArchiveRunBudget::new(MAX_CONTENT_CHECK_BYTES);
            let outcome = archive.verify_all(self.cancel, &mut budget);
            self.summary.bytes_read += budget.consumed();
            if let [member] = outcome.members.as_slice()
                && let Some(hashes) = &member.hashes
            {
                let inner = agree(recorded, &hashes.crc32, &hashes.md5, &hashes.sha1);
                verdict = match (verdict, inner) {
                    (_, Some(true)) => Some(true),
                    (None, other) => other,
                    (earlier, _) => earlier,
                };
            }
        }
        match verdict {
            Some(true) => {
                self.summary.matched += 1;
                ContentVerdict::Match
            }
            Some(false) => {
                self.summary.differed += 1;
                ContentVerdict::Differs
            }
            None => {
                self.summary.not_comparable += 1;
                ContentVerdict::NotComparable
            }
        }
    }
}

#[derive(Deserialize)]
struct LiteIdentity {
    #[serde(default)]
    size_bytes: u64,
    #[serde(default)]
    modified_unix: i64,
    #[serde(default)]
    ino: u64,
    #[serde(default)]
    dev: u64,
}

#[derive(Deserialize)]
struct LiteOperation {
    #[serde(default)]
    kind: String,
}

#[derive(Deserialize)]
struct LiteEntry {
    source_path: PathBuf,
    destination_path: PathBuf,
    #[serde(default)]
    state: String,
    #[serde(default)]
    operation: Option<LiteOperation>,
    #[serde(default)]
    identity: Option<LiteIdentity>,
    #[serde(default)]
    applied_at_unix: Option<i64>,
}

#[derive(Deserialize)]
struct LiteJournal {
    #[serde(default)]
    entries: Vec<LiteEntry>,
}

/// Applied moves from EmuWiz's rename journals, read as history only.
///
/// The journal module's own reader exists to authorise Undo and recovery, so
/// it refuses a journal whose folders have since been redirected. Those are
/// exactly the journals reconciliation needs, and nothing here acts on one: a
/// move counts only when the inode it recorded is found on disk. So the few
/// fields needed are read directly. Returns how many journals were unreadable.
fn journal_moves(dir: &Path, moves: &mut Vec<JournalMove>) -> usize {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    let mut unreadable = 0;
    for path in files {
        let regular = fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_JOURNAL_BYTES);
        let journal = regular
            .then(|| fs::read(&path).ok())
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<LiteJournal>(&bytes).ok());
        let Some(journal) = journal else {
            unreadable += 1;
            continue;
        };
        for entry in journal.entries {
            let is_move = entry.operation.as_ref().is_none_or(|operation| {
                operation.kind.is_empty() || operation.kind == "rename_move"
            });
            let Some(identity) = entry.identity.filter(|identity| identity.ino != 0) else {
                continue;
            };
            if entry.state == "applied" && is_move {
                moves.push(JournalMove {
                    from: entry.source_path,
                    to: entry.destination_path,
                    inode: identity.ino,
                    device: identity.dev,
                    size: identity.size_bytes,
                    modified: identity.modified_unix,
                    applied_at: entry.applied_at_unix,
                });
            }
        }
    }
    unreadable
}

/// Library folders worth listing: configured ones and retired ones that still
/// own rows, without folders nested inside another one.
fn library_folders(sources: &[CollectionSourceFolder]) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = sources
        .iter()
        .filter(|source| source.configured || source.rows > 0)
        .map(|source| source.path.clone())
        .filter(|path| fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        .collect();
    folders.sort();
    folders.dedup();
    let all = folders.clone();
    folders.retain(|path| {
        !all.iter()
            .any(|other| other != path && path.starts_with(other))
    });
    folders
}

/// Runs the whole reconciliation. Nothing is changed; see the module notes.
pub fn reconcile(
    request: &ReconcileRequest,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ReconciliationReport, ReconcileError> {
    let mut notes = Vec::new();
    let database = Database::open_read_only(&request.database_path)
        .map_err(|error| ReconcileError::Database(error.to_string()))?;
    let rows = database
        .collection_record_rows()
        .map_err(|error| ReconcileError::Database(error.to_string()))?;
    let sources = database
        .collection_source_folders()
        .map_err(|error| ReconcileError::Database(error.to_string()))?;
    let scan_runs: Vec<ScanRun> = database
        .collection_scan_runs()
        .map_err(|error| ReconcileError::Database(error.to_string()))?
        .into_iter()
        .filter_map(|run| {
            Some(ScanRun {
                started: super::reconcile::unix_from_iso(&run.started_at)?,
                finished: super::reconcile::unix_from_iso(&run.finished_at)?,
                completed: run.status == "completed",
                coverage: run.coverage.map(|folders| {
                    folders
                        .into_iter()
                        .map(|(folder, state)| (folder, state == "complete"))
                        .collect()
                }),
            })
        })
        .collect();
    drop(database);

    let folders = library_folders(&sources);
    if folders.is_empty() {
        return Err(ReconcileError::NoLibraryFolders);
    }
    let inventory = take_inventory(
        &InventoryRequest {
            roots: &folders,
            max_entries: request.max_entries,
        },
        cancel,
        &mut |entries| {
            progress(Progress {
                phase: "Listing library folders",
                done: entries,
                total: None,
            })
        },
    )
    .map_err(|error| match error {
        InventoryError::Cancelled => ReconcileError::Cancelled,
        InventoryError::NoFolders => ReconcileError::NoLibraryFolders,
    })?;
    if !inventory.summary.complete {
        return Err(ReconcileError::IncompleteListing(
            inventory
                .summary
                .roots
                .iter()
                .filter(|root| !root.complete)
                .map(|root| format!("{}: {}", root.root.display(), root.errors.join(", ")))
                .collect(),
        ));
    }

    // What can be judged. A row whose nearest existing folder is empty is not
    // judged: an empty folder that the library has rows under is far more
    // likely a drive that is not attached than a deleted collection. Neither
    // is a row under a filesystem the listing did not enter.
    let mut non_empty: HashSet<&Path> = HashSet::new();
    for entry in &inventory.entries {
        if let Some(parent) = entry.path.parent() {
            non_empty.insert(parent);
        }
    }
    let judgeable = |path: &Path| -> bool {
        if !inventory.covers(path) {
            return false;
        }
        let mut folder = path.parent();
        while let Some(current) = folder {
            if non_empty.contains(current) {
                return true;
            }
            if inventory.get(current).is_some() || inventory.is_root(current) {
                return false; // it exists, and it is empty
            }
            folder = current.parent();
        }
        false
    };
    let source_device: HashMap<i64, Option<u64>> = sources
        .iter()
        .map(|source| {
            (
                source.id,
                fs::metadata(&source.path)
                    .ok()
                    .map(|metadata| metadata.dev()),
            )
        })
        .collect();
    let source_root: HashMap<i64, &Path> = sources
        .iter()
        .map(|source| (source.id, source.path.as_path()))
        .collect();
    let mut not_judged: BTreeMap<i64, u64> = BTreeMap::new();

    let mut old = Vec::new();
    let mut current = Vec::new();
    let mut measures = Measures {
        rows_in_library: rows.len() as u64,
        ..Measures::default()
    };
    let mut objects: HashSet<(u64, u64)> = HashSet::new();
    let library_paths: HashSet<&Path> =
        rows.iter().map(|row| row.absolute_path.as_path()).collect();
    let old_from_row = |row: &CollectionRecordRow, set: RecordSet| OldRecord {
        set,
        archive_id: row.id,
        path: row.absolute_path.clone(),
        archive_kind: row.archive_kind.clone(),
        size: row.size_bytes,
        mtime: row.modified_time_unix_seconds,
        first_seen_at: Some(row.first_seen_at.clone()),
        last_seen_at: Some(row.last_seen_at.clone()),
        flagged_missing_at: row.last_verified_missing_at.clone(),
        platform: row.platform.clone(),
        source_device: source_device.get(&row.source_folder_id).copied().flatten(),
        source_root: source_root
            .get(&row.source_folder_id)
            .map(|root| root.to_path_buf()),
        source_folder_id: Some(row.source_folder_id),
    };
    for row in &rows {
        // Present means the thing the row recorded is at its path: a regular
        // file (a folder for a game that is a folder), on the library folder's
        // own disk, and not reached through a link. The library scan follows
        // no link, so anything else is not what the scan recorded there.
        let present = inventory.get(&row.absolute_path).filter(|entry| {
            let right_kind = if DIRECTORY_KINDS.contains(&row.archive_kind.as_str()) {
                entry.kind == EntryKind::Directory
            } else {
                entry.kind == EntryKind::File
            };
            right_kind
                && !inventory.through_followed_link(&row.absolute_path)
                && source_device
                    .get(&row.source_folder_id)
                    .copied()
                    .flatten()
                    .is_none_or(|device| device == entry.device)
        });
        let can_judge = present.is_some() || judgeable(&row.absolute_path);
        if row.last_verified_missing_at.is_some() {
            measures.rows_flagged_missing += 1;
            if can_judge {
                old.push(old_from_row(row, RecordSet::FlaggedMissing));
            } else {
                measures.rows_not_checked += 1;
                *not_judged.entry(row.source_folder_id).or_default() += 1;
            }
            continue;
        }
        match present {
            Some(entry) => {
                if entry.kind == EntryKind::Directory {
                    measures.rows_present_folders += 1;
                } else if Some(entry.size) == row.size_bytes
                    && Some(entry.mtime) == row.modified_time_unix_seconds
                {
                    measures.rows_present_as_recorded += 1;
                } else {
                    measures.rows_present_but_changed += 1;
                }
                if entry.kind == EntryKind::File && objects.insert((entry.device, entry.inode)) {
                    measures.bytes_of_distinct_present_files = measures
                        .bytes_of_distinct_present_files
                        .saturating_add(entry.size);
                }
                current.push(CurrentRow {
                    archive_id: row.id,
                    path: row.absolute_path.clone(),
                    archive_kind: row.archive_kind.clone(),
                    first_seen_at: row.first_seen_at.clone(),
                    source_folder_id: row.source_folder_id,
                });
            }
            None if can_judge => {
                measures.rows_absent_never_flagged += 1;
                old.push(old_from_row(row, RecordSet::AbsentNeverFlagged));
            }
            None => {
                measures.rows_not_checked += 1;
                *not_judged.entry(row.source_folder_id).or_default() += 1;
            }
        }
    }
    for (source_id, count) in &not_judged {
        let folder = sources
            .iter()
            .find(|source| source.id == *source_id)
            .map(|source| source.path.display().to_string())
            .unwrap_or_default();
        notes.push(format!(
            "{count} library rows under {folder} were not checked: their folder is missing or empty (its drive may not be attached), is reached through a link, or is on another filesystem that was not entered"
        ));
    }
    measures.distinct_files_behind_present_rows = objects.len() as u64;

    // Forget Missing exports: beside the database when written for it, plus
    // any the caller names. A row that is back in the library is not counted twice.
    let mut receipt_paths: Vec<(PathBuf, bool)> = receipts_beside(&request.database_path)
        .into_iter()
        .map(|path| (path, false))
        .collect();
    for path in &request.forget_receipts {
        if let Some(existing) = receipt_paths.iter_mut().find(|(known, _)| known == path) {
            existing.1 = true;
        } else {
            receipt_paths.push((path.clone(), true));
        }
    }
    let mut receipts = Vec::new();
    let mut seen_forgotten: HashSet<PathBuf> = HashSet::new();
    for (path, named) in receipt_paths {
        match read_receipt(&path, &request.database_path) {
            Ok((summary, forgotten)) => {
                if !summary.written_for_this_database && !named {
                    notes.push(format!(
                        "{} was written for another database and was not used; name it explicitly to include it",
                        path.display()
                    ));
                    continue;
                }
                if summary.state.as_deref() == Some("undone") {
                    notes.push(format!(
                        "{} was undone; its rows are back in the library",
                        path.display()
                    ));
                    continue;
                }
                let mut back = 0u64;
                for row in forgotten {
                    // Back in the library (under any id), or listed twice.
                    if library_paths.contains(row.path.as_path())
                        || !seen_forgotten.insert(row.path.clone())
                    {
                        back += 1;
                        continue;
                    }
                    if !judgeable(&row.path) && inventory.get(&row.path).is_none() {
                        measures.forgotten_records_not_checked += 1;
                        continue;
                    }
                    measures.records_removed_by_forget_missing += 1;
                    old.push(OldRecord {
                        set: RecordSet::Forgotten,
                        archive_id: row.id,
                        source_device: source_device.get(&row.source_folder_id).copied().flatten(),
                        source_root: source_root
                            .get(&row.source_folder_id)
                            .map(|root| root.to_path_buf()),
                        source_folder_id: Some(row.source_folder_id),
                        path: row.path,
                        archive_kind: row.kind,
                        size: row.size,
                        mtime: row.mtime,
                        first_seen_at: row.first_seen,
                        last_seen_at: row.last_seen,
                        flagged_missing_at: row.flagged,
                        platform: row.platform,
                    });
                }
                if back > 0 {
                    notes.push(format!(
                        "{back} rows of {} are in the library again or were listed twice and are not counted again",
                        path.display()
                    ));
                }
                receipts.push(summary);
            }
            Err(detail) => notes.push(format!("{} could not be used: {detail}", path.display())),
        }
    }

    // EmuWiz's own applied moves. Older installs keep their journals in the
    // legacy folder, newer ones in the sealed folder; both are history.
    let mut moves = Vec::new();
    if let Some(parent) = request.database_path.parent() {
        let mut unreadable = 0;
        for name in [
            LEGACY_RENAME_TRANSACTIONS_DIRECTORY,
            RENAME_TRANSACTIONS_DIRECTORY,
        ] {
            let journals = parent.join(name);
            if journals.is_dir() {
                unreadable += journal_moves(&journals, &mut moves);
            }
        }
        if unreadable > 0 {
            notes.push(format!(
                "{unreadable} rename journals could not be read and were not used as evidence"
            ));
        }
    }

    progress(Progress {
        phase: "Checking links",
        done: 0,
        total: None,
    });
    let (link_findings, links) = audit_links(&inventory, cancel);
    {
        let row_paths: HashSet<&Path> =
            rows.iter().map(|row| row.absolute_path.as_path()).collect();
        measures.links_that_are_library_rows = link_findings
            .iter()
            .filter(|link| row_paths.contains(link.link.as_path()))
            .count() as u64;
    }

    let recorded_hashes = if request.verify_content {
        let wanted: HashSet<&Path> = old.iter().map(|record| record.path.as_path()).collect();
        romm_hashes(&request.database_path, &wanted)
    } else {
        HashMap::new()
    };
    let inputs = ReconcileInputs {
        old,
        current,
        moves,
        recorded_hashes,
        scan_runs,
    };
    let mut verifier = Verifier {
        cancel,
        summary: ContentCheckSummary {
            requested: request.verify_content,
            ..ContentCheckSummary::default()
        },
    };
    let records = classify_records(
        &inputs,
        &inventory,
        &link_findings,
        request
            .verify_content
            .then_some(&mut verifier as &mut dyn ContentVerifier),
        cancel,
        &mut |done, total| {
            progress(Progress {
                phase: "Matching records",
                done,
                total: Some(total),
            })
        },
    );
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(ReconcileError::Cancelled);
    }

    let mut record_sets: BTreeMap<RecordSet, BTreeMap<&'static str, TierCount>> = BTreeMap::new();
    let mut by_class: BTreeMap<EvidenceClass, u64> = BTreeMap::new();
    let mut reached: HashSet<&Path> = HashSet::new();
    for finding in &records {
        let count = record_sets
            .entry(finding.set)
            .or_default()
            .entry(finding.tier.code())
            .or_default();
        count.records += 1;
        count.recorded_bytes = count
            .recorded_bytes
            .saturating_add(finding.size_bytes.unwrap_or(0));
        *by_class.entry(finding.class).or_default() += 1;
        if finding.tier.tied_to_file() {
            measures.records_tied_to_a_file += 1;
            if let Some(target) = &finding.target {
                reached.insert(target.as_path());
            }
        }
    }
    measures.records_examined = records.len() as u64;
    measures.distinct_files_reached = reached.len() as u64;
    drop(reached);
    let not_entered: Vec<String> = inventory
        .summary
        .roots
        .iter()
        .flat_map(|root| root.other_filesystems_not_entered.iter())
        .map(|folder| folder.display().to_string())
        .collect();
    if !not_entered.is_empty() {
        const NAMED: usize = 5;
        notes.push(format!(
            "{} folders inside the library folders are on other filesystems and were not entered: library rows under them were not checked, and a file moved into one of them would not be found ({}{})",
            not_entered.len(),
            not_entered
                .iter()
                .take(NAMED)
                .cloned()
                .collect::<Vec<_>>()
                .join("; "),
            if not_entered.len() > NAMED { "; ..." } else { "" }
        ));
    }
    if !request.verify_content {
        notes.push(
            "file contents were not read; confirmed records rest on EmuWiz's move record and the inode".into(),
        );
    }
    notes.push(
        "a metadata match rests on recorded facts such as name, size and date; it does not show that two files hold the same bytes".into(),
    );
    let summary = inventory.summary.clone();
    Ok(ReconciliationReport {
        schema_version: REPORT_SCHEMA_VERSION,
        database_path: request.database_path.clone(),
        generated_at_unix: now_unix(),
        inventory: summary,
        measures,
        record_sets,
        by_class,
        receipts,
        content_check: verifier.summary,
        links,
        records,
        link_findings,
        notes,
    })
}
