//! Copy-first recovery from an interrupted write (a hot rollback journal).
//!
//! The open guard ([`journal_state::require_inert_rollback_journal`]) refuses
//! a library whose rollback journal has a live header, because opening it
//! makes SQLite roll the journal back and delete it, and from the file name
//! alone EmuWiz cannot tell a crashed writer from one it cannot see. That
//! guard is unchanged. This module is the explicit path out of that state:
//!
//! 1. **Prepare** (changes nothing in the library). Under the upgrade lock,
//!    with no SQLite write lock on the database, on a local filesystem whose
//!    locks can be trusted, the database and its journal are copied into a
//!    recovery folder and shown to be the same objects before and after the
//!    copy. SQLite then performs its own rollback on a *second* copy, which
//!    is checked (integrity, foreign keys, schema, saved evidence readable).
//!    Whether the journal actually restored anything is read off the result,
//!    not guessed from the journal's presence.
//! 2. **Apply** (explicitly requested). Only when the live database and
//!    journal are still byte-for-byte the preserved set does SQLite roll the
//!    journal back on the live database: the same operation, on the same
//!    bytes, that was just checked on the copy. The result must hash to the
//!    checked copy. No recovered copy ever replaces the live file.
//!
//! The preserved set and its receipt are never deleted here. Anything this
//! module cannot establish (a visible or undeterminable writer, a network
//! or FUSE filesystem, a WAL, a multi-database journal, a damaged journal)
//! is refused and left for the manual procedure in
//! `docs/DATABASE_RECOVERY.md`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::backup_ownership::UpgradeLock;
use super::journal_state::{self, RollbackJournalState};
use super::{Database, MIGRATIONS, latest_known_version, now_utc_string};
use crate::{ArchiveFsError, Result};

const RECEIPT_FORMAT: u32 = 1;
const RECEIPT_NAME: &str = "receipt.json";
/// A lock released a moment ago can still look held for a few milliseconds
/// (a process being spawned elsewhere briefly shares the descriptor).
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// SQLite's rollback-journal magic; it also ends a super-journal record.
const JOURNAL_MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

fn refuse(message: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Database(message.into())
}

/// One preserved file: what it was when it was copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreservedFile {
    pub sha256: String,
    pub bytes: u64,
}

/// Where a recovery stands. Written atomically at each step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalRecoveryState {
    /// The copy was recovered and checked. The library is untouched.
    Prepared,
    /// An apply started. The live database may be part-way through SQLite's
    /// rollback; applying again finishes it.
    Applying,
    /// SQLite rolled the journal back on the live database and the result
    /// matched the checked copy.
    Applied,
}

/// The durable record of one recovery, kept beside the preserved files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRecoveryReceipt {
    pub format: u32,
    pub state: JournalRecoveryState,
    pub database_path: PathBuf,
    pub recovery_dir: PathBuf,
    pub created_at: String,
    pub applied_at: Option<String>,
    pub database: PreservedFile,
    pub journal: PreservedFile,
    /// The database after SQLite's rollback on the isolated copy.
    pub recovered: PreservedFile,
    /// Whether the rollback changed the database at all. `false` means the
    /// journal was a leftover that restored nothing.
    pub journal_restored_content: bool,
    pub schema_version: i64,
    /// Whether this build opens the recovered copy as a current library. A
    /// copy at an older schema is whole but still needs the normal upgrade.
    pub opens_as_current_library: bool,
    /// Saved DAT rows in the recovered copy, by verdict. Read for the
    /// report only: recovery neither adds nor upgrades evidence.
    pub saved_dat_rows: BTreeMap<String, u64>,
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

