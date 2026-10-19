//! Independent seals for rename-style transaction journals.
//!
//! Every journal write goes through [`super::journal::write_journal`], which
//! uses this store in a pending/committed generation protocol:
//!
//! 1. In one SQLite transaction, prove the journal now on disk is the one the
//!    seal records (its committed digest, or the digest of a durably recorded
//!    pending intent after a crash), then record a new pending intent
//!    (generation + 1, digest of the record about to be written).
//! 2. Write and flush the journal.
//! 3. Commit the intent as the new sealed generation.
//!
//! A crash after step 1 leaves the journal equal to either the committed or
//! the pending digest; either is reconciled from durable evidence. A journal
//! that matches neither, a missing journal, or one from another transaction,
//! workflow or namespace is refused and never adopted. An unsealed journal
//! that already exists is legacy: it is never sealed by being read or
//! rewritten, and needs the separately reviewed legacy-recovery route.
//!
//! The store is `undo-seals.sqlite3` beside the library database, in WAL mode
//! with `synchronous=FULL`, readable only by its owner. It is independent of
//! the library schema.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::model::{RenameTransaction, TransactionState};

pub const SEAL_STORE_FILENAME: &str = "undo-seals.sqlite3";

/// Prefix of every refusal caused by a missing, changed or unverifiable seal.
pub const JOURNAL_SEAL_REFUSED: &str = "journal integrity lost";
/// Prefix of the refusal for a journal written before seals existed.
pub const LEGACY_UNSEALED_JOURNAL: &str = "legacy unsealed journal";
/// Prefix of the refusal while the journal cannot be reached. Not an
/// integrity failure: the Undo stays available for a later retry.
pub const JOURNAL_TEMPORARILY_UNAVAILABLE: &str = "journal temporarily unavailable";

/// How a journal on disk relates to its independent seal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealCheck {
    /// No journal and no seal: nothing was ever recorded.
    Absent,
    /// The journal is exactly the sealed generation.
    Sealed { generation: i64 },
    /// The journal equals a durably recorded pending intent: a crash fell
    /// between writing it and committing its seal.
    PendingIntent { generation: i64 },
    /// An unsealed journal from before seals existed.
    Legacy,
    /// The journal folder, journal or seal store cannot be reached right now.
    Unavailable(String),
    /// Missing, changed, replaced or unverifiable; never trusted.
    Mismatch(String),
}

impl SealCheck {
    pub fn trusted(&self) -> bool {
        matches!(
            self,
            SealCheck::Sealed { .. } | SealCheck::PendingIntent { .. }
        )
    }

    /// The refusal for an untrusted journal.
    pub fn refusal(&self) -> Option<String> {
        match self {
            SealCheck::Legacy => Some(format!(
                "{LEGACY_UNSEALED_JOURNAL}: this record was made before EmuWiz sealed its Undo \
                 receipts, so it cannot be verified. Nothing was moved or written. It needs the \
                 reviewed legacy-recovery route, not an ordinary Undo"
            )),
            SealCheck::Unavailable(detail) => Some(format!(
                "{JOURNAL_TEMPORARILY_UNAVAILABLE}: {detail}. Nothing was moved or written; Undo \
                 stays available and can be tried again once the journal can be reached"
            )),
            SealCheck::Mismatch(detail) => Some(format!(
                "{JOURNAL_SEAL_REFUSED}: {detail}. Nothing further was written or moved; this \
                 Undo needs a reviewed recovery of its journal before it can run again"
            )),
            _ => None,
        }
    }
}

/// Whether an error is a seal refusal (changed journal or legacy journal).
pub fn is_seal_refusal(error: &str) -> bool {
    error.contains(JOURNAL_SEAL_REFUSED) || error.contains(LEGACY_UNSEALED_JOURNAL)
}

/// The workflow a transaction's Apply recorded.
pub fn workflow_of(transaction: &RenameTransaction) -> String {
    if let Some(workflow) = transaction
        .unknown
        .get("workflow")
        .and_then(|value| value.as_str())
    {
        return workflow.to_string();
    }
    if transaction
        .unknown
        .contains_key("mame_internal_repair_plan_digest")
    {
        return "mame_internal_repair".into();
    }
    "rename_apply".into()
}

