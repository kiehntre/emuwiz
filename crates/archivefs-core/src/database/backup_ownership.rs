//! Ownership, capacity and bounded retention for pre-upgrade database backups.
//!
//! The online-backup mechanism itself is unchanged (`upgrade_library_database`).
//! This module adds only the evidence around it:
//!
//! * a **receipt** beside every backup that records who created it, for which
//!   database identity and schema transition, and (once complete) the exact
//!   length and SHA-256 of the verified snapshot;
//! * a **capacity** check before any file is created, and a **cap** on how many
//!   retained backups may accumulate beside one database;
//! * an exclusive **upgrade lock** so two upgrades cannot interleave;
//! * a read-only **inspection** that classifies existing backups without ever
//!   granting them authority.
//!
//! Authority is deliberately not transferable. Creating a backup, inspecting it,
//! migrating a database and restoring one are separate acts: a backup that
//! inspects as `Verified` is evidence only. Nothing here reuses, adopts,
//! replaces, restores or deletes an existing backup, and a restart is never
//! taken as proof that an earlier backup completed.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ArchiveFsError, Result};

/// At most this many recognised upgrade backups may be retained beside one
/// database. Reaching it refuses a new upgrade; nothing is deleted for the user.
pub const MAX_RETAINED_UPGRADE_BACKUPS: usize = 8;
/// Free space that must remain after the snapshot has been written.
pub const BACKUP_CAPACITY_HEADROOM_BYTES: u64 = 16 * 1024 * 1024;
/// Sidecar suffix appended to the backup file name.
pub const BACKUP_RECEIPT_SUFFIX: &str = ".receipt.json";
const RECEIPT_FORMAT: u32 = 1;
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
const MAX_SEQUENCE: u16 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptState {
    InProgress,
    Complete,
}

/// What a receipt claims. Hex fields keep non-UTF-8 file names exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BackupReceipt {
    format: u32,
    state: ReceiptState,
    database_file_name_hex: String,
    backup_file_name_hex: String,
    from_version: i64,
    to_version: i64,
    source_device: u64,
    source_inode: u64,
    source_len: u64,
    created_unix_seconds: i64,
    backup_len: Option<u64>,
    backup_sha256: Option<String>,
}

/// Classification of an existing backup. Inspection is read-only and grants no
/// authority to reuse, replace or restore it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeBackupState {
    /// A complete receipt matches the file; length, SHA-256, schema and
    /// SQLite integrity were all re-verified through freshly opened handles.
    Verified {
        from_version: i64,
        to_version: i64,
        len: u64,
        sha256: String,
    },
    /// Created by this program but never completed (for example interrupted).
    /// Not usable and not adopted; it still counts toward the retention cap.
    Incomplete,
    /// No receipt: provenance unknown. Never adopted or trusted.
    Unowned,
    /// A receipt exists but contradicts the file or an object is unsafe
    /// (symlink, hardlink, wrong name, hash or length mismatch, corruption).
    Inconsistent(String),
    /// The material cannot be read by this process, so recovery accessibility
    /// is not established.
    Inaccessible(String),
}

/// One recognised backup and its current classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeBackupInspection {
    pub backup_path: PathBuf,
    pub state: UpgradeBackupState,
}

fn db_error(message: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Database(message.into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn receipt_path_for(backup_path: &Path) -> PathBuf {
    let mut name = backup_path.as_os_str().to_os_string();
    name.push(BACKUP_RECEIPT_SUFFIX);
    PathBuf::from(name)
}

fn parent_of(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn file_name_of(path: &Path) -> Result<&OsStr> {
    path.file_name().ok_or_else(|| {
        db_error(format!(
            "could not derive a backup name for {}. Nothing was changed.",
            path.display()
        ))
    })
}

/// The database file identity a backup is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceIdentity {
    pub device: u64,
    pub inode: u64,
    pub len: u64,
}

impl SourceIdentity {
    /// Captures the identity of a regular, non-symlink database file.
    pub(super) fn capture(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            db_error(format!(
                "could not inspect the library database {}. Nothing was changed: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(db_error(format!(
                "Library database {} is not a regular file (symlinks and special files are \
                 refused). Nothing was changed.",
                path.display()
            )));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            len: metadata.len(),
        })
    }

    /// Refuses when the path no longer names the file that was backed up.
    pub(super) fn recheck(&self, path: &Path) -> Result<()> {
        let now = Self::capture(path)?;
        if now.device != self.device || now.inode != self.inode {
            return Err(db_error(format!(
                "the library database at {} was replaced after its backup was taken (file \
                 identity changed); refusing to migrate it",
                path.display()
            )));
        }
        Ok(())
    }
}

/// Held for the whole upgrade; released when dropped.
pub(super) struct UpgradeLock {
    _file: File,
}