fn stamp(path: &Path) -> Result<Stamp> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        refuse(format!(
            "could not inspect {}: {error}. Nothing was changed.",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(refuse(format!(
            "{} is not a regular file. Nothing was changed.",
            path.display()
        )));
    }
    Ok(Stamp {
        device: metadata.dev(),
        inode: metadata.ino(),
        len: metadata.len(),
        mtime: (metadata.mtime(), metadata.mtime_nsec()),
        ctime: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_file(path: &Path) -> Result<PreservedFile> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| refuse(format!("could not read {}: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut bytes = 0_u64;
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|error| refuse(format!("could not read {}: {error}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        bytes += n as u64;
    }
    Ok(PreservedFile {
        sha256: hex(&hasher.finalize()),
        bytes,
    })
}

/// Copies `source` to a new file, hashing what was read.
fn copy_new(source: &Path, target: &Path) -> Result<PreservedFile> {
    let error = |what: &str, path: &Path, error: std::io::Error| {
        refuse(format!("could not {what} {}: {error}", path.display()))
    };
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(source)
        .map_err(|e| error("read", source, e))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(target)
        .map_err(|e| error("create", target, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut bytes = 0_u64;
    loop {
        let n = input
            .read(&mut buffer)
            .map_err(|e| error("read", source, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        output
            .write_all(&buffer[..n])
            .map_err(|e| error("write", target, e))?;
        bytes += n as u64;
    }
    output.sync_all().map_err(|e| error("flush", target, e))?;
    Ok(PreservedFile {
        sha256: hex(&hasher.finalize()),
        bytes,
    })
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| refuse(format!("could not flush {}: {error}", path.display())))
}

/// Filesystems whose POSIX record locks SQLite and `/proc/locks` agree on.
fn local_filesystem(path: &Path) -> Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| refuse("the database path contains a NUL byte"))?;
    // SAFETY: `c_path` is a valid NUL-terminated string and `stats` is a
    // properly sized, writable buffer for `statfs`.
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stats) } != 0 {
        return Err(refuse(format!(
            "could not read the filesystem type of {}: {}. Nothing was changed.",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    const LOCAL: [i64; 7] = [
        0xEF53,      // ext2/3/4
        0x5846_5342, // xfs
        0x9123_683E, // btrfs
        0x0102_1994, // tmpfs
        0x2FC1_2FC1, // zfs
        0xF2F5_2010, // f2fs
        0x794C_7630, // overlayfs
    ];
    Ok(LOCAL.contains(&(stats.f_type as i64)))
}

/// A rollback journal that names a super-journal belongs to a transaction
/// across several databases; this module only handles a single database.
fn names_super_journal(journal: &Path) -> Result<bool> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(journal)
        .map_err(|error| refuse(format!("could not read {}: {error}", journal.display())))?;
    let len = file
        .metadata()
        .map_err(|error| refuse(format!("could not read {}: {error}", journal.display())))?
        .len();
    if len < 16 {
        return Ok(false);
    }
    let mut tail = [0_u8; 16];
    file.seek(SeekFrom::Start(len - 16))
        .and_then(|_| file.read_exact(&mut tail))
        .map_err(|error| refuse(format!("could not read {}: {error}", journal.display())))?;
    let name_len = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
    Ok(tail[8..] == JOURNAL_MAGIC && name_len > 0 && u64::from(name_len) < len)
}

/// Everything that must hold before the live files may be read or, later,
/// recovered. Returns the journal length.
fn require_recoverable_state(database_path: &Path) -> Result<u64> {
    for suffix in ["-wal", "-shm"] {
        if fs::symlink_metadata(sidecar(database_path, suffix)).is_ok() {
            return Err(refuse(format!(
                "a write-ahead log sidecar ({}) sits beside the library database. That is not \
                 an interrupted rollback-journal write; follow docs/DATABASE_RECOVERY.md. \
                 Nothing was changed.",
                sidecar(database_path, suffix).display()
            )));
        }
    }
    let RollbackJournalState::LiveHeader { bytes } =
        journal_state::rollback_journal_state(database_path)?
    else {
        return Err(refuse(
            "the library database has no interrupted write to recover: there is no rollback \
             journal with a live header beside it. Nothing was changed.",
        ));
    };
    if !local_filesystem(database_path)? {
        return Err(refuse(
            "the library database is on a filesystem whose file locks cannot be relied on \
             (network or FUSE storage), so a writer that is still running could not be seen. \
             Follow docs/DATABASE_RECOVERY.md. Nothing was changed.",
        ));
    }
    if journal_state::sqlite_write_lock_held(database_path)? {
        return Err(refuse(
            "another EmuWiz worker or process is writing the library database right now. Wait \
             for it to finish; there is nothing to recover while it runs. Nothing was changed.",
        ));
    }
    if journal_state::own_process_holds_lock(database_path)? {
        return Err(refuse(
            "this process still has the library database open with a lock. Close the library \
             and try again. Nothing was changed.",
        ));
    }
    if names_super_journal(&sidecar(database_path, "-journal"))? {
        return Err(refuse(
            "the rollback journal belongs to a transaction across several databases. Follow \
             docs/DATABASE_RECOVERY.md. Nothing was changed.",
        ));
    }
    Ok(bytes)
}

fn receipt_path(recovery_dir: &Path) -> PathBuf {
    recovery_dir.join(RECEIPT_NAME)
}

fn write_receipt(receipt: &JournalRecoveryReceipt) -> Result<()> {
    let path = receipt_path(&receipt.recovery_dir);
    let temporary = receipt.recovery_dir.join("receipt.json.tmp");
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| refuse(format!("could not encode the recovery receipt: {error}")))?;
    let error = |error: std::io::Error| {
        refuse(format!(
            "could not write the recovery receipt {}: {error}",
            path.display()
        ))
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)
        .map_err(error)?;
    file.write_all(&bytes).map_err(error)?;
    file.sync_all().map_err(error)?;
    fs::rename(&temporary, &path).map_err(error)?;
    sync_dir(&receipt.recovery_dir)
}

/// Reads the receipt of an earlier [`prepare_journal_recovery`].
pub fn read_journal_recovery_receipt(recovery_dir: &Path) -> Result<JournalRecoveryReceipt> {
    let path = receipt_path(recovery_dir);
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .and_then(|file| file.take(1024 * 1024).read_to_end(&mut bytes))
        .map_err(|error| {
            refuse(format!(
                "{} has no finished recovery receipt ({error}). Only a recovery that was \
                 prepared and checked can be applied.",
                recovery_dir.display()
            ))
        })?;
    let receipt: JournalRecoveryReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| refuse(format!("the recovery receipt is unreadable: {error}")))?;
    if receipt.format != RECEIPT_FORMAT {
        return Err(refuse(format!(
            "the recovery receipt has format {}, which this build does not understand",
            receipt.format
        )));
    }
    Ok(receipt)
}

/// Lets SQLite roll back on `database` (a private copy, or the live file
/// during an authorised apply) and checks the result. Returns the schema
/// version and saved DAT rows by verdict.
fn sqlite_rollback_and_check(database: &Path) -> Result<(i64, BTreeMap<String, u64>)> {
    let error = |what: &str, error: rusqlite::Error| {
        refuse(format!(
            "SQLite could not {what} {}: {error}",
            database.display()
        ))
    };
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| error("open", e))?;
    // The first read is what makes SQLite replay a hot journal. It checks
    // for a live writer itself first, and reports "busy" if there is one.
    let _: i64 = connection
        .query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get(0))
        .map_err(|e| error("recover", e))?;
    let integrity: Vec<String> = connection
        .prepare("PRAGMA integrity_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get(0))
                .and_then(|rows| rows.collect())
        })
        .map_err(|e| error("check", e))?;
    if integrity != ["ok"] {
        return Err(refuse(format!(
            "after SQLite's rollback {} fails its integrity check: {}",
            database.display(),
            integrity.join("; ")
        )));
    }
    let broken_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .and_then(|mut statement| statement.exists([]))
        .map_err(|e| error("check", e))?;
    if broken_keys {
        return Err(refuse(format!(
            "after SQLite's rollback {} has broken record relationships",
            database.display()
        )));
    }
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| error("read", e))?;
    let mut saved = BTreeMap::new();
    let has_identities: bool = connection
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='library_dat_identities'")
        .and_then(|mut statement| statement.exists([]))
        .map_err(|e| error("read", e))?;
    if has_identities {
        let mut statement = connection
            .prepare("SELECT verification_state, COUNT(*) FROM library_dat_identities GROUP BY 1")
            .map_err(|e| error("read", e))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| error("read", e))?;
        for row in rows {
            let (state, count) = row.map_err(|e| error("read", e))?;
            saved.insert(state, count.max(0) as u64);
        }
    }
    connection.close().map_err(|(_, e)| error("close", e))?;
    Ok((version, saved))
}

