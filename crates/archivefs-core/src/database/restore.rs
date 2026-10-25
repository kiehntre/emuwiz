//! Explicit, verified library-database restore.
//!
//! This module is intentionally separate from the normal database open path.
//! A restore is never inferred from health, never runs during startup, and
//! never writes the live database until a freshly verified emergency backup
//! exists. The caller must close/stop all database workers before calling the
//! executor and must reload the catalogue afterwards.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use libc::statvfs;
use rusqlite::{Connection, MAIN_DB};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    MIGRATIONS, latest_known_version, open_read_only_connection, verify_database_connection,
};
use crate::{ArchiveFsError, Result};

const CONFIRMATION: &str = "RESTORE DATABASE";
const REQUIRED_TABLES: &[&str] = &[
    "schema_migrations",
    "source_folders",
    "archives",
    "scan_runs",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseRestorePlan {
    pub plan_id: String,
    pub generated_at_unix: u64,
    pub live_database_path: PathBuf,
    pub selected_backup_path: PathBuf,
    pub selected_backup_sha256: String,
    pub backup_schema_version: i64,
    pub expected_live_size_bytes: u64,
    pub expected_live_modified_unix_seconds: Option<i64>,
    pub expected_live_sha256: String,
    pub current_live_schema_version: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseRestoreState {
    Planned,
    Validating,
    EmergencyBackupCreated,
    RestoreStaged,
    RestoreApplied,
    Verification,
    Completed,
    Failed,
    RollbackAttempted,
    RolledBack,
    RollbackFailed,
}

impl DatabaseRestoreState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Planned => "Planned",
            Self::Validating => "Validating",
            Self::EmergencyBackupCreated => "Emergency backup created",
            Self::RestoreStaged => "Restore staged",
            Self::RestoreApplied => "Restore applied",
            Self::Verification => "Verification",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::RollbackAttempted => "Rollback attempted",
            Self::RolledBack => "Rolled back",
            Self::RollbackFailed => "Rollback failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseRestoreReceipt {
    pub schema_version: u32,
    pub operation_id: String,
    pub state: DatabaseRestoreState,
    pub plan: DatabaseRestorePlan,
    pub emergency_backup_path: Option<PathBuf>,
    pub emergency_backup_sha256: Option<String>,
    pub applied_database_sha256: Option<String>,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseRestoreResult {
    pub receipt: DatabaseRestoreReceipt,
    pub emergency_backup_path: PathBuf,
}

/// Captures all mutable facts needed to detect a stale restore plan.
pub fn prepare_database_restore(
    live: impl AsRef<Path>,
    backup: impl AsRef<Path>,
) -> Result<DatabaseRestorePlan> {
    let live = live.as_ref().to_path_buf();
    let backup = backup.as_ref().to_path_buf();
    let live_meta = regular_file_metadata(&live, "current database")?;
    regular_file_metadata(&backup, "selected backup")?;
    let backup_sha = sha256_file(&backup)?;
    let backup_connection = open_read_only_connection(&backup)?;
    let backup_schema = verify_restore_connection(&backup_connection)?;
    let live_connection = open_read_only_connection(&live)?;
    let live_schema = verify_restore_connection(&live_connection)?;
    let live_sha256 = live_hash(&live)?;
    let generated_at = unix_now();
    let plan_id = format!("database-restore-{generated_at}-{}", &backup_sha[..12]);
    Ok(DatabaseRestorePlan {
        plan_id,
        generated_at_unix: generated_at,
        live_database_path: live,
        selected_backup_path: backup,
        selected_backup_sha256: backup_sha,
        backup_schema_version: backup_schema,
        expected_live_size_bytes: live_meta.len(),
        expected_live_modified_unix_seconds: modified_unix(&live_meta),
        expected_live_sha256: live_sha256,
        current_live_schema_version: live_schema,
    })
}

fn live_hash(path: &Path) -> Result<String> {
    sha256_file(path)
}

/// Executes only after the caller has stopped all database workers and the
/// user has supplied the strong confirmation phrase `RESTORE DATABASE`.
pub fn restore_database(
    plan: &DatabaseRestorePlan,
    confirmation: &str,
) -> Result<DatabaseRestoreResult> {
    if confirmation != CONFIRMATION {
        return Err(db(
            "Restore requires the exact confirmation phrase RESTORE DATABASE.",
        ));
    }
    let mut receipt = receipt_for(
        plan,
        DatabaseRestoreState::Validating,
        "Validating restore inputs",
    );
    write_receipt(&receipt)?;
    validate_plan(plan)?;
    ensure_sufficient_space(plan)?;

    let emergency = reserve_sibling(&plan.live_database_path, "before-restore", "backup")?;
    create_verified_backup(
        &plan.live_database_path,
        &emergency,
        plan.current_live_schema_version,
    )?;
    receipt.state = DatabaseRestoreState::EmergencyBackupCreated;
    receipt.emergency_backup_path = Some(emergency.clone());
    receipt.emergency_backup_sha256 = Some(sha256_file(&emergency)?);
    receipt.message = "Mandatory pre-restore safety backup created and verified.".into();
    write_receipt(&receipt)?;

    let staged = reserve_sibling(&plan.live_database_path, "restore-staging", "sqlite3")?;
    if let Err(error) = stage_backup(
        &plan.selected_backup_path,
        &staged,
        plan.backup_schema_version,
    ) {
        let _ = fs::remove_file(&staged);
        receipt.state = DatabaseRestoreState::Failed;
        receipt.message = format!("Restore failed before replacement: {error}");
        write_receipt(&receipt)?;
        return Err(error);
    }
    receipt.state = DatabaseRestoreState::RestoreStaged;
    receipt.message = "Verified replacement is staged beside the live database.".into();
    write_receipt(&receipt)?;

    // A WAL/SHM sidecar belongs to the pathname, not to the inode. Refuse to
    // exchange while one exists: silently carrying it to the restored DB is
    // unsafe. The controlled-close boundary must checkpoint/remove it first.
    reject_sidecars(&plan.live_database_path)?;
    atomic_exchange(&staged, &plan.live_database_path)?;
    receipt.state = DatabaseRestoreState::RestoreApplied;
    receipt.applied_database_sha256 = Some(sha256_file(&plan.live_database_path)?);
    receipt.message = "Verified database replacement applied atomically.".into();
    write_receipt(&receipt)?;

    receipt.state = DatabaseRestoreState::Verification;
    write_receipt(&receipt)?;
    if let Err(error) = validate_live_database(&plan.live_database_path, plan.backup_schema_version)
    {
        receipt.state = DatabaseRestoreState::RollbackAttempted;
        receipt.message = format!(
            "Restore completed but verification failed: {error}. Attempting rollback from the safety backup."
        );
        write_receipt(&receipt)?;
        if rollback_exchange(
            &staged,
            &plan.live_database_path,
            &emergency,
            plan.current_live_schema_version,
        )
        .is_ok()
        {
            receipt.state = DatabaseRestoreState::RolledBack;
            receipt.message = "Restore verification failed; original database was restored from the verified safety backup.".into();
            write_receipt(&receipt)?;
        } else {
            receipt.state = DatabaseRestoreState::RollbackFailed;
            receipt.message = "Restore verification failed and automatic rollback failed; the verified safety backup remains available for review.".into();
            write_receipt(&receipt)?;
        }
        return Err(db(receipt.message.clone()));
    }
    fsync_directory(plan.live_database_path.parent())?;
    let _ = fs::remove_file(&staged);
    receipt.state = DatabaseRestoreState::Completed;
    receipt.message =
        "Database restored from verified backup; catalogue reload/restart is required.".into();
    write_receipt(&receipt)?;
    Ok(DatabaseRestoreResult {
        receipt,
        emergency_backup_path: emergency,
    })
}

pub fn rollback_database_restore(
    receipt: &DatabaseRestoreReceipt,
    confirmation: &str,
) -> Result<DatabaseRestoreReceipt> {
    if confirmation != CONFIRMATION {
        return Err(db(
            "Rollback requires the exact confirmation phrase RESTORE DATABASE.",
        ));
    }
    let emergency = receipt
        .emergency_backup_path
        .as_ref()
        .ok_or_else(|| db("No verified emergency backup is available for rollback."))?;
    let emergency_hash = receipt
        .emergency_backup_sha256
        .as_deref()
        .ok_or_else(|| db("Emergency backup evidence is incomplete; rollback requires review."))?;
    if sha256_file(emergency)? != emergency_hash {
        return Err(db("Emergency backup hash no longer matches."));
    }
    let current_hash = sha256_file(&receipt.plan.live_database_path)?;
    if receipt.applied_database_sha256.as_deref() != Some(current_hash.as_str()) {
        return Err(db(
            "Current database changed since the restore completed; rollback requires review.",
        ));
    }
    validate_database_file(emergency, receipt.plan.current_live_schema_version)?;
    let staged = reserve_sibling(
        &receipt.plan.live_database_path,
        "rollback-staging",
        "sqlite3",
    )?;
    stage_backup(emergency, &staged, receipt.plan.current_live_schema_version)?;
    reject_sidecars(&receipt.plan.live_database_path)?;
    atomic_exchange(&staged, &receipt.plan.live_database_path)?;
    validate_live_database(
        &receipt.plan.live_database_path,
        receipt.plan.current_live_schema_version,
    )?;
    let _ = fs::remove_file(&staged);
    let mut updated = receipt.clone();
    updated.state = DatabaseRestoreState::RolledBack;
    updated.updated_at_unix = unix_now();
    updated.message = "Database rollback completed from the verified emergency backup.".into();
    write_receipt(&updated)?;
    Ok(updated)
}

fn validate_plan(plan: &DatabaseRestorePlan) -> Result<()> {
    regular_file_metadata(&plan.selected_backup_path, "selected backup")?;
    if sha256_file(&plan.selected_backup_path)? != plan.selected_backup_sha256 {
        return Err(db("Backup hash no longer matches."));
    }
    validate_database_file(&plan.selected_backup_path, plan.backup_schema_version)?;
    if plan.current_live_schema_version != latest_known_version(MIGRATIONS) {
        return Err(db(
            "Current database schema is not supported by this build.",
        ));
    }
    validate_plan_live_freshness(plan)
}

fn validate_plan_live_freshness(plan: &DatabaseRestorePlan) -> Result<()> {
    let meta = regular_file_metadata(&plan.live_database_path, "current database")?;
    let modified = modified_unix(&meta);
    if meta.len() != plan.expected_live_size_bytes
        || modified != plan.expected_live_modified_unix_seconds
        || live_hash(&plan.live_database_path)? != plan.expected_live_sha256
    {
        return Err(db(
            "Current database changed since this restore was prepared.",
        ));
    }
    Ok(())
}

fn ensure_sufficient_space(plan: &DatabaseRestorePlan) -> Result<()> {
    let parent = plan
        .live_database_path
        .parent()
        .ok_or_else(|| db("Live database has no parent directory."))?;
    let path = CString::new(parent.as_os_str().as_bytes())
        .map_err(|_| db("Database directory contains an invalid NUL byte."))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(db(format!(
            "Could not determine free space for the database directory: {}",
            std::io::Error::last_os_error()
        )));
    }
    let stats = unsafe { stats.assume_init() };
    let available = u128::from(stats.f_bavail) * u128::from(stats.f_frsize);
    let required = u128::from(plan.expected_live_size_bytes)
        + u128::from(regular_file_metadata(&plan.selected_backup_path, "selected backup")?.len())
        + 1024 * 1024;
    if available < required {
        return Err(db(format!(
            "Insufficient disk space for the mandatory safety backup and staged restore ({} bytes available, {} required).",
            available, required
        )));
    }
    Ok(())
}

fn validate_database_file(path: &Path, expected_schema: i64) -> Result<()> {
    let connection = open_read_only_connection(path)?;
    let actual = verify_restore_connection(&connection)?;
    if actual != expected_schema {
        return Err(db(format!(
            "Backup schema is {actual}, expected {expected_schema}."
        )));
    }
    Ok(())
}

fn validate_live_database(path: &Path, expected_schema: i64) -> Result<()> {
    validate_database_file(path, expected_schema)
}

fn verify_restore_connection(connection: &Connection) -> Result<i64> {
    let schema = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|e| db(format!("Could not read SQLite schema version: {e}")))?;
    if schema != latest_known_version(MIGRATIONS) {
        return Err(db(format!(
            "SQLite schema {schema} is not supported; expected {}.",
            latest_known_version(MIGRATIONS)
        )));
    }
    verify_database_connection(connection, schema)?;
    for table in REQUIRED_TABLES {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(|e| {
                db(format!(
                    "Could not inspect required SQLite table {table}: {e}"
                ))
            })?;
        if !exists {
            return Err(db(format!("Required SQLite table {table} is missing.")));
        }
    }
    Ok(schema)
}