impl UpgradeLock {
    pub(super) fn acquire(database_path: &Path) -> Result<Self> {
        let mut name = file_name_of(database_path)?.to_os_string();
        name.push(".upgrade.lock");
        let lock_path = parent_of(database_path).join(name);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|error| {
                db_error(format!(
                    "could not open the upgrade lock {}. The library database was not changed: \
                     {error}",
                    lock_path.display()
                ))
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(fs::TryLockError::WouldBlock) => Err(db_error(format!(
                "another upgrade of {} is already running; refusing to start a second one. \
                 Nothing was changed.",
                database_path.display()
            ))),
            Err(fs::TryLockError::Error(error)) => Err(db_error(format!(
                "could not take the upgrade lock {}. The library database was not changed: {error}",
                lock_path.display()
            ))),
        }
    }
}

fn available_bytes(directory: &Path) -> Result<u64> {
    use std::ffi::CString;
    let c_path = CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| db_error("backup directory path contains a NUL byte"))?;
    // SAFETY: `c_path` is a valid NUL-terminated string and `stat` is a plain
    // zeroed out-parameter that statvfs fully initialises on success.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return Err(db_error(format!(
            "could not determine free space beside {}. Nothing was changed: {}",
            directory.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

fn recognised_backup_name(database_name: &OsStr, candidate: &OsStr) -> bool {
    let db = database_name.as_bytes();
    let name = candidate.as_bytes();
    // <db>.schema-<from>-before-<to>.backup[.<sequence>]
    let Some(rest) = name
        .strip_prefix(db)
        .and_then(|rest| rest.strip_prefix(b".schema-"))
    else {
        return false;
    };
    let Some(split) = rest.windows(7).position(|window| window == b".backup") else {
        return false;
    };
    let (versions, tail) = rest.split_at(split);
    let Ok(versions) = std::str::from_utf8(versions) else {
        return false;
    };
    let Some((from, to)) = versions.split_once("-before-") else {
        return false;
    };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(from) || !digits(to) {
        return false;
    }
    match tail {
        b".backup" => true,
        other => other
            .strip_prefix(b".backup.")
            .is_some_and(|seq| !seq.is_empty() && seq.iter().all(u8::is_ascii_digit)),
    }
}

fn recognised_backups(database_path: &Path) -> Result<Vec<PathBuf>> {
    let database_name = file_name_of(database_path)?;
    let parent = parent_of(database_path);
    let mut found = Vec::new();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) => {
            return Err(db_error(format!(
                "could not list {} to count retained backups. Nothing was changed: {error}",
                parent.display()
            )));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            db_error(format!(
                "could not list {} to count retained backups. Nothing was changed: {error}",
                parent.display()
            ))
        })?;
        if recognised_backup_name(database_name, &entry.file_name()) {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// Refuses before any file is created when the retention cap is reached or the
/// volume cannot hold the snapshot with headroom.
pub(super) fn preflight(database_path: &Path, source_bytes: u64) -> Result<()> {
    let retained = recognised_backups(database_path)?.len();
    if retained >= MAX_RETAINED_UPGRADE_BACKUPS {
        return Err(db_error(format!(
            "{retained} upgrade backups are already retained beside {} (limit \
             {MAX_RETAINED_UPGRADE_BACKUPS}). Review them with the backup inspection and remove \
             old ones yourself; EmuWiz never deletes a backup. Nothing was changed.",
            database_path.display()
        )));
    }
    let free = available_bytes(parent_of(database_path))?;
    let needed = source_bytes.saturating_add(BACKUP_CAPACITY_HEADROOM_BYTES);
    if free < needed {
        return Err(db_error(format!(
            "not enough free space for a verified pre-upgrade backup beside {}: {free} bytes \
             available, {needed} needed (database plus {BACKUP_CAPACITY_HEADROOM_BYTES} bytes of \
             headroom). Nothing was changed.",
            database_path.display()
        )));
    }
    Ok(())
}

fn fsync_directory(directory: &Path) {
    if let Ok(handle) = File::open(directory) {
        let _ = handle.sync_all();
    }
}

fn write_json_new(path: &Path, receipt: &BackupReceipt) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)?;
    let bytes = serde_json::to_vec_pretty(receipt).map_err(std::io::Error::other)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

/// A backup name and receipt claimed by this call; both were created by us and
/// are the only files this type may ever remove.
pub(super) struct BackupReservation {
    pub backup_path: PathBuf,
    receipt_path: PathBuf,
    receipt: BackupReceipt,
    database_path: PathBuf,
    finished: bool,
}