/// Preserves the database and its journal, recovers a copy and checks it.
/// Changes nothing in the library: the live database and journal are only
/// read. Creates one recovery folder beside the database.
pub fn prepare_journal_recovery(database_path: &Path) -> Result<JournalRecoveryReceipt> {
    let _lock = UpgradeLock::acquire_waiting(database_path, LOCK_WAIT)?;
    let journal_path = sidecar(database_path, "-journal");
    let journal_bytes = require_recoverable_state(database_path)?;
    let before = (stamp(database_path)?, stamp(&journal_path)?);
    let file_name = database_path
        .file_name()
        .ok_or_else(|| refuse("the library database path has no file name"))?
        .to_os_string();
    let parent = database_path
        .parent()
        .ok_or_else(|| refuse("the library database path has no parent folder"))?;

    // Two copies of the database and of the journal, plus headroom.
    // SAFETY: zeroed `statvfs` is a valid out-buffer; the path is NUL-terminated.
    let free = {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(parent.as_os_str().as_bytes())
            .map_err(|_| refuse("the database folder path contains a NUL byte"))?;
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
            return Err(refuse(
                "could not read the free space beside the library database",
            ));
        }
        (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64)
    };
    let needed = 2 * (before.0.len + journal_bytes) + 64 * 1024 * 1024;
    if free < needed {
        return Err(refuse(format!(
            "preparing a recovery needs about {} MiB free beside the library database and \
             only {} MiB is free. Nothing was changed.",
            needed / (1024 * 1024),
            free / (1024 * 1024)
        )));
    }

    let created_at = now_utc_string();
    use std::os::unix::fs::DirBuilderExt;
    let make = |path: &Path| {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|error| {
                refuse(format!(
                    "could not create {}: {error}. Nothing was changed.",
                    path.display()
                ))
            })
    };
    // A new folder every time: an earlier recovery is never reused or
    // written into, even one made in the same second.
    let mut attempt = 0_u32;
    let recovery_dir = loop {
        let mut dir_name = file_name.clone();
        dir_name.push(format!(
            ".journal-recovery-{}-{}{}",
            created_at.replace([':', '-'], ""),
            std::process::id(),
            if attempt == 0 {
                String::new()
            } else {
                format!("-{attempt}")
            }
        ));
        let candidate = parent.join(dir_name);
        match fs::DirBuilder::new().mode(0o700).create(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 1000 => {
                attempt += 1;
            }
            Err(error) => {
                return Err(refuse(format!(
                    "could not create {}: {error}. Nothing was changed.",
                    candidate.display()
                )));
            }
        }
    };
    let original_dir = recovery_dir.join("original");
    let recovered_dir = recovery_dir.join("recovered");
    make(&original_dir)?;
    make(&recovered_dir)?;

    let mut journal_name = file_name.clone();
    journal_name.push("-journal");
    let database = copy_new(database_path, &original_dir.join(&file_name))?;
    let journal = copy_new(&journal_path, &original_dir.join(&journal_name))?;
    sync_dir(&original_dir)?;
    // The pair is only a recovery set if nothing wrote either file while it
    // was being copied, and no writer has appeared.
    let after = (stamp(database_path)?, stamp(&journal_path)?);
    if before != after
        || database.bytes != before.0.len
        || journal.bytes != before.1.len
        || journal_state::sqlite_write_lock_held(database_path)?
    {
        return Err(refuse(format!(
            "the library database or its journal changed while it was being copied, so the \
             copy in {} is not a recovery set and has no receipt. Something is still writing \
             the library. Nothing was changed.",
            recovery_dir.display()
        )));
    }

    // SQLite recovers a second copy; the preserved set is never opened by it.
    let work = recovered_dir.join(&file_name);
    let work_copy = copy_new(&original_dir.join(&file_name), &work)?;
    let work_journal = copy_new(
        &original_dir.join(&journal_name),
        &recovered_dir.join(&journal_name),
    )?;
    if work_copy != database || work_journal != journal {
        return Err(refuse(
            "the preserved recovery set changed while it was being copied",
        ));
    }
    sync_dir(&recovered_dir)?;
    let (schema_version, saved_dat_rows) = sqlite_rollback_and_check(&work)?;
    if journal_state::rollback_journal_state(&work)?
        == (RollbackJournalState::LiveHeader {
            bytes: journal.bytes,
        })
    {
        return Err(refuse(
            "SQLite did not treat the journal as an interrupted write on the copy. Follow \
             docs/DATABASE_RECOVERY.md. Nothing was changed.",
        ));
    }
    let recovered = hash_file(&work)?;
    let opens_as_current_library = schema_version == latest_known_version(MIGRATIONS)
        && Database::open_read_only(&work)
            .and_then(|db| {
                db.saved_dat_evidence_status(&Default::default())
                    .map(|_| ())
            })
            .is_ok();
    let receipt = JournalRecoveryReceipt {
        format: RECEIPT_FORMAT,
        state: JournalRecoveryState::Prepared,
        database_path: database_path.to_path_buf(),
        recovery_dir,
        created_at,
        applied_at: None,
        journal_restored_content: recovered.sha256 != database.sha256,
        database,
        journal,
        recovered,
        schema_version,
        opens_as_current_library,
        saved_dat_rows,
    };
    write_receipt(&receipt)?;
    Ok(receipt)
}