/// Canonical SHA-256 over every recovery-relevant field: the whole record with
/// object keys sorted, excluding per-attempt preflight diagnostics.
pub fn digest(transaction: &RenameTransaction) -> [u8; 32] {
    let mut value = serde_json::to_value(transaction).unwrap_or(serde_json::Value::Null);
    if let Some(entries) = value
        .get_mut("entries")
        .and_then(|entries| entries.as_array_mut())
    {
        for entry in entries {
            if let Some(entry) = entry.as_object_mut() {
                entry.remove("preflight_passed");
                entry.remove("preflight_failures");
            }
        }
    }
    let mut hasher = Sha256::new();
    canonical(&value, &mut hasher);
    hasher.finalize().into()
}

fn canonical(value: &serde_json::Value, hasher: &mut Sha256) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            hasher.update(b"{");
            for key in keys {
                hasher.update(serde_json::to_string(key).unwrap_or_default().as_bytes());
                hasher.update(b":");
                canonical(&map[key], hasher);
                hasher.update(b",");
            }
            hasher.update(b"}");
        }
        serde_json::Value::Array(items) => {
            hasher.update(b"[");
            for item in items {
                canonical(item, hasher);
                hasher.update(b",");
            }
            hasher.update(b"]");
        }
        other => hasher.update(other.to_string().as_bytes()),
    }
}

/// The store location: beside the library database. Test binaries resolve it
/// inside their private, isolated data directory.
pub fn store_path() -> Result<PathBuf, String> {
    crate::app_dirs::data_path(SEAL_STORE_FILENAME)
        .map_err(|error| format!("cannot resolve the Undo seal store: {error}"))
}

/// Confirms this build can run the sealed-journal protocol: its seal store
/// opens and is prepared.
pub fn self_check() -> Result<(), String> {
    with_writer(|_| Ok(()))
}

/// Runs `work` on this process's single writer connection. Writers in one
/// process are serialised here instead of contending for the store's write
/// lock; other processes are covered by the busy timeout. Readers use their
/// own read-only connections, which WAL never blocks.
fn with_writer<T>(work: impl FnOnce(&mut Connection) -> Result<T, String>) -> Result<T, String> {
    static WRITER: std::sync::Mutex<Option<(PathBuf, Connection)>> = std::sync::Mutex::new(None);
    let path = store_path()?;
    let mut guard = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = guard
        .as_ref()
        .is_some_and(|(open_path, _)| open_path == &path && path.is_file());
    if !current {
        let connection = open(true)?.ok_or("the Undo seal store is unavailable")?;
        *guard = Some((path, connection));
    }
    let (_, connection) = guard.as_mut().expect("writer connection was just opened");
    work(connection)
}