impl BackupReservation {
    /// Claims `<db>.schema-<from>-before-<to>.backup[.<n>]` and its receipt.
    pub(super) fn reserve(
        database_path: &Path,
        from_version: i64,
        to_version: i64,
        identity: &SourceIdentity,
    ) -> Result<Self> {
        let parent = parent_of(database_path);
        let file_name = file_name_of(database_path)?;
        for sequence in 0..MAX_SEQUENCE {
            let mut backup_name: OsString = file_name.to_os_string();
            backup_name.push(format!(".schema-{from_version}-before-{to_version}.backup"));
            if sequence > 0 {
                backup_name.push(format!(".{sequence}"));
            }
            let candidate = parent.join(&backup_name);
            let receipt_path = receipt_path_for(&candidate);
            // A leftover receipt without its backup is unexplained evidence:
            // skip the name instead of overwriting or adopting it.
            if fs::symlink_metadata(&receipt_path).is_ok() {
                continue;
            }
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(0o600)
                .open(&candidate)
            {
                Ok(file) => drop(file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(db_error(format!(
                        "Could not reserve a pre-upgrade database backup beside {}. The library \
                         database was not changed: {error}",
                        database_path.display()
                    )));
                }
            }
            let receipt = BackupReceipt {
                format: RECEIPT_FORMAT,
                state: ReceiptState::InProgress,
                database_file_name_hex: hex(file_name.as_bytes()),
                backup_file_name_hex: hex(backup_name.as_bytes()),
                from_version,
                to_version,
                source_device: identity.device,
                source_inode: identity.inode,
                source_len: identity.len,
                created_unix_seconds: now_unix(),
                backup_len: None,
                backup_sha256: None,
            };
            match write_json_new(&receipt_path, &receipt) {
                Ok(()) => {
                    fsync_directory(parent);
                    return Ok(Self {
                        backup_path: candidate,
                        receipt_path,
                        receipt,
                        database_path: database_path.to_path_buf(),
                        finished: false,
                    });
                }
                Err(error) => {
                    let _ = fs::remove_file(&candidate);
                    let _ = fs::remove_file(&receipt_path);
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        continue;
                    }
                    return Err(db_error(format!(
                        "Could not record ownership for the pre-upgrade backup beside {}. The \
                         library database was not changed: {error}",
                        database_path.display()
                    )));
                }
            }
        }
        Err(db_error(format!(
            "Could not choose an unused pre-upgrade backup name beside {}. The library database \
             was not changed.",
            database_path.display()
        )))
    }

    /// Publishes the complete receipt atomically, then re-inspects the pair
    /// through fresh handles. The backup gains no authority unless it inspects
    /// as `Verified`.
    pub(super) fn complete(&mut self, backup_len: u64, backup_sha256: &str) -> Result<()> {
        let mut complete = self.receipt.clone();
        complete.state = ReceiptState::Complete;
        complete.backup_len = Some(backup_len);
        complete.backup_sha256 = Some(backup_sha256.to_string());
        let mut temp_name = self.receipt_path.as_os_str().to_os_string();
        temp_name.push(".tmp");
        let temp_path = PathBuf::from(temp_name);
        let publish = (|| -> std::io::Result<()> {
            write_json_new(&temp_path, &complete)?;
            fs::rename(&temp_path, &self.receipt_path)?;
            fsync_directory(parent_of(&self.receipt_path));
            Ok(())
        })();
        if let Err(error) = publish {
            let _ = fs::remove_file(&temp_path);
            return Err(db_error(format!(
                "could not publish the ownership receipt for the pre-upgrade backup: {error}"
            )));
        }
        self.receipt = complete;
        match inspect_upgrade_backup(&self.database_path, &self.backup_path) {
            UpgradeBackupState::Verified { .. } => {
                self.finished = true;
                Ok(())
            }
            other => Err(db_error(format!(
                "the pre-upgrade backup did not re-verify through a fresh open ({other:?}); \
                 refusing to migrate"
            ))),
        }
    }

    /// Removes only the files this reservation created (used on failure before
    /// the backup earned authority).
    pub(super) fn discard(&mut self) {
        let mut temp_name = self.receipt_path.as_os_str().to_os_string();
        temp_name.push(".tmp");
        let _ = fs::remove_file(PathBuf::from(temp_name));
        let _ = fs::remove_file(&self.receipt_path);
        let _ = fs::remove_file(&self.backup_path);
        self.finished = true;
    }
}