/// Lets SQLite roll the journal back on the live database, after proving
/// the live files still are the preserved, checked set. Call only on the
/// person's explicit say-so. Never copies a recovered file over the library.
///
/// Safe to call again after an interruption: the journal is not changed by
/// a rollback until it has finished, and replaying it is idempotent.
pub fn apply_journal_recovery(
    database_path: &Path,
    recovery_dir: &Path,
) -> Result<JournalRecoveryReceipt> {
    let _lock = UpgradeLock::acquire_waiting(database_path, LOCK_WAIT)?;
    let mut receipt = read_journal_recovery_receipt(recovery_dir)?;
    if receipt.database_path != database_path || receipt.recovery_dir != recovery_dir {
        return Err(refuse(
            "this recovery was prepared for a different library database or folder. Nothing \
             was changed.",
        ));
    }
    if receipt.state == JournalRecoveryState::Applied {
        return Err(refuse(
            "this recovery has already been applied. Nothing was changed.",
        ));
    }
    let journal_path = sidecar(database_path, "-journal");
    require_recoverable_state(database_path)?;
    let file_name = database_path
        .file_name()
        .ok_or_else(|| refuse("the library database path has no file name"))?;
    let mut journal_name = file_name.to_os_string();
    journal_name.push("-journal");
    let original_dir = recovery_dir.join("original");
    if hash_file(&original_dir.join(file_name))? != receipt.database
        || hash_file(&original_dir.join(&journal_name))? != receipt.journal
    {
        return Err(refuse(
            "the preserved copy in the recovery folder no longer matches its receipt. Prepare \
             a new recovery. Nothing was changed.",
        ));
    }
    if hash_file(&journal_path)? != receipt.journal {
        return Err(refuse(
            "the rollback journal beside the library is not the one this recovery checked. \
             Prepare a new recovery. Nothing was changed.",
        ));
    }
    // Before a first apply the database must be exactly the preserved one.
    // After an interrupted apply it may be part-way through the same
    // rollback; the unchanged journal still restores the same pages.
    if receipt.state == JournalRecoveryState::Prepared
        && hash_file(database_path)? != receipt.database
    {
        return Err(refuse(
            "the library database is not the one this recovery checked. Prepare a new \
             recovery. Nothing was changed.",
        ));
    }
    receipt.state = JournalRecoveryState::Applying;
    write_receipt(&receipt)?;

    sqlite_rollback_and_check(database_path)?;
    if matches!(
        journal_state::rollback_journal_state(database_path)?,
        RollbackJournalState::LiveHeader { .. }
    ) {
        return Err(refuse(format!(
            "SQLite did not roll the journal back on the library database. The preserved \
             originals are in {}.",
            original_dir.display()
        )));
    }
    let now = hash_file(database_path)?;
    if now != receipt.recovered {
        return Err(refuse(format!(
            "SQLite rolled the journal back, but the library database does not match the copy \
             that was checked (expected SHA-256 {}, found {}). Do not use the library until \
             this is understood. The preserved originals are in {}.",
            receipt.recovered.sha256,
            now.sha256,
            original_dir.display()
        )));
    }
    receipt.state = JournalRecoveryState::Applied;
    receipt.applied_at = Some(now_utc_string());
    write_receipt(&receipt)?;
    Ok(receipt)
}