fn open(write: bool) -> Result<Option<Connection>, String> {
    let path = store_path()?;
    if !write && std::fs::symlink_metadata(&path).is_err() {
        return Ok(None);
    }
    if write && let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create the Undo seal store folder: {error}"))?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&path)
        && !metadata.file_type().is_file()
    {
        return Err("the Undo seal store is not a plain file".into());
    }
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
    };
    let connection = Connection::open_with_flags(&path, flags)
        .map_err(|error| format!("cannot open the Undo seal store: {error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|error| error.to_string())?;
    if write {
        // Setting up a new store races between connections: the WAL switch is
        // not covered by the busy timeout. Serialise it in this process and
        // retry briefly for another process.
        static SETUP: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _setup = SETUP
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut attempts = 0;
        loop {
            let mode: Result<String, _> =
                connection.pragma_query_value(None, "journal_mode", |row| row.get(0));
            let result = match mode {
                Ok(mode) if mode.eq_ignore_ascii_case("wal") => Ok(()),
                Ok(_) => connection
                    .pragma_update(None, "journal_mode", "WAL")
                    .map(|_| ()),
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => break,
                Err(error) if attempts < 200 && error.to_string().contains("locked") => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(format!(
                        "cannot enable WAL for the Undo seal store: {error}"
                    ));
                }
            }
        }
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS journal_seals (
                    namespace_id TEXT NOT NULL,
                    transaction_id TEXT NOT NULL,
                    workflow TEXT NOT NULL,
                    namespace TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    digest BLOB,
                    pending_generation INTEGER,
                    pending_digest BLOB,
                    PRIMARY KEY (namespace_id, transaction_id)
                );",
            )
            .map_err(|error| format!("cannot prepare the Undo seal store: {error}"))?;
        restrict_permissions(&path);
    } else {
        connection
            .pragma_update(None, "query_only", true)
            .map_err(|error| error.to_string())?;
    }
    Ok(Some(connection))
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for candidate in [
        path.to_path_buf(),
        path.with_extension("sqlite3-wal"),
        path.with_extension("sqlite3-shm"),
    ] {
        let _ = std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// The journal folder's stable identity: a random id in a file inside the
/// folder, created with its first sealed write. Seals are keyed by it, never by
/// the folder's path or inode, so a complete recovery set (folder, identity
/// and seal store) stays verifiable after a move or restore.
pub const NAMESPACE_FILE: &str = ".emuwiz-journal-namespace";

fn namespace_id(journal_dir: &Path, create: bool) -> Result<Option<String>, SealCheck> {
    let path = journal_dir.join(NAMESPACE_FILE);
    let read = |path: &Path| -> Result<Option<String>, SealCheck> {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(SealCheck::Unavailable(format!(
                    "the journal folder identity cannot be inspected ({error})"
                )));
            }
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(SealCheck::Mismatch(
                    "the journal folder identity is not a plain file".into(),
                ));
            }
            Ok(_) => {}
        }
        let text = std::fs::read_to_string(path).map_err(|error| {
            SealCheck::Unavailable(format!(
                "the journal folder identity cannot be read ({error})"
            ))
        })?;
        let id = text.trim();
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SealCheck::Mismatch(
                "the journal folder identity is not valid".into(),
            ));
        }
        Ok(Some(id.to_ascii_lowercase()))
    };
    if let Some(id) = read(&path)? {
        return Ok(Some(id));
    }
    if !create {
        return Ok(None);
    }
    let mut random = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| std::io::Read::read_exact(&mut source, &mut random))
        .map_err(|error| {
            SealCheck::Unavailable(format!(
                "cannot create the journal folder identity ({error})"
            ))
        })?;
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    // Written and flushed under a private name, then linked into place, so a
    // crash can never leave an empty or partial identity at the real name.
    let staging = journal_dir.join(format!(".{NAMESPACE_FILE}.{id}.staging"));
    let staged = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, format!("{id}\n").as_bytes())?;
            file.sync_all()
        });
    let linked = staged.and_then(|()| std::fs::hard_link(&staging, &path));
    let _ = std::fs::remove_file(&staging);
    match linked {
        Ok(()) => {
            if let Ok(folder) = std::fs::File::open(journal_dir) {
                let _ = folder.sync_all();
            }
            Ok(Some(id))
        }
        // Created concurrently: use the one that won.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read(&path),
        Err(error) => Err(SealCheck::Unavailable(format!(
            "cannot create the journal folder identity ({error})"
        ))),
    }
}

#[derive(Debug)]
struct Row {
    workflow: String,
    namespace: String,
    generation: i64,
    digest: Option<Vec<u8>>,
    pending_generation: Option<i64>,
    pending_digest: Option<Vec<u8>>,
}

fn load(connection: &Connection, dir: &str, id: &str) -> Result<Option<Row>, String> {
    connection
        .query_row(
            "SELECT workflow, namespace, generation, digest, pending_generation, pending_digest
             FROM journal_seals WHERE namespace_id = ?1 AND transaction_id = ?2",
            params![dir, id],
            |row| {
                Ok(Row {
                    workflow: row.get(0)?,
                    namespace: row.get(1)?,
                    generation: row.get(2)?,
                    digest: row.get(3)?,
                    pending_generation: row.get(4)?,
                    pending_digest: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|error| format!("cannot read the Undo seal store: {error}"))
}

/// The journal on disk, parsed only as a plain file. `Ok(None)` when absent.
fn read_disk(path: &Path) -> Result<Option<RenameTransaction>, String> {
    read_disk_checked(path).map_err(|check| match check {
        SealCheck::Unavailable(detail) | SealCheck::Mismatch(detail) => detail,
        _ => String::new(),
    })
}

/// [`read_disk`], telling an unreachable journal (`Unavailable`) from one that
/// is not a valid journal (`Mismatch`).
fn read_disk_checked(path: &Path) -> Result<Option<RenameTransaction>, SealCheck> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SealCheck::Unavailable(format!(
                "the journal cannot be inspected ({error})"
            )));
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(SealCheck::Mismatch(
                "the journal is no longer a plain file".into(),
            ));
        }
        Ok(_) => {}
    }
    let bytes = std::fs::read(path)
        .map_err(|error| SealCheck::Unavailable(format!("the journal cannot be read ({error})")))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| SealCheck::Mismatch("the journal on disk can no longer be read".into()))
}