/// Opens `path` without following a final symlink and requires an ordinary,
/// single-link regular file; returns the handle and its metadata.
fn open_regular(path: &Path) -> std::result::Result<(File, fs::Metadata), UpgradeBackupState> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(UpgradeBackupState::Inaccessible(format!(
                "{}: {error}",
                path.display()
            )));
        }
        Err(error) => {
            return Err(UpgradeBackupState::Inconsistent(format!(
                "{} could not be opened safely (symlink or substitution?): {error}",
                path.display()
            )));
        }
    };
    let metadata = file.metadata().map_err(|error| {
        UpgradeBackupState::Inaccessible(format!("{}: {error}", path.display()))
    })?;
    if !metadata.is_file() {
        return Err(UpgradeBackupState::Inconsistent(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.nlink() != 1 {
        return Err(UpgradeBackupState::Inconsistent(format!(
            "{} has {} links; a retained backup must be a single-link file",
            path.display(),
            metadata.nlink()
        )));
    }
    Ok((file, metadata))
}

/// Classifies one backup. Read-only; never changes or adopts anything.
pub(super) fn inspect_upgrade_backup(
    database_path: &Path,
    backup_path: &Path,
) -> UpgradeBackupState {
    let receipt_path = receipt_path_for(backup_path);
    let receipt_present = match fs::symlink_metadata(&receipt_path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return UpgradeBackupState::Inaccessible(format!(
                "{}: {error}",
                receipt_path.display()
            ));
        }
    };
    let (mut backup_file, backup_meta) = match open_regular(backup_path) {
        Ok(opened) => opened,
        Err(state) => return state,
    };
    if !receipt_present {
        return UpgradeBackupState::Unowned;
    }
    let (mut receipt_file, receipt_meta) = match open_regular(&receipt_path) {
        Ok(opened) => opened,
        Err(state) => return state,
    };
    if receipt_meta.len() > MAX_RECEIPT_BYTES {
        return UpgradeBackupState::Inconsistent("receipt is implausibly large".to_string());
    }
    let mut receipt_bytes = Vec::new();
    if let Err(error) = receipt_file.read_to_end(&mut receipt_bytes) {
        return UpgradeBackupState::Inaccessible(format!("{}: {error}", receipt_path.display()));
    }
    let receipt: BackupReceipt = match serde_json::from_slice(&receipt_bytes) {
        Ok(receipt) => receipt,
        Err(error) => {
            return UpgradeBackupState::Inconsistent(format!("receipt is not valid: {error}"));
        }
    };
    let (Some(database_name), Some(backup_name)) =
        (database_path.file_name(), backup_path.file_name())
    else {
        return UpgradeBackupState::Inconsistent("path has no file name".to_string());
    };
    if receipt.format != RECEIPT_FORMAT {
        return UpgradeBackupState::Inconsistent(format!(
            "unsupported receipt format {}",
            receipt.format
        ));
    }
    if receipt.database_file_name_hex != hex(database_name.as_bytes())
        || receipt.backup_file_name_hex != hex(backup_name.as_bytes())
    {
        return UpgradeBackupState::Inconsistent(
            "receipt names a different database or backup file".to_string(),
        );
    }
    if receipt.state == ReceiptState::InProgress {
        return UpgradeBackupState::Incomplete;
    }
    let (Some(expected_len), Some(expected_sha)) =
        (receipt.backup_len, receipt.backup_sha256.clone())
    else {
        return UpgradeBackupState::Inconsistent(
            "complete receipt lacks length or digest".to_string(),
        );
    };
    if backup_meta.len() != expected_len {
        return UpgradeBackupState::Inconsistent(format!(
            "backup length {} differs from the receipt ({expected_len})",
            backup_meta.len()
        ));
    }
    // Hash through the handle opened above: the bytes we hash are the object we
    // validated, not a later path lookup.
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        match backup_file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => digest.update(&buffer[..read]),
            Err(error) => {
                return UpgradeBackupState::Inaccessible(format!(
                    "{}: {error}",
                    backup_path.display()
                ));
            }
        }
    }
    let actual_sha = hex(&digest.finalize());
    if actual_sha != expected_sha {
        return UpgradeBackupState::Inconsistent(
            "backup digest differs from the receipt".to_string(),
        );
    }
    match super::verify_database_backup(backup_path, receipt.from_version) {
        Ok(()) => UpgradeBackupState::Verified {
            from_version: receipt.from_version,
            to_version: receipt.to_version,
            len: expected_len,
            sha256: expected_sha,
        },
        Err(error) => {
            UpgradeBackupState::Inconsistent(format!("backup failed SQLite verification: {error}"))
        }
    }
}

/// Read-only listing of the recognised upgrade backups beside a database.
/// Nothing is created, adopted, reused, restored or removed.
pub fn inspect_upgrade_backups(database_path: &Path) -> Result<Vec<UpgradeBackupInspection>> {
    Ok(recognised_backups(database_path)?
        .into_iter()
        .map(|backup_path| UpgradeBackupInspection {
            state: inspect_upgrade_backup(database_path, &backup_path),
            backup_path,
        })
        .collect())
}