fn stage_backup(source: &Path, destination: &Path, schema: i64) -> Result<()> {
    let source_connection = open_read_only_connection(source)?;
    source_connection
        .backup(MAIN_DB, destination, None)
        .map_err(|e| db(format!("Could not stage verified backup: {e}")))?;
    fsync_file(destination)?;
    validate_database_file(destination, schema)
}

fn create_verified_backup(source: &Path, destination: &Path, schema: i64) -> Result<()> {
    stage_backup(source, destination, schema)
}

fn regular_file_metadata(path: &Path, label: &str) -> Result<fs::Metadata> {
    let metadata = fs::metadata(path).map_err(|e| db(format!("{label} is unavailable: {e}")))?;
    if !metadata.is_file() {
        return Err(db(format!("{label} is not a regular file.")));
    }
    Ok(metadata)
}

fn modified_unix(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).map_err(|e| db(format!("Could not hash {}: {e}", path.display())))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn reserve_sibling(live: &Path, role: &str, extension: &str) -> Result<PathBuf> {
    let parent = live
        .parent()
        .ok_or_else(|| db("Live database has no parent directory."))?;
    let stem = live
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| db("Live database filename is not valid UTF-8."))?;
    for n in 0..1000u32 {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!(".{n}")
        };
        let path = parent.join(format!(
            "{stem}.{role}-{}-{}{}.{}",
            unix_now(),
            std::process::id(),
            suffix,
            extension
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => {
                let _ = fs::remove_file(&path);
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(db(format!("Could not reserve {role} path: {e}"))),
        }
    }
    Err(db(format!("Could not reserve {role} path.")))
}