/// The journal folder is present as a directory; otherwise it is unavailable
/// (missing, unmounted or obstructed), which is not a seal failure.
fn folder_available(journal_dir: &Path) -> Result<(), SealCheck> {
    match std::fs::metadata(journal_dir) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(SealCheck::Unavailable(
            "the journal folder is not a folder right now".into(),
        )),
        Err(error) => Err(SealCheck::Unavailable(format!(
            "the journal folder cannot be reached ({error})"
        ))),
    }
}

/// Compares a journal with its seal row without changing anything.
fn classify(
    row: Option<&Row>,
    journal: Option<&RenameTransaction>,
    transaction_id: &str,
) -> SealCheck {
    let Some(row) = row else {
        return match journal {
            None => SealCheck::Absent,
            Some(_) => SealCheck::Legacy,
        };
    };
    let Some(journal) = journal else {
        // A first write that crashed before its journal existed.
        if row.digest.is_none() {
            return SealCheck::Absent;
        }
        return SealCheck::Mismatch("the sealed journal is missing".into());
    };
    if journal.transaction_id != transaction_id {
        return SealCheck::Mismatch("the journal belongs to another transaction".into());
    }
    if workflow_of(journal) != row.workflow {
        return SealCheck::Mismatch("the journal belongs to another workflow".into());
    }
    if journal.source_scan_root != row.namespace {
        return SealCheck::Mismatch("the journal belongs to another namespace".into());
    }
    let actual = digest(journal);
    if row.digest.as_deref() == Some(actual.as_slice()) {
        return SealCheck::Sealed {
            generation: row.generation,
        };
    }
    if let (Some(generation), Some(pending)) = (row.pending_generation, &row.pending_digest)
        && pending.as_slice() == actual.as_slice()
    {
        return SealCheck::PendingIntent { generation };
    }
    SealCheck::Mismatch("the journal on disk does not match its independent seal".into())
}

/// Read-only verification of one journal against its seal.
pub fn verify(journal_dir: &Path, transaction_id: &str) -> SealCheck {
    let Some(path) = super::journal::journal_path(journal_dir, transaction_id) else {
        return SealCheck::Mismatch("the journal cannot be named for this transaction".into());
    };
    if let Err(check) = folder_available(journal_dir) {
        return check;
    }
    let journal = match read_disk_checked(&path) {
        Ok(journal) => journal,
        Err(check) => return check,
    };
    // A folder without its identity holds no sealed journals.
    let namespace = match namespace_id(journal_dir, false) {
        Ok(Some(id)) => id,
        Ok(None) => return classify(None, journal.as_ref(), transaction_id),
        Err(check) => return check,
    };
    let connection = match open(false) {
        Ok(Some(connection)) => connection,
        Ok(None) => return classify(None, journal.as_ref(), transaction_id),
        Err(detail) => return SealCheck::Unavailable(detail),
    };
    let row = match load(&connection, &namespace, transaction_id) {
        Ok(row) => row,
        Err(error) if error.contains("no such table") => None,
        Err(detail) => return SealCheck::Unavailable(detail),
    };
    classify(row.as_ref(), journal.as_ref(), transaction_id)
}

/// A recorded pending intent awaiting [`commit`] after the journal write.
pub(crate) struct Intent {
    dir: String,
    transaction_id: String,
    generation: i64,
}

/// Step 1: prove the current journal is the sealed one and record the intent
/// to write `transaction`.
pub(crate) fn begin(
    journal_dir: &Path,
    path: &Path,
    transaction: &RenameTransaction,
) -> Result<Intent, String> {
    super::writer_gate::check()?;
    // As the journal writer always has, create a missing journal folder.
    std::fs::create_dir_all(journal_dir)
        .map_err(|error| format!("cannot create the journal folder: {error}"))?;
    // Read the folder identity first: a refusal (a legacy journal, a changed
    // one) must not create anything. It is created only for a first sealed
    // journal in a folder that has none.
    let existing =
        namespace_id(journal_dir, false).map_err(|check| check.refusal().unwrap_or_default())?;
    let dir = match existing {
        Some(id) => id,
        None => {
            if let Some(refusal) = classify(
                None,
                read_disk_checked(path)
                    .map_err(|check| check.refusal().unwrap_or_default())?
                    .as_ref(),
                &transaction.transaction_id,
            )
            .refusal()
            {
                return Err(refusal);
            }
            namespace_id(journal_dir, true)
                .map_err(|check| check.refusal().unwrap_or_default())?
                .ok_or("the journal folder identity could not be established")?
        }
    };
    let generation = with_writer(|connection| {
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("cannot lock the Undo seal store: {error}"))?;
        let row = load(&tx, &dir, &transaction.transaction_id)?;
        let on_disk =
            read_disk_checked(path).map_err(|check| check.refusal().unwrap_or_default())?;
        let check = classify(row.as_ref(), on_disk.as_ref(), &transaction.transaction_id);
        if let Some(refusal) = check.refusal() {
            return Err(refusal);
        }
        let digest = digest(transaction).to_vec();
        let workflow = workflow_of(transaction);
        let generation = match (&row, &check) {
            (None, _) => {
                tx.execute(
                    "INSERT INTO journal_seals (namespace_id, transaction_id, workflow, namespace,
                                            generation, digest, pending_generation,
                                            pending_digest)
                 VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, ?5)",
                    params![
                        dir,
                        transaction.transaction_id,
                        workflow,
                        transaction.source_scan_root,
                        digest
                    ],
                )
                .map_err(|error| format!("cannot record the Undo seal intent: {error}"))?;
                1
            }
            (Some(row), _) => {
                if workflow != row.workflow || transaction.source_scan_root != row.namespace {
                    return Err(SealCheck::Mismatch(
                        "this record belongs to another workflow or namespace".into(),
                    )
                    .refusal()
                    .unwrap_or_default());
                }
                // A pending intent the journal already reached is the base.
                let base = match check {
                    SealCheck::PendingIntent { generation } => generation,
                    _ => row.generation,
                };
                let next = base + 1;
                tx.execute(
                    "UPDATE journal_seals SET generation = ?3,
                    digest = CASE WHEN ?3 = generation THEN digest ELSE pending_digest END,
                    pending_generation = ?4, pending_digest = ?5
                 WHERE namespace_id = ?1 AND transaction_id = ?2",
                    params![dir, transaction.transaction_id, base, next, digest],
                )
                .map_err(|error| format!("cannot record the Undo seal intent: {error}"))?;
                next
            }
        };
        tx.commit()
            .map_err(|error| format!("cannot record the Undo seal intent: {error}"))?;
        Ok(generation)
    })?;
    Ok(Intent {
        dir,
        transaction_id: transaction.transaction_id.clone(),
        generation,
    })
}

/// Step 3: the journal is durable; seal it as the new generation.
pub(crate) fn commit(intent: Intent) -> Result<(), String> {
    let changed = with_writer(|connection| {
        connection
            .execute(
                "UPDATE journal_seals SET generation = pending_generation, digest = pending_digest,
                pending_generation = NULL, pending_digest = NULL
             WHERE namespace_id = ?1 AND transaction_id = ?2 AND pending_generation = ?3",
                params![intent.dir, intent.transaction_id, intent.generation],
            )
            .map_err(|error| format!("cannot commit the Undo seal: {error}"))
    })?;
    if changed != 1 {
        return Err(SealCheck::Mismatch(
            "the Undo seal changed while its journal was being written".into(),
        )
        .refusal()
        .unwrap_or_default());
    }
    Ok(())
}

/// Forgets the seal of a journal that was deliberately removed.
pub(super) fn forget(journal_dir: &Path, transaction_id: &str) -> Result<(), String> {
    let Some(dir) =
        namespace_id(journal_dir, false).map_err(|check| check.refusal().unwrap_or_default())?
    else {
        return Ok(());
    };
    with_writer(|connection| {
        connection
            .execute(
                "DELETE FROM journal_seals WHERE namespace_id = ?1 AND transaction_id = ?2",
                params![dir, transaction_id],
            )
            .map(|_| ())
            .map_err(|error| format!("cannot update the Undo seal store: {error}"))
    })
}

/// One journal in a read-only inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryItem {
    pub transaction_id: String,
    pub workflow: Option<String>,
    pub state: Option<TransactionState>,
    pub entries: usize,
    pub seal: SealCheck,
}