fn reject_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        if path
            .with_file_name(format!(
                "{}{}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                suffix
            ))
            .exists()
        {
            return Err(db(
                "Database has an active WAL/SHM sidecar; close the database and retry restore.",
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn atomic_exchange(left: &Path, right: &Path) -> Result<()> {
    const AT_FDCWD: i32 = -100;
    const RENAME_EXCHANGE: u32 = 2;
    unsafe extern "C" {
        fn renameat2(
            olddirfd: i32,
            oldpath: *const i8,
            newdirfd: i32,
            newpath: *const i8,
            flags: u32,
        ) -> i32;
    }
    let old = CString::new(left.as_os_str().as_bytes())
        .map_err(|_| db("Staging path contains an invalid NUL byte."))?;
    let new = CString::new(right.as_os_str().as_bytes())
        .map_err(|_| db("Live database path contains an invalid NUL byte."))?;
    let result = unsafe {
        renameat2(
            AT_FDCWD,
            old.as_ptr(),
            AT_FDCWD,
            new.as_ptr(),
            RENAME_EXCHANGE,
        )
    };
    if result != 0 {
        return Err(db(format!(
            "Atomic database exchange failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    fsync_directory(right.parent())
}

#[cfg(not(target_os = "linux"))]
fn atomic_exchange(_left: &Path, _right: &Path) -> Result<()> {
    Err(db(
        "This build cannot provide an atomic database exchange on this platform.",
    ))
}

fn rollback_exchange(
    staged_old_live: &Path,
    live: &Path,
    emergency: &Path,
    schema: i64,
) -> Result<()> {
    // The old live file is still at staged_old_live after RENAME_EXCHANGE.
    // Validate it before exchanging back; the emergency backup is the final
    // independent recovery source if this fails.
    validate_database_file(staged_old_live, schema)?;
    let _ = emergency;
    atomic_exchange(staged_old_live, live)
}

fn fsync_file(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| db(format!("Could not fsync {}: {e}", path.display())))
}
fn fsync_directory(path: Option<&Path>) -> Result<()> {
    if let Some(path) = path {
        File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| db(format!("Could not fsync database directory: {e}")))
    } else {
        Ok(())
    }
}
fn db(message: impl Into<String>) -> ArchiveFsError {
    ArchiveFsError::Database(message.into())
}
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn receipt_for(
    plan: &DatabaseRestorePlan,
    state: DatabaseRestoreState,
    message: impl Into<String>,
) -> DatabaseRestoreReceipt {
    DatabaseRestoreReceipt {
        schema_version: 1,
        operation_id: plan.plan_id.clone(),
        state,
        plan: plan.clone(),
        emergency_backup_path: None,
        emergency_backup_sha256: None,
        applied_database_sha256: None,
        created_at_unix: unix_now(),
        updated_at_unix: unix_now(),
        message: message.into(),
    }
}
fn receipt_path(receipt: &DatabaseRestoreReceipt) -> PathBuf {
    receipt.plan.live_database_path.with_file_name(format!(
        "{}.restore-{}.json",
        receipt
            .plan
            .live_database_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        receipt.operation_id
    ))
}
fn write_receipt(receipt: &DatabaseRestoreReceipt) -> Result<()> {
    let path = receipt_path(receipt);
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|e| db(format!("Could not encode restore receipt: {e}")))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| db(format!("Could not write restore receipt: {e}")))?;
    file.write_all(&bytes)
        .map_err(|e| db(format!("Could not write restore receipt: {e}")))?;
    file.sync_all()
        .map_err(|e| db(format!("Could not fsync restore receipt: {e}")))?;
    fsync_directory(path.parent())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn db(path: &Path, marker: &str) {
        let database = super::super::Database::open_or_create(path).unwrap();
        database
            .connection
            .execute(
                "CREATE TABLE IF NOT EXISTS restore_marker(value TEXT NOT NULL)",
                [],
            )
            .unwrap();
        database
            .connection
            .execute("DELETE FROM restore_marker", [])
            .unwrap();
        database
            .connection
            .execute("INSERT INTO restore_marker(value) VALUES (?1)", [marker])
            .unwrap();
    }
    fn marker(path: &Path) -> String {
        let database = super::super::Database::open_read_only(path).unwrap();
        database
            .connection
            .query_row("SELECT value FROM restore_marker", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn verified_restore_exchanges_and_retains_emergency_backup() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let result = restore_database(&plan, CONFIRMATION).unwrap();
        assert_eq!(result.receipt.state, DatabaseRestoreState::Completed);
        assert_eq!(marker(&live), "backup");
        assert_eq!(marker(&result.emergency_backup_path), "live");
    }

    #[test]
    fn stale_live_database_is_refused_before_emergency_backup() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        db(&live, "changed");
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(error.to_string().contains("changed since"));
        assert!(!root.path().read_dir().unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("before-restore")
        }));
    }

    #[test]
    fn hash_mismatch_is_refused() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let mut plan = prepare_database_restore(&live, &backup).unwrap();
        fs::write(&backup, b"not sqlite").unwrap();
        plan.selected_backup_sha256 = "bad".into();
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(error.to_string().contains("hash"));
    }

    #[test]
    fn rollback_requires_unchanged_restored_database_and_round_trips() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let result = restore_database(&plan, CONFIRMATION).unwrap();
        let rolled_back = rollback_database_restore(&result.receipt, CONFIRMATION).unwrap();
        assert_eq!(rolled_back.state, DatabaseRestoreState::RolledBack);
        assert_eq!(marker(&live), "live");
    }

    #[test]
    fn active_sidecar_blocks_replacement() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        fs::write(live.with_file_name("library.sqlite3-wal"), b"active").unwrap();
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(error.to_string().contains("WAL/SHM"));
        assert_eq!(marker(&live), "live");
    }
}