/// Classifies every journal in `journal_dir` against its seal without any
/// write: no journal is rewritten, sealed or removed, and a missing store is
/// not created.
pub fn inventory(journal_dir: &Path) -> Vec<InventoryItem> {
    let mut items = Vec::new();
    let Ok(read) = std::fs::read_dir(journal_dir) else {
        return items;
    };
    let mut names: Vec<String> = read
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    for name in names {
        let id = name.trim_end_matches(".json").to_string();
        let parsed = read_disk(&journal_dir.join(&name)).ok().flatten();
        items.push(InventoryItem {
            workflow: parsed.as_ref().map(workflow_of),
            state: parsed.as_ref().map(|transaction| transaction.state),
            entries: parsed
                .as_ref()
                .map_or(0, |transaction| transaction.entries.len()),
            seal: verify(journal_dir, &id),
            transaction_id: id,
        });
    }
    items
}

#[cfg(test)]
pub(super) mod hooks {
    use std::cell::RefCell;
    thread_local! {
        /// Runs between recording the intent and writing the journal.
        pub static AFTER_INTENT: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        /// Runs between writing the journal and committing its seal; an `Err`
        /// simulates a crash at that point.
        pub static BEFORE_COMMIT: RefCell<Option<Box<dyn FnOnce() -> Result<(), String>>>> =
            const { RefCell::new(None) };
    }
}

/// How much a History record can be trusted, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceStatus {
    /// Sealed and verified: ordinary Undo may be offered.
    Sealed,
    /// Matches a durably recorded intent after a crash; reconciled on write.
    SealedPending,
    /// Written through the compact recovery authority, not the seal store.
    CompactAuthority,
    /// From a folder written before seals existed.
    Legacy,
    /// In the sealed folder, but no seal exists for it.
    MissingSeal,
    /// The file cannot be read as a journal.
    Corrupt(String),
    /// The journal no longer matches its independent seal.
    IntegrityMismatch(String),
    /// Its journal or seal cannot be reached right now.
    Unavailable(String),
}

impl EvidenceStatus {
    /// Whether ordinary Undo may be offered for this record.
    pub fn trusted(&self) -> bool {
        matches!(
            self,
            EvidenceStatus::Sealed
                | EvidenceStatus::SealedPending
                | EvidenceStatus::CompactAuthority
        )
    }

    /// The heading a person sees for an untrusted record.
    pub fn title(&self) -> &'static str {
        match self {
            EvidenceStatus::Sealed
            | EvidenceStatus::SealedPending
            | EvidenceStatus::CompactAuthority => "Sealed record",
            EvidenceStatus::Legacy => {
                "Legacy recovery record — automatic Undo unavailable until reviewed"
            }
            EvidenceStatus::MissingSeal => {
                "Unsealed record — automatic Undo unavailable until reviewed"
            }
            EvidenceStatus::Corrupt(_) => "Unreadable record — kept as evidence",
            EvidenceStatus::IntegrityMismatch(_) => {
                "Changed record — its journal no longer matches its seal"
            }
            EvidenceStatus::Unavailable(_) => "Record temporarily unavailable",
        }
    }

    /// Why ordinary Undo is not offered.
    pub fn explanation(&self) -> String {
        match self {
            EvidenceStatus::Legacy => "This record was written before EmuWiz sealed its Undo \
                records. It is shown exactly as found and is never rewritten or sealed. It \
                needs the separately reviewed legacy-recovery route."
                .into(),
            EvidenceStatus::MissingSeal => "No independent seal exists for this record, so it \
                cannot be verified. It is shown exactly as found."
                .into(),
            EvidenceStatus::Corrupt(detail) => format!(
                "The file could not be read as an Undo record ({detail}). It is kept, unchanged, \
                 as evidence."
            ),
            EvidenceStatus::IntegrityMismatch(detail) => format!(
                "{detail}. The record is never adopted as true; it needs a reviewed recovery."
            ),
            EvidenceStatus::Unavailable(detail) => {
                format!("{detail}. Reopen History once it can be reached; nothing was changed.")
            }
            _ => String::new(),
        }
    }
}

/// One journal file found for History, with how far it can be trusted.
#[derive(Debug, Clone)]
pub struct JournalEvidence {
    pub journal_dir: PathBuf,
    pub file: PathBuf,
    /// The id named by the file; the record's own id may differ.
    pub file_id: String,
    pub legacy_namespace: bool,
    pub transaction: Option<RenameTransaction>,
    pub status: EvidenceStatus,
}

/// Every recognised pre-seal journal folder that exists, once each.
pub fn recognised_legacy_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data = crate::app_dirs::data_dir().ok();
    recognised_legacy_dirs_in(home.as_deref(), data.as_deref())
}

/// [`recognised_legacy_dirs`] for an injected home and data directory: the
/// effective data directory's pre-seal folder plus the EmuWiz and ArchiveFS
/// ones under the home, deduplicated by canonical path.
pub fn recognised_legacy_dirs_in(home: Option<&Path>, data_dir: Option<&Path>) -> Vec<PathBuf> {
    let legacy = super::journal::LEGACY_RENAME_TRANSACTIONS_DIRECTORY;
    let mut candidates = Vec::new();
    if let Some(data_dir) = data_dir {
        candidates.push(data_dir.join(legacy));
    }
    if let Some(home) = home {
        for root in [".local/share/emuwiz", ".local/share/archivefs"] {
            candidates.push(home.join(root).join(legacy));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    candidates
        .into_iter()
        // Followed if it is a link: reading through it is still read-only,
        // and a relocated legacy folder must not silently disappear.
        .filter(|dir| dir.is_dir())
        .filter(|dir| seen.insert(std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone())))
        .collect()
}

/// Reads History from the sealed folder and every legacy folder without any
/// write: nothing is copied, moved, rewritten, sealed or upgraded, and a
/// missing seal store is not created. Records with the same id in different
/// folders stay separate, and unreadable files stay visible.
pub fn history_evidence(sealed_dir: &Path, legacy_dirs: &[PathBuf]) -> Vec<JournalEvidence> {
    let mut found = Vec::new();
    let mut folders = vec![(sealed_dir.to_path_buf(), false)];
    folders.extend(legacy_dirs.iter().cloned().map(|dir| (dir, true)));
    for (dir, legacy) in folders {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<String> = read
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".json") && !name.starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            let file = dir.join(&name);
            let file_id = name.trim_end_matches(".json").to_string();
            let parsed = read_disk(&file).or_else(|detail| {
                // Compact-format records are read through their own reader.
                #[cfg(target_os = "linux")]
                if super::compact_recovery::is_enrolled(&dir, &file_id) {
                    return super::journal::read_journal(&file)
                        .map(Some)
                        .map_err(|error| error.to_string());
                }
                Err(detail)
            });
            let (transaction, status) = match parsed {
                Err(detail) => (None, EvidenceStatus::Corrupt(detail)),
                Ok(None) => continue,
                Ok(Some(transaction)) if legacy => (Some(transaction), EvidenceStatus::Legacy),
                Ok(Some(transaction)) => {
                    // Sealed-folder records also pass the journal reader's own
                    // containment checks before they can be trusted.
                    if let Err(error) = super::journal::read_journal(&file) {
                        found.push(JournalEvidence {
                            journal_dir: dir.clone(),
                            file,
                            file_id,
                            legacy_namespace: false,
                            transaction: Some(transaction),
                            status: EvidenceStatus::Corrupt(error.to_string()),
                        });
                        continue;
                    }
                    #[cfg(target_os = "linux")]
                    if super::compact_recovery::is_enrolled(&dir, &file_id) {
                        found.push(JournalEvidence {
                            journal_dir: dir.clone(),
                            file,
                            file_id,
                            legacy_namespace: false,
                            transaction: Some(transaction),
                            status: EvidenceStatus::CompactAuthority,
                        });
                        continue;
                    }
                    let status = match verify(&dir, &file_id) {
                        SealCheck::Sealed { .. } => EvidenceStatus::Sealed,
                        SealCheck::PendingIntent { .. } => EvidenceStatus::SealedPending,
                        SealCheck::Legacy | SealCheck::Absent => EvidenceStatus::MissingSeal,
                        SealCheck::Mismatch(detail) => EvidenceStatus::IntegrityMismatch(detail),
                        SealCheck::Unavailable(detail) => EvidenceStatus::Unavailable(detail),
                    };
                    (Some(transaction), status)
                }
            };
            found.push(JournalEvidence {
                journal_dir: dir.clone(),
                file,
                file_id,
                legacy_namespace: legacy,
                transaction,
                status,
            });
        }
    }
    found
}
