//! Explicit, verified library-database restore.
//!
//! This module is intentionally separate from the normal database open path.
//! A restore is never inferred from health, never runs during startup, and
//! never writes the live database until a freshly verified emergency backup
//! exists.
//!
//! Safety model (see `docs/DATABASE_RESTORE.md`):
//!
//! * **Writer exclusion.** Before anything is created or changed the executor
//!   takes the database upgrade lock (so no other EmuWiz process can open,
//!   create or upgrade the database) and a SQLite `BEGIN IMMEDIATE`
//!   transaction on the live file (the RESERVED lock, so no connection in any
//!   process can begin or commit a write). Both are held until the restore has
//!   either completed or been rolled back. A writer that already holds a
//!   transaction makes the restore refuse; it never waits for it to finish and
//!   never loses an acknowledged commit. Readers are tolerated: they keep
//!   reading the inode they opened and the GUI reloads afterwards.
//! * **Sidecars.** `-wal`/`-shm` (foreign to the DELETE-journal catalogue) and
//!   a rollback journal with a live header refuse the restore. A live journal
//!   whose writer still holds SQLite's write lock is reported as "busy"; with
//!   no visible lock the verdict is "activity cannot be determined" (never
//!   "crashed"), and the journal is left untouched for copy-first recovery.
//! * **Staged upgrade.** A backup at an older schema is copied with SQLite's
//!   online-backup API into a staging file beside the live database and
//!   migrated there. The selected backup is only ever opened read-only.
//! * **Durable recovery record.** Every receipt is written atomically
//!   (temporary file, fsync, rename, directory fsync) and names the staging
//!   file, its hash and the emergency backup before the exchange happens, so
//!   an interruption at any point can be classified after restart by
//!   [`assess_interrupted_restore`].
//! * **Fail safe.** Every failure before the exchange leaves the original
//!   untouched; every failure after it keeps the original reachable (first at
//!   the staging path, always in the verified emergency backup).

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use libc::statvfs;
use rusqlite::{Connection, MAIN_DB};
use serde::{Deserialize, Serialize};

use super::{
    MIGRATIONS, apply_migrations, backup_ownership, journal_state, latest_known_version,
    open_connection_with, open_read_only_connection, pending_migration_versions, schema_version,
    sha256_file, verify_database_connection,
};
use crate::{ArchiveFsError, Result};

const CONFIRMATION: &str = "RESTORE DATABASE";
const REQUIRED_TABLES: &[&str] = &[
    "schema_migrations",
    "source_folders",
    "archives",
    "scan_runs",
];
/// How long the executor waits for a writer that is between transactions.
/// A writer holding an open transaction makes the restore refuse; it is never
/// waited out, so this only absorbs the gap between two short writes.
const WRITER_WAIT: Duration = Duration::from_secs(2);

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
    /// Migrations the staged copy of the backup must apply before it can
    /// replace the live database. Empty when the backup is already current.
    #[serde(default)]
    pub pending_migrations: Vec<i64>,
}

impl DatabaseRestorePlan {
    /// Whether the selected backup is older than this build's schema and will
    /// be upgraded in its staging copy.
    pub fn requires_upgrade(&self) -> bool {
        !self.pending_migrations.is_empty()
    }
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

    /// Whether the receipt describes a finished operation. Anything else was
    /// interrupted and needs [`assess_interrupted_restore`].
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::RolledBack | Self::RollbackFailed
        )
    }
}

/// Device and inode of a regular file: the identity of the bytes, as opposed
/// to the pathname that currently points at them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
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
    /// Where the verified replacement is staged before the exchange. After
    /// the exchange the superseded original sits at this path until it is
    /// removed (see `superseded_database_removed`).
    #[serde(default)]
    pub staged_replacement_path: Option<PathBuf>,
    /// SHA-256 of the staged replacement, recorded before the exchange so an
    /// interrupted restore can tell whether the exchange happened.
    #[serde(default)]
    pub staged_replacement_sha256: Option<String>,
    /// Identity of the live database while the exclusion was held.
    #[serde(default)]
    pub original_identity: Option<FileIdentity>,
    /// Identity of the staged replacement before the exchange.
    #[serde(default)]
    pub replacement_identity: Option<FileIdentity>,
    /// Set once the exchange has happened: the original's current pathname.
    #[serde(default)]
    pub superseded_database_path: Option<PathBuf>,
    /// True once the superseded original was unlinked after a verified
    /// restore (its content remains in the emergency backup).
    #[serde(default)]
    pub superseded_database_removed: bool,
    /// `Some(true)` when the operation ended with the original database in
    /// place and unchanged; `Some(false)` when the live database is the
    /// restored replacement; `None` when the receipt was not finalised.
    #[serde(default)]
    pub original_intact: Option<bool>,
    /// True when the upgrade lock and the SQLite RESERVED lock were held for
    /// the whole operation (always, for receipts written by this build).
    #[serde(default)]
    pub exclusion_held: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseRestoreResult {
    pub receipt: DatabaseRestoreReceipt,
    pub emergency_backup_path: PathBuf,
}

/// Deterministic fault-injection seam for tests. The hook runs at every named
/// transition of a restore; returning `Err` makes that step fail through the
/// normal error path, panicking inside it models a crash (the exclusion and
/// every open handle are dropped by unwinding, exactly as the kernel releases
/// them when a process dies).
#[cfg(any(test, feature = "test-support"))]
pub mod fault_injection {
    use std::cell::RefCell;

    type Hook = Box<dyn Fn(&str) -> std::result::Result<(), String>>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// Installs `hook` for the current thread until the guard drops.
    pub fn install(hook: impl Fn(&str) -> std::result::Result<(), String> + 'static) -> Guard {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        Guard
    }

    pub struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            HOOK.with(|slot| *slot.borrow_mut() = None);
        }
    }

    pub(super) fn fire(stage: &str) -> std::result::Result<(), String> {
        HOOK.with(|slot| match slot.borrow().as_ref() {
            Some(hook) => hook(stage),
            None => Ok(()),
        })
    }

    pub use super::{
        STAGE_AFTER_EMERGENCY_BACKUP, STAGE_AFTER_EXCHANGE, STAGE_AFTER_EXCLUSION,
        STAGE_AFTER_STAGING, STAGE_AFTER_VERIFICATION, STAGE_BEFORE_COMPLETED_RECEIPT,
        STAGE_BEFORE_EXCHANGE, STAGE_BEFORE_SUPERSEDED_REMOVAL, STAGE_BEFORE_VERIFICATION,
    };

    /// Every stage name, in execution order, for tests that sweep them.
    pub const STAGES: &[&str] = &[
        super::STAGE_AFTER_EXCLUSION,
        super::STAGE_AFTER_EMERGENCY_BACKUP,
        super::STAGE_AFTER_STAGING,
        super::STAGE_BEFORE_EXCHANGE,
        super::STAGE_AFTER_EXCHANGE,
        super::STAGE_BEFORE_VERIFICATION,
        super::STAGE_AFTER_VERIFICATION,
        super::STAGE_BEFORE_SUPERSEDED_REMOVAL,
        super::STAGE_BEFORE_COMPLETED_RECEIPT,
    ];
}

pub const STAGE_AFTER_EXCLUSION: &str = "after-exclusion";
pub const STAGE_AFTER_EMERGENCY_BACKUP: &str = "after-emergency-backup";
pub const STAGE_AFTER_STAGING: &str = "after-staging";
pub const STAGE_BEFORE_EXCHANGE: &str = "before-exchange";
pub const STAGE_AFTER_EXCHANGE: &str = "after-exchange";
pub const STAGE_BEFORE_VERIFICATION: &str = "before-verification";
pub const STAGE_AFTER_VERIFICATION: &str = "after-verification";
pub const STAGE_BEFORE_SUPERSEDED_REMOVAL: &str = "before-superseded-removal";
pub const STAGE_BEFORE_COMPLETED_RECEIPT: &str = "before-completed-receipt";

#[cfg(any(test, feature = "test-support"))]
fn restore_stage(stage: &str) -> Result<()> {
    fault_injection::fire(stage)
        .map_err(|message| db(format!("Injected fault at {stage}: {message}")))
}

#[cfg(not(any(test, feature = "test-support")))]
#[inline(always)]
fn restore_stage(_stage: &str) -> Result<()> {
    Ok(())
}

/// Captures all mutable facts needed to detect a stale restore plan.
///
/// The backup may be at any schema this build can migrate from (1 through
/// the current schema); the live database must be current.
pub fn prepare_database_restore(
    live: impl AsRef<Path>,
    backup: impl AsRef<Path>,
) -> Result<DatabaseRestorePlan> {
    let live = live.as_ref().to_path_buf();
    let backup = backup.as_ref().to_path_buf();
    refuse_if_this_process_uses(&live, "Restore review")?;
    let live_meta = regular_file_metadata(&live, "current database")?;
    regular_file_metadata(&backup, "selected backup")?;
    if same_file(&live, &backup)? {
        return Err(db(
            "The selected backup is the live database itself; choose a backup file.",
        ));
    }
    let backup_sha = sha256_file(&backup)?;
    let backup_schema = {
        let connection = open_read_only_connection(&backup)?;
        verify_backup_connection(&connection)?
    };
    let live_schema = {
        let connection = open_read_only_connection(&live)?;
        verify_current_connection(&connection)?
    };
    let live_sha256 = sha256_file(&live)?;
    let generated_at = unix_now();
    // Unique per process and per moment: two reviews of the same backup in
    // the same second (or in two processes) must never share a receipt.
    let plan_id = format!(
        "database-restore-{generated_at}-{:06}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_micros()),
        std::process::id(),
        &backup_sha[..12]
    );
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
        pending_migrations: pending_migration_versions(MIGRATIONS, backup_schema),
    })
}

/// Holds every lock a restore needs, for as long as it is alive.
///
/// * the database upgrade lock (`<db>.upgrade.lock`, `flock`): no other
///   EmuWiz process can open, create, upgrade or restore the database;
/// * a `BEGIN IMMEDIATE` transaction on the live file: SQLite's RESERVED lock,
///   so no connection anywhere can begin or commit a write, while readers keep
///   working. Nothing is ever written through this transaction, so it creates
///   no rollback journal of its own.
///
/// Dropping it ends the transaction (a no-op rollback) and releases the lock.
struct RestoreExclusion {
    _upgrade_lock: backup_ownership::UpgradeLock,
    _connection: Connection,
    /// A read handle on the live inode, opened before the SQLite lock and
    /// kept open for the whole exclusion. All hashing and metadata reads of
    /// the live database go through it: on POSIX, closing *any* descriptor
    /// this process has on the file would drop the SQLite lock above.
    live: File,
    identity: FileIdentity,
}

impl RestoreExclusion {
    fn acquire(live: &Path) -> Result<Self> {
        let upgrade_lock = backup_ownership::UpgradeLock::acquire(live).map_err(|error| {
            db(format!(
                "Restore refused: {error} (another process is upgrading, creating or restoring \
                 this database). Nothing was changed."
            ))
        })?;
        // A worker in this process holding a SQLite lock would lose it the
        // moment any descriptor opened below is closed; refuse first.
        refuse_if_this_process_uses(live, "Restore")?;
        let identity_before = file_identity(live, "current database")?;
        // Sidecars are judged before SQLite may touch the file: a hot journal
        // must never be rolled back silently, and a WAL never carried across.
        require_no_wal_sidecars(live)?;
        journal_state::require_inert_rollback_journal(live)
            .map_err(|error| db(format!("Restore refused: {error}")))?;
        let live_handle = open_live_handle(live)?;
        let connection = open_connection_with(live, false)
            .map_err(|error| db(format!("Restore refused: {error}. Nothing was changed.")))?;
        connection
            .busy_timeout(WRITER_WAIT)
            .map_err(|error| db(format!("Could not configure the exclusion wait: {error}")))?;
        if let Err(error) = connection.execute_batch("BEGIN IMMEDIATE") {
            return Err(db(format!(
                "Restore refused: another EmuWiz worker or process is writing the library \
                 database ({error}). Wait for that scan or action to finish and try again. \
                 Nothing was changed."
            )));
        }
        // From here on no writer can start or commit. The file must still be
        // the one that was inspected, and no journal may be left beside it.
        let identity = file_identity(live, "current database")?;
        if identity != identity_before {
            return Err(db(
                "Restore refused: the live database was replaced while the exclusion was being \
                 taken. Nothing was changed.",
            ));
        }
        require_no_sidecars_at_all(live)?;
        Ok(Self {
            _upgrade_lock: upgrade_lock,
            _connection: connection,
            live: live_handle,
            identity,
        })
    }

    /// SHA-256 of the live inode through the held handle (no open, no close).
    fn live_sha256(&self) -> Result<String> {
        sha256_of_handle(&self.live)
    }

    fn live_metadata(&self) -> Result<fs::Metadata> {
        self.live
            .metadata()
            .map_err(|error| db(format!("Could not inspect the current database: {error}")))
    }
}

fn open_live_handle(live: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(live)
        .map_err(|error| {
            db(format!(
                "Could not open the current database for reading: {error}"
            ))
        })
}

fn sha256_of_handle(file: &File) -> Result<String> {
    use sha2::Digest;
    use std::os::unix::fs::FileExt;
    let mut digest = sha2::Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut offset = 0_u64;
    loop {
        let read = file
            .read_at(&mut buffer, offset)
            .map_err(|error| db(format!("Could not hash the current database: {error}")))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        offset += read as u64;
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Plain file I/O on the live database (hash, fsync) is only safe while no
/// SQLite connection of this process holds a lock on it: closing the
/// descriptor afterwards would release that lock. Callers that hold the
/// exclusion read through its handle instead and never hit this.
fn refuse_if_this_process_uses(live: &Path, operation: &str) -> Result<()> {
    if journal_state::own_process_holds_lock(live)? {
        return Err(db(format!(
            "{operation} refused: a worker in this EmuWiz window is using the library database \
             right now. Wait for it to finish and try again. Nothing was changed."
        )));
    }
    Ok(())
}

/// Executes only after the user has supplied the strong confirmation phrase
/// `RESTORE DATABASE`. The caller should stop its own database workers first
/// (so the restore is not refused as busy) and must reload the catalogue
/// afterwards.
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

    // Everything up to the exchange is guarded by `refuse`: the receipt is
    // finalised as Failed with the original untouched.
    let exclusion = match RestoreExclusion::acquire(&plan.live_database_path) {
        Ok(exclusion) => exclusion,
        Err(error) => return Err(refuse(&mut receipt, error)),
    };
    receipt.exclusion_held = true;
    receipt.original_identity = Some(exclusion.identity);
    if let Err(error) = restore_stage(STAGE_AFTER_EXCLUSION)
        .and_then(|()| validate_plan(plan, &exclusion))
        .and_then(|()| ensure_sufficient_space(plan))
    {
        return Err(refuse(&mut receipt, error));
    }

    let emergency = match reserve_sibling(&plan.live_database_path, "before-restore", "backup") {
        Ok(path) => path,
        Err(error) => return Err(refuse(&mut receipt, error)),
    };
    let staged = match reserve_sibling(&plan.live_database_path, "restore-staging", "sqlite3") {
        Ok(path) => path,
        Err(error) => return Err(refuse(&mut receipt, error)),
    };
    // Both names are durable before either file exists, so leftovers of an
    // interrupted run are always attributable.
    receipt.emergency_backup_path = Some(emergency.clone());
    receipt.staged_replacement_path = Some(staged.clone());
    receipt.message =
        "Restore inputs validated under exclusion; creating the safety backup.".into();
    if let Err(error) = write_receipt(&receipt) {
        return Err(refuse(&mut receipt, error));
    }

    if let Err(error) = create_verified_backup(
        &plan.live_database_path,
        &emergency,
        plan.current_live_schema_version,
    )
    .and_then(|()| sha256_file(&emergency))
    .and_then(|sha| {
        // The exclusion guarantees nothing was committed since the plan's
        // hash was re-checked, so the backup is exactly the reviewed state.
        receipt.emergency_backup_sha256 = Some(sha);
        receipt.state = DatabaseRestoreState::EmergencyBackupCreated;
        receipt.message = "Mandatory pre-restore safety backup created and verified.".into();
        write_receipt(&receipt)
    })
    .and_then(|()| restore_stage(STAGE_AFTER_EMERGENCY_BACKUP))
    {
        let _ = fs::remove_file(&emergency);
        receipt.emergency_backup_path = None;
        receipt.emergency_backup_sha256 = None;
        return Err(refuse(&mut receipt, error));
    }

    let staging = stage_replacement(plan, &staged)
        .and_then(|schema| {
            let sha = sha256_file(&staged)?;
            let identity = file_identity(&staged, "staged replacement")?;
            Ok((schema, sha, identity))
        })
        .and_then(|(schema, sha, identity)| {
            receipt.staged_replacement_sha256 = Some(sha);
            receipt.replacement_identity = Some(identity);
            receipt.state = DatabaseRestoreState::RestoreStaged;
            receipt.message = if plan.requires_upgrade() {
                format!(
                    "Verified replacement staged beside the live database and upgraded from schema {} to {schema}.",
                    plan.backup_schema_version
                )
            } else {
                "Verified replacement is staged beside the live database.".into()
            };
            write_receipt(&receipt)
        })
        .and_then(|()| restore_stage(STAGE_AFTER_STAGING))
        // The exclusion makes a new sidecar impossible, but the exchange must
        // never carry one across; re-check with the lock held.
        .and_then(|()| require_no_sidecars_at_all(&plan.live_database_path))
        .and_then(|()| require_no_sidecars_at_all(&staged))
        .and_then(|()| restore_stage(STAGE_BEFORE_EXCHANGE));
    if let Err(error) = staging {
        let _ = fs::remove_file(&staged);
        receipt.staged_replacement_path = None;
        receipt.staged_replacement_sha256 = None;
        receipt.replacement_identity = None;
        return Err(refuse(&mut receipt, error));
    }

    // ---- destructive boundary: from here the live pathname changes. ----
    if let Err(error) = atomic_exchange(&staged, &plan.live_database_path) {
        // renameat2 either happened or did not; an error means it did not.
        let _ = fs::remove_file(&staged);
        receipt.staged_replacement_path = None;
        receipt.staged_replacement_sha256 = None;
        receipt.replacement_identity = None;
        return Err(refuse(&mut receipt, error));
    }
    receipt.state = DatabaseRestoreState::RestoreApplied;
    receipt.superseded_database_path = Some(staged.clone());
    receipt.applied_database_sha256 = receipt.staged_replacement_sha256.clone();
    receipt.message = "Verified database replacement applied atomically; verifying.".into();
    write_receipt(&receipt)?;
    restore_stage(STAGE_AFTER_EXCHANGE)?;

    receipt.state = DatabaseRestoreState::Verification;
    write_receipt(&receipt)?;
    let verification = restore_stage(STAGE_BEFORE_VERIFICATION)
        .and_then(|()| verify_applied_identity(&plan.live_database_path, &receipt))
        .and_then(|()| {
            validate_live_database(&plan.live_database_path, latest_known_version(MIGRATIONS))
        })
        .and_then(|()| restore_stage(STAGE_AFTER_VERIFICATION));
    if let Err(error) = verification {
        return Err(roll_back_after_exchange(
            &mut receipt,
            &exclusion,
            &staged,
            &emergency,
            error,
        ));
    }

    // The original is now redundant: the verified emergency backup carries
    // its content and the receipt carries both hashes. Its removal is
    // recorded before and after so a crash in between is explainable.
    restore_stage(STAGE_BEFORE_SUPERSEDED_REMOVAL)?;
    let removed = fs::remove_file(&staged).is_ok();
    fsync_directory(plan.live_database_path.parent())?;
    restore_stage(STAGE_BEFORE_COMPLETED_RECEIPT)?;
    receipt.state = DatabaseRestoreState::Completed;
    receipt.superseded_database_removed = removed;
    receipt.original_intact = Some(false);
    receipt.message = if plan.requires_upgrade() {
        format!(
            "Database restored from verified backup (upgraded from schema {} to {}); catalogue reload/restart is required.",
            plan.backup_schema_version,
            latest_known_version(MIGRATIONS)
        )
    } else {
        "Database restored from verified backup; catalogue reload/restart is required.".into()
    };
    write_receipt(&receipt)?;
    drop(exclusion);
    Ok(DatabaseRestoreResult {
        receipt,
        emergency_backup_path: emergency,
    })
}

/// Finalises a receipt for a refusal that happened before the exchange: the
/// original database is intact by construction.
fn refuse(receipt: &mut DatabaseRestoreReceipt, error: ArchiveFsError) -> ArchiveFsError {
    receipt.state = DatabaseRestoreState::Failed;
    receipt.original_intact = Some(true);
    receipt.message =
        format!("Restore failed before replacement; the original database is intact: {error}");
    // The refusal is the primary error; a receipt that cannot be written must
    // not hide it.
    let _ = write_receipt(receipt);
    error
}

fn roll_back_after_exchange(
    receipt: &mut DatabaseRestoreReceipt,
    exclusion: &RestoreExclusion,
    staged_old_live: &Path,
    emergency: &Path,
    error: ArchiveFsError,
) -> ArchiveFsError {
    receipt.state = DatabaseRestoreState::RollbackAttempted;
    receipt.message = format!(
        "Restore applied but verification failed: {error}. Attempting rollback from the superseded original at {}.",
        staged_old_live.display()
    );
    let _ = write_receipt(receipt);
    match rollback_exchange(
        exclusion,
        staged_old_live,
        &receipt.plan.live_database_path,
        receipt.plan.current_live_schema_version,
        receipt.plan.expected_live_sha256.as_str(),
    ) {
        Ok(()) => {
            receipt.state = DatabaseRestoreState::RolledBack;
            receipt.superseded_database_path = None;
            receipt.original_intact = Some(true);
            receipt.message = format!(
                "Restore verification failed ({error}); the original database was put back and verified. The emergency backup {} is retained.",
                emergency.display()
            );
        }
        Err(rollback_error) => {
            receipt.state = DatabaseRestoreState::RollbackFailed;
            receipt.original_intact = Some(false);
            receipt.message = format!(
                "Restore verification failed ({error}) and automatic rollback failed ({rollback_error}). The original database is preserved at {} and in the verified emergency backup {}; review before any further action.",
                staged_old_live.display(),
                emergency.display()
            );
        }
    }
    let _ = write_receipt(receipt);
    db(receipt.message.clone())
}

/// Puts the original database back after a completed restore, using the
/// verified emergency backup. Requires the same exclusion as a restore and
/// refuses if the restored database has changed since the restore completed.
pub fn rollback_database_restore(
    receipt: &DatabaseRestoreReceipt,
    confirmation: &str,
) -> Result<DatabaseRestoreReceipt> {
    if confirmation != CONFIRMATION {
        return Err(db(
            "Rollback requires the exact confirmation phrase RESTORE DATABASE.",
        ));
    }
    let live = receipt.plan.live_database_path.clone();
    let exclusion = RestoreExclusion::acquire(&live)
        .map_err(|error| db(format!("Rollback refused: {error}")))?;
    let (emergency, _) = review_rollback_inputs(receipt, Some(&exclusion))?;
    let schema = receipt.plan.current_live_schema_version;
    let staged = reserve_sibling(&live, "rollback-staging", "sqlite3")?;
    let mut updated = receipt.clone();
    updated.state = DatabaseRestoreState::RollbackAttempted;
    updated.updated_at_unix = unix_now();
    updated.message = format!(
        "Rolling back the completed restore from the verified emergency backup {}.",
        emergency.display()
    );
    write_receipt(&updated)?;
    if let Err(error) = stage_backup(&emergency, &staged, schema)
        .and_then(|()| require_no_sidecars_at_all(&live))
        .and_then(|()| require_no_sidecars_at_all(&staged))
    {
        let _ = fs::remove_file(&staged);
        updated.state = DatabaseRestoreState::Completed;
        updated.message = format!(
            "Rollback refused before any change ({error}); the restored database remains in place."
        );
        write_receipt(&updated)?;
        return Err(error);
    }
    atomic_exchange(&staged, &live)?;
    if let Err(error) = validate_live_database(&live, schema) {
        // The restored database is still at the staging path; put it back.
        let message = match atomic_exchange(&staged, &live) {
            Ok(()) => format!(
                "Rollback verification failed ({error}); the restored database was put back. The emergency backup remains at {}.",
                emergency.display()
            ),
            Err(undo) => format!(
                "Rollback verification failed ({error}) and the restored database could not be put back ({undo}); it is preserved at {}.",
                staged.display()
            ),
        };
        updated.state = DatabaseRestoreState::RollbackFailed;
        updated.message = message.clone();
        write_receipt(&updated)?;
        return Err(db(message));
    }
    let _ = fs::remove_file(&staged);
    fsync_directory(live.parent())?;
    updated.state = DatabaseRestoreState::RolledBack;
    updated.original_intact = Some(true);
    updated.superseded_database_path = None;
    updated.updated_at_unix = unix_now();
    updated.message = "Database rollback completed from the verified emergency backup; catalogue reload/restart is required.".into();
    write_receipt(&updated)?;
    Ok(updated)
}

/// Read-only review of a completed restore's rollback inputs: the emergency
/// backup must exist with its recorded hash, and the live database must still
/// be the restored replacement. Returns the receipt for the caller to hold.
pub fn review_database_rollback(receipt_path: &Path) -> Result<DatabaseRestoreReceipt> {
    let receipt = read_receipt(receipt_path)?;
    if receipt.state != DatabaseRestoreState::Completed {
        return Err(db(format!(
            "Only a completed restore can be rolled back; this receipt is {}.",
            receipt.state.label()
        )));
    }
    refuse_if_this_process_uses(&receipt.plan.live_database_path, "Rollback review")?;
    review_rollback_inputs(&receipt, None)?;
    Ok(receipt)
}

fn review_rollback_inputs(
    receipt: &DatabaseRestoreReceipt,
    exclusion: Option<&RestoreExclusion>,
) -> Result<(PathBuf, String)> {
    let emergency = receipt
        .emergency_backup_path
        .clone()
        .ok_or_else(|| db("No verified emergency backup is available for rollback."))?;
    let emergency_hash = receipt
        .emergency_backup_sha256
        .clone()
        .ok_or_else(|| db("Emergency backup evidence is incomplete; rollback requires review."))?;
    regular_file_metadata(&emergency, "emergency backup")?;
    if sha256_file(&emergency)? != emergency_hash {
        return Err(db("Emergency backup hash no longer matches."));
    }
    let current_hash = match exclusion {
        Some(exclusion) => exclusion.live_sha256()?,
        None => sha256_file(&receipt.plan.live_database_path)?,
    };
    if receipt.applied_database_sha256.as_deref() != Some(current_hash.as_str()) {
        return Err(db(
            "Current database changed since the restore completed; rollback requires review.",
        ));
    }
    validate_database_file(&emergency, receipt.plan.current_live_schema_version)?;
    Ok((emergency, emergency_hash))
}

/// What an interrupted restore left behind, judged from the receipt's
/// recorded hashes against the files as they are now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InterruptedRestoreOutcome {
    /// The receipt is already final; nothing to reconcile.
    Terminal,
    /// The exchange never happened: the live database is byte-identical to
    /// the state the plan was prepared from.
    OriginalIntact,
    /// The exchange happened and the replacement now verifies; the receipt
    /// can be finalised as completed.
    ReplacementVerified,
    /// The exchange happened but the replacement does not verify. The
    /// original is at `superseded_path` when present, and always in the
    /// emergency backup when `emergency_verified` is true.
    ReplacementFailedVerification { detail: String },
    /// The live database matches neither recorded hash; manual review only.
    Unrecognised { detail: String },
    /// Not judged: a worker in this process is using the database, and
    /// reading the file now could release its lock. Try again when idle.
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptedRestoreAssessment {
    pub receipt_path: PathBuf,
    pub receipt: DatabaseRestoreReceipt,
    pub outcome: InterruptedRestoreOutcome,
    pub live_sha256: Option<String>,
    /// The superseded original still exists at its recorded path with its
    /// recorded hash.
    pub superseded_original_present: bool,
    /// The emergency backup exists with its recorded hash.
    pub emergency_backup_verified: bool,
    /// Files this restore created that still exist and are no longer needed.
    pub leftover_paths: Vec<PathBuf>,
    pub summary: String,
    pub next_step: String,
}

impl InterruptedRestoreAssessment {
    /// Whether [`finalise_interrupted_restore`] can bring the receipt to a
    /// truthful terminal state without touching any database file.
    pub fn can_finalise(&self) -> bool {
        matches!(
            self.outcome,
            InterruptedRestoreOutcome::OriginalIntact
                | InterruptedRestoreOutcome::ReplacementVerified
        )
    }
}

/// Classifies a restore receipt after an interruption (crash, kill, power
/// loss) without changing anything. Hashing the live database is the only
/// cost; no file is opened for write.
pub fn assess_interrupted_restore(receipt_path: &Path) -> Result<InterruptedRestoreAssessment> {
    let receipt = read_receipt(receipt_path)?;
    assess_receipt(receipt_path, receipt, None)
}

fn assess_receipt(
    receipt_path: &Path,
    receipt: DatabaseRestoreReceipt,
    exclusion: Option<&RestoreExclusion>,
) -> Result<InterruptedRestoreAssessment> {
    let live = receipt.plan.live_database_path.clone();
    let emergency_backup_verified = match (
        &receipt.emergency_backup_path,
        &receipt.emergency_backup_sha256,
    ) {
        (Some(path), Some(sha)) => {
            path.is_file() && sha256_file(path).ok().as_deref() == Some(sha.as_str())
        }
        _ => false,
    };
    let superseded_original_present = receipt
        .staged_replacement_path
        .as_deref()
        .filter(|path| path.is_file())
        .and_then(|path| sha256_file(path).ok())
        .is_some_and(|sha| sha == receipt.plan.expected_live_sha256);
    let mut assessment = InterruptedRestoreAssessment {
        receipt_path: receipt_path.to_path_buf(),
        outcome: InterruptedRestoreOutcome::Terminal,
        live_sha256: None,
        superseded_original_present,
        emergency_backup_verified,
        leftover_paths: Vec::new(),
        summary: String::new(),
        next_step: String::new(),
        receipt,
    };
    if assessment.receipt.state.is_terminal() {
        assessment.summary = format!(
            "{}: {}",
            assessment.receipt.state.label(),
            assessment.receipt.message
        );
        assessment.next_step = "No reconciliation needed.".into();
        return Ok(assessment);
    }
    if exclusion.is_none() && journal_state::own_process_holds_lock(&live)? {
        assessment.outcome = InterruptedRestoreOutcome::Deferred;
        assessment.summary = "Interrupted restore: not judged yet because a worker in this window is using the database.".into();
        assessment.next_step = "Reopen History when no scan or action is running.".into();
        return Ok(assessment);
    }
    let live_sha =
        match regular_file_metadata(&live, "current database").and_then(|_| match exclusion {
            Some(exclusion) => exclusion.live_sha256(),
            None => sha256_file(&live),
        }) {
            Ok(sha) => sha,
            Err(error) => {
                assessment.outcome = InterruptedRestoreOutcome::Unrecognised {
                    detail: format!("the live database could not be read: {error}"),
                };
                assessment.summary =
                    "Interrupted restore: the live database could not be inspected.".into();
                assessment.next_step =
                    "Review the database directory by hand before opening EmuWiz again.".into();
                return Ok(assessment);
            }
        };
    assessment.live_sha256 = Some(live_sha.clone());
    let state_label = assessment.receipt.state.label();
    let expected_live_sha = assessment.receipt.plan.expected_live_sha256.clone();
    let staged_sha = assessment.receipt.staged_replacement_sha256.clone();
    let staged = assessment.receipt.staged_replacement_path.clone();
    let emergency = assessment.receipt.emergency_backup_path.clone();
    if live_sha == expected_live_sha {
        // Exchange never happened. Anything staged or backed up is surplus.
        for path in emergency.iter().chain(staged.iter()) {
            if path.is_file() {
                assessment.leftover_paths.push(path.clone());
            }
        }
        assessment.outcome = InterruptedRestoreOutcome::OriginalIntact;
        assessment.summary = format!(
            "Interrupted at '{state_label}' before the replacement: the original database is in place and unchanged."
        );
        assessment.next_step = if assessment.leftover_paths.is_empty() {
            "Finalise the receipt; nothing else to do.".into()
        } else {
            "Finalise the receipt; the listed leftover files can be deleted afterwards.".into()
        };
        return Ok(assessment);
    }
    if Some(live_sha.as_str()) == staged_sha.as_deref() {
        match validate_live_database(&live, latest_known_version(MIGRATIONS)) {
            Ok(()) => {
                assessment.outcome = InterruptedRestoreOutcome::ReplacementVerified;
                assessment.summary = format!(
                    "Interrupted at '{state_label}' after the replacement: the restored database is in place and verifies."
                );
                assessment.next_step = if superseded_original_present {
                    "Finalise the receipt as completed; the superseded original is kept at its staging path until you delete it.".into()
                } else {
                    "Finalise the receipt as completed.".into()
                };
            }
            Err(error) => {
                assessment.outcome = InterruptedRestoreOutcome::ReplacementFailedVerification {
                    detail: error.to_string(),
                };
                assessment.summary = format!(
                    "Interrupted at '{state_label}' after the replacement, and the restored database does not verify."
                );
                assessment.next_step = match (superseded_original_present, emergency_backup_verified) {
                    (true, _) => format!(
                        "Put the original back from {} (it matches the pre-restore hash), or restore from the emergency backup.",
                        staged.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
                    ),
                    (false, true) => "Restore from the verified emergency backup named in the receipt.".into(),
                    (false, false) => "Neither the superseded original nor a verified emergency backup is present; recover by hand.".into(),
                };
            }
        }
        return Ok(assessment);
    }
    assessment.outcome = InterruptedRestoreOutcome::Unrecognised {
        detail: "the live database matches neither the pre-restore hash nor the staged replacement"
            .into(),
    };
    assessment.summary = format!(
        "Interrupted at '{state_label}': the live database matches neither recorded state."
    );
    assessment.next_step =
        "Do not finalise automatically; review the database directory by hand.".into();
    Ok(assessment)
}

/// Brings an interrupted receipt to a truthful terminal state. Only the
/// receipt is written; no database file is created, changed or deleted. The
/// assessment is repeated under the restore exclusion so it cannot be stale.
pub fn finalise_interrupted_restore(
    receipt_path: &Path,
    confirmation: &str,
) -> Result<InterruptedRestoreAssessment> {
    if confirmation != CONFIRMATION {
        return Err(db(
            "Finalising an interrupted restore requires the exact confirmation phrase RESTORE DATABASE.",
        ));
    }
    let receipt = read_receipt(receipt_path)?;
    let exclusion = RestoreExclusion::acquire(&receipt.plan.live_database_path)
        .map_err(|error| db(format!("Reconciliation refused: {error}")))?;
    let mut assessment = assess_receipt(receipt_path, receipt, Some(&exclusion))?;
    let state_label = assessment.receipt.state.label();
    match &assessment.outcome {
        InterruptedRestoreOutcome::Terminal => return Ok(assessment),
        InterruptedRestoreOutcome::OriginalIntact => {
            assessment.receipt.state = DatabaseRestoreState::Failed;
            assessment.receipt.original_intact = Some(true);
            assessment.receipt.message = format!(
                "Restore was interrupted at '{state_label}' before the replacement; the original database is intact. Leftover files: {}.",
                if assessment.leftover_paths.is_empty() {
                    "none".to_string()
                } else {
                    assessment
                        .leftover_paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
        }
        InterruptedRestoreOutcome::ReplacementVerified => {
            let superseded_present = assessment.superseded_original_present;
            let superseded_path = assessment.receipt.staged_replacement_path.clone();
            assessment.receipt.state = DatabaseRestoreState::Completed;
            assessment.receipt.original_intact = Some(false);
            assessment.receipt.applied_database_sha256 = assessment.live_sha256.clone();
            assessment.receipt.superseded_database_path =
                superseded_present.then_some(superseded_path).flatten();
            assessment.receipt.superseded_database_removed = !superseded_present;
            assessment.receipt.message = format!(
                "Restore was interrupted at '{state_label}' after the replacement; the restored database verified after restart. Catalogue reload/restart is required."
            );
        }
        InterruptedRestoreOutcome::ReplacementFailedVerification { .. }
        | InterruptedRestoreOutcome::Unrecognised { .. }
        | InterruptedRestoreOutcome::Deferred => {
            return Err(db(format!(
                "Cannot finalise automatically: {} {}",
                assessment.summary, assessment.next_step
            )));
        }
    }
    assessment.receipt.updated_at_unix = unix_now();
    write_receipt(&assessment.receipt)?;
    Ok(assessment)
}

/// Receipt files beside `live` whose restore never reached a terminal state.
pub fn interrupted_restore_receipts(live: &Path) -> Vec<PathBuf> {
    let Some(parent) = live.parent() else {
        return Vec::new();
    };
    let Some(name) = live.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{name}.restore-");
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".json"))
        })
        .filter(|path| read_receipt(path).is_ok_and(|receipt| !receipt.state.is_terminal()))
        .collect();
    found.sort();
    found
}

pub fn read_receipt(path: &Path) -> Result<DatabaseRestoreReceipt> {
    let bytes = fs::read(path).map_err(|error| {
        db(format!(
            "Could not read restore receipt {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        db(format!(
            "Malformed restore receipt {}: {error}",
            path.display()
        ))
    })
}

fn validate_plan(plan: &DatabaseRestorePlan, exclusion: &RestoreExclusion) -> Result<()> {
    regular_file_metadata(&plan.selected_backup_path, "selected backup")?;
    if same_file(&plan.live_database_path, &plan.selected_backup_path)? {
        return Err(db("The selected backup is the live database itself."));
    }
    if sha256_file(&plan.selected_backup_path)? != plan.selected_backup_sha256 {
        return Err(db("Backup hash no longer matches."));
    }
    let latest = latest_known_version(MIGRATIONS);
    if plan.current_live_schema_version != latest {
        return Err(db(
            "Current database schema is not supported by this build.",
        ));
    }
    if plan.pending_migrations != pending_migration_versions(MIGRATIONS, plan.backup_schema_version)
    {
        return Err(db(
            "This restore plan was prepared by a different EmuWiz build; review the restore again.",
        ));
    }
    {
        let connection = open_read_only_connection(&plan.selected_backup_path)?;
        let schema = verify_backup_connection(&connection)?;
        if schema != plan.backup_schema_version {
            return Err(db(format!(
                "Backup schema is {schema}, expected {}.",
                plan.backup_schema_version
            )));
        }
    }
    validate_plan_live_freshness(plan, exclusion)?;
    // The live file is re-verified under the exclusion: a replay or a torn
    // write between review and now would have changed its hash above, but a
    // corrupt page that quick_check catches must stop the restore too.
    let connection = open_read_only_connection(&plan.live_database_path)?;
    verify_database_connection(&connection, latest).map_err(|error| {
        db(format!(
            "The current database did not pass SQLite's safety check, so no safety backup can be trusted: {error}"
        ))
    })
}

fn validate_plan_live_freshness(
    plan: &DatabaseRestorePlan,
    exclusion: &RestoreExclusion,
) -> Result<()> {
    let meta = exclusion.live_metadata()?;
    let modified = modified_unix(&meta);
    if meta.len() != plan.expected_live_size_bytes
        || modified != plan.expected_live_modified_unix_seconds
        || exclusion.live_sha256()? != plan.expected_live_sha256
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
    let backup_len =
        u128::from(regular_file_metadata(&plan.selected_backup_path, "selected backup")?.len());
    // Emergency backup + staged copy; an upgraded staging copy also needs
    // room for its migration journals, bounded here by one more copy.
    let staging_headroom = if plan.requires_upgrade() {
        backup_len
    } else {
        0
    };
    let required =
        u128::from(plan.expected_live_size_bytes) + backup_len + staging_headroom + 1024 * 1024;
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
    let actual = verify_backup_connection(&connection)?;
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

/// After the exchange the live pathname must carry the staged inode and the
/// staged bytes; anything else means the exchange did not do what was asked.
fn verify_applied_identity(live: &Path, receipt: &DatabaseRestoreReceipt) -> Result<()> {
    let identity = file_identity(live, "restored database")?;
    if Some(identity) != receipt.replacement_identity {
        return Err(db(
            "The live pathname does not carry the staged replacement after the exchange.",
        ));
    }
    if sha256_file(live)?.as_str()
        != receipt
            .staged_replacement_sha256
            .as_deref()
            .unwrap_or_default()
    {
        return Err(db(
            "The restored database does not match the verified staged copy.",
        ));
    }
    Ok(())
}

/// A backup is acceptable at any schema this build can migrate from.
fn verify_backup_connection(connection: &Connection) -> Result<i64> {
    let schema = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|e| db(format!("Could not read SQLite schema version: {e}")))?;
    let latest = latest_known_version(MIGRATIONS);
    if schema < 1 || schema > latest {
        return Err(db(format!(
            "SQLite schema {schema} is not supported; this build restores schemas 1 through {latest}."
        )));
    }
    verify_database_connection(connection, schema)?;
    require_tables(connection)?;
    Ok(schema)
}

/// The live database must already be at this build's schema.
fn verify_current_connection(connection: &Connection) -> Result<i64> {
    let schema = verify_backup_connection(connection)?;
    let latest = latest_known_version(MIGRATIONS);
    if schema != latest {
        return Err(db(format!(
            "Current database schema {schema} is not this build's schema {latest}; upgrade the library before restoring into it."
        )));
    }
    Ok(schema)
}

fn require_tables(connection: &Connection) -> Result<()> {
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
    Ok(())
}

/// Online-backup copy of `source` to `destination`, fsynced and verified at
/// `schema`. `source` is only opened read-only.
fn stage_backup(source: &Path, destination: &Path, schema: i64) -> Result<()> {
    let source_connection = open_read_only_connection(source)?;
    source_connection
        .backup(MAIN_DB, destination, None)
        .map_err(|e| db(format!("Could not stage verified backup: {e}")))?;
    drop(source_connection);
    fsync_file(destination)?;
    validate_database_file(destination, schema)
}

/// Copies the selected backup into the staging file and, when it is older
/// than this build, migrates the copy there. Returns the staged schema.
fn stage_replacement(plan: &DatabaseRestorePlan, staged: &Path) -> Result<i64> {
    stage_backup(
        &plan.selected_backup_path,
        staged,
        plan.backup_schema_version,
    )?;
    if !plan.requires_upgrade() {
        return Ok(plan.backup_schema_version);
    }
    let latest = latest_known_version(MIGRATIONS);
    let upgrade = (|| -> Result<()> {
        let mut connection = open_connection_with(staged, false)?;
        apply_migrations(&mut connection, MIGRATIONS)?;
        let migrated = schema_version(&connection)?;
        if migrated != latest {
            return Err(db(format!(
                "staged upgrade finished at schema {migrated}, expected {latest}"
            )));
        }
        Ok(())
    })();
    if let Err(error) = upgrade {
        return Err(db(format!(
            "Could not upgrade the staged copy of the backup from schema {} to {latest}: {error}. The selected backup was only read and is unchanged.",
            plan.backup_schema_version
        )));
    }
    fsync_file(staged)?;
    // Migrations ran in their own transactions; none may have left a journal.
    require_no_sidecars_at_all(staged)?;
    validate_database_file(staged, latest)?;
    Ok(latest)
}

fn create_verified_backup(source: &Path, destination: &Path, schema: i64) -> Result<()> {
    stage_backup(source, destination, schema)
}

fn regular_file_metadata(path: &Path, label: &str) -> Result<fs::Metadata> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| db(format!("{label} is unavailable: {e}")))?;
    if !metadata.is_file() {
        return Err(db(format!("{label} is not a regular file.")));
    }
    Ok(metadata)
}

fn file_identity(path: &Path, label: &str) -> Result<FileIdentity> {
    let metadata = regular_file_metadata(path, label)?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn same_file(left: &Path, right: &Path) -> Result<bool> {
    Ok(file_identity(left, "current database")? == file_identity(right, "selected backup")?)
}

fn modified_unix(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
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
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
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

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn sidecar_exists(path: &Path, suffix: &str) -> Result<bool> {
    match fs::symlink_metadata(sidecar(path, suffix)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(db(format!(
            "Could not inspect the {suffix} sidecar of {}: {error}",
            path.display()
        ))),
    }
}

/// The catalogue runs in DELETE journal mode; a WAL or shared-memory file
/// beside it belongs to a different configuration and must never be carried
/// across an exchange.
fn require_no_wal_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        if sidecar_exists(path, suffix)? {
            return Err(db(format!(
                "Database has an active WAL/SHM sidecar ({}); close the database and retry restore. Nothing was changed.",
                sidecar(path, suffix).display()
            )));
        }
    }
    Ok(())
}

/// With the exclusion held no connection can be mid-write, so no sidecar of
/// any kind may exist beside the file that is about to be exchanged.
fn require_no_sidecars_at_all(path: &Path) -> Result<()> {
    require_no_wal_sidecars(path)?;
    if sidecar_exists(path, "-journal")? {
        return Err(db(format!(
            "A rollback journal ({}) exists beside the database; the exchange would separate it from its database. Nothing was changed.",
            sidecar(path, "-journal").display()
        )));
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

/// Exchanges the superseded original (still at the staging path) back into
/// place after it has been proven to be the pre-restore bytes.
fn rollback_exchange(
    exclusion: &RestoreExclusion,
    staged_old_live: &Path,
    live: &Path,
    schema: i64,
    expected_sha256: &str,
) -> Result<()> {
    // The superseded original is the inode the exclusion handle still
    // refers to, wherever its pathname now is.
    if exclusion.live_sha256()? != expected_sha256 {
        return Err(db(
            "The superseded original at the staging path no longer matches its pre-restore hash.",
        ));
    }
    validate_database_file(staged_old_live, schema)?;
    atomic_exchange(staged_old_live, live)?;
    validate_live_database(live, schema)?;
    let _ = fs::remove_file(staged_old_live);
    Ok(())
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
        staged_replacement_path: None,
        staged_replacement_sha256: None,
        original_identity: None,
        replacement_identity: None,
        superseded_database_path: None,
        superseded_database_removed: false,
        original_intact: None,
        exclusion_held: false,
    }
}
pub fn receipt_path(receipt: &DatabaseRestoreReceipt) -> PathBuf {
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
/// Atomic, durable receipt publication: the previous receipt is never
/// truncated in place, so a crash mid-write leaves the last good one.
fn write_receipt(receipt: &DatabaseRestoreReceipt) -> Result<()> {
    let path = receipt_path(receipt);
    let temporary = path.with_file_name(format!(
        "{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|e| db(format!("Could not encode restore receipt: {e}")))?;
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(db(format!("Could not write restore receipt: {error}")));
    }
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
    /// A backup at an older schema, built by running only the first `steps`
    /// migrations, with the same marker table.
    fn old_schema_db(path: &Path, steps: usize, marker: &str) {
        let mut connection = super::super::open_connection(path).unwrap();
        apply_migrations(&mut connection, &MIGRATIONS[..steps]).unwrap();
        connection
            .execute(
                "CREATE TABLE IF NOT EXISTS restore_marker(value TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO restore_marker(value) VALUES (?1)", [marker])
            .unwrap();
    }
    fn receipts(root: &Path) -> Vec<DatabaseRestoreReceipt> {
        let mut found: Vec<_> = fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.to_string_lossy().contains(".restore-")
                    && p.extension().is_some_and(|e| e == "json")
            })
            .map(|p| read_receipt(&p).unwrap())
            .collect();
        found.sort_by_key(|r| r.operation_id.clone());
        found
    }

    #[test]
    fn verified_restore_exchanges_and_retains_emergency_backup() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        assert!(!plan.requires_upgrade());
        let result = restore_database(&plan, CONFIRMATION).unwrap();
        assert_eq!(result.receipt.state, DatabaseRestoreState::Completed);
        assert!(result.receipt.exclusion_held);
        assert_eq!(result.receipt.original_intact, Some(false));
        assert!(result.receipt.superseded_database_removed);
        assert_eq!(marker(&live), "backup");
        assert_eq!(marker(&result.emergency_backup_path), "live");
        // The superseded original is gone; the backup was never written.
        assert!(!result.receipt.staged_replacement_path.unwrap().exists());
        assert_eq!(sha256_file(&backup).unwrap(), plan.selected_backup_sha256);
        // No sidecars or temporary receipts remain.
        let names: Vec<String> = fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names
                .iter()
                .all(|n| !n.contains(".tmp-") && !n.ends_with("-journal")),
            "{names:?}"
        );
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
        let receipt = &receipts(root.path())[0];
        assert_eq!(receipt.state, DatabaseRestoreState::Failed);
        assert_eq!(receipt.original_intact, Some(true));
        assert_eq!(marker(&live), "changed");
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
        assert_eq!(marker(&live), "live");
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
        let path = receipt_path(&result.receipt);
        let reviewed = review_database_rollback(&path).unwrap();
        assert_eq!(reviewed, result.receipt);
        let rolled_back = rollback_database_restore(&result.receipt, CONFIRMATION).unwrap();
        assert_eq!(rolled_back.state, DatabaseRestoreState::RolledBack);
        assert_eq!(marker(&live), "live");
        // A second rollback is refused: the live database is no longer the
        // restored replacement.
        let error = review_database_rollback(&path).unwrap_err();
        assert!(error.to_string().contains("completed restore"), "{error}");
    }

    #[test]
    fn rollback_is_refused_when_restored_database_changed() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let result = restore_database(&plan, CONFIRMATION).unwrap();
        db(&live, "edited after restore");
        let error = rollback_database_restore(&result.receipt, CONFIRMATION).unwrap_err();
        assert!(
            error.to_string().contains("changed since the restore"),
            "{error}"
        );
        assert_eq!(marker(&live), "edited after restore");
    }

    #[test]
    fn active_wal_sidecar_blocks_replacement() {
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
        assert!(!root.path().read_dir().unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("before-restore")
        }));
    }

    #[test]
    fn hot_rollback_journal_without_writer_is_refused_closed() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        // A journal with a live header and no lock holder: a crashed writer.
        let journal = live.with_file_name("library.sqlite3-journal");
        fs::write(
            &journal,
            [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7, 0, 0, 0, 0],
        )
        .unwrap();
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Database activity cannot be determined"),
            "{error}"
        );
        assert!(
            !error.to_string().to_lowercase().contains("crash"),
            "{error}"
        );
        assert!(
            journal.exists(),
            "the journal must be preserved for copy-first recovery"
        );
        assert_eq!(fs::read(&journal).unwrap().len(), 12);
        let receipt = &receipts(root.path())[0];
        assert_eq!(receipt.state, DatabaseRestoreState::Failed);
        assert_eq!(receipt.original_intact, Some(true));
    }

    #[test]
    fn writer_holding_open_transaction_makes_restore_refuse() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let writer = super::super::Database::open_or_create(&live).unwrap();
        writer
            .connection
            .execute_batch(
                "BEGIN IMMEDIATE; INSERT INTO restore_marker(value) VALUES ('in-flight');",
            )
            .unwrap();
        let started = std::time::Instant::now();
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("writing the library database")
                || text.contains("using the library database"),
            "{text}"
        );
        assert!(text.contains("Nothing was changed"), "{text}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // The writer's transaction is untouched and commits normally.
        writer.connection.execute_batch("COMMIT").unwrap();
        drop(writer);
        let count: i64 = super::super::Database::open_read_only(&live)
            .unwrap()
            .connection
            .query_row("SELECT COUNT(*) FROM restore_marker", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert!(!root.path().read_dir().unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("before-restore")
        }));
        let receipt = &receipts(root.path())[0];
        assert_eq!(receipt.state, DatabaseRestoreState::Failed);
        assert_eq!(receipt.original_intact, Some(true));
    }

    #[test]
    fn reader_holding_shared_lock_does_not_block_restore() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        // An open read-only connection between statements holds no lock, so
        // it neither blocks the restore nor is disturbed by it.
        let reader = super::super::Database::open_read_only(&live).unwrap();
        let before: String = reader
            .connection
            .query_row("SELECT value FROM restore_marker", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, "live");
        let result = restore_database(&plan, CONFIRMATION).unwrap();
        assert_eq!(result.receipt.state, DatabaseRestoreState::Completed);
        // The reader keeps reading the inode it opened (the superseded
        // original) until it reopens; that is why the GUI reloads.
        let seen: String = reader
            .connection
            .query_row("SELECT value FROM restore_marker", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seen, "live");
        drop(reader);
        assert_eq!(marker(&live), "backup");
    }

    #[test]
    fn stale_connection_cannot_write_superseded_database_silently() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let stale = super::super::Database::open_or_create(&live).unwrap();
        let result = restore_database(&plan, CONFIRMATION).unwrap();
        assert_eq!(result.receipt.state, DatabaseRestoreState::Completed);
        let error = stale
            .connection
            .execute("INSERT INTO restore_marker(value) VALUES ('late')", [])
            .unwrap_err();
        assert!(error.to_string().contains("readonly"), "{error}");
        assert_eq!(marker(&live), "backup");
    }

    #[test]
    fn old_schema_backup_is_upgraded_in_staging_and_backup_file_untouched() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root
            .path()
            .join("library.sqlite3.schema-3-before-26.backup");
        db(&live, "live");
        old_schema_db(&backup, 3, "old backup");
        let before = sha256_file(&backup).unwrap();
        let plan = prepare_database_restore(&live, &backup).unwrap();
        assert_eq!(plan.backup_schema_version, 3);
        assert!(plan.requires_upgrade());
        assert_eq!(plan.pending_migrations.first(), Some(&4));
        let result = restore_database(&plan, CONFIRMATION).unwrap();
        assert_eq!(result.receipt.state, DatabaseRestoreState::Completed);
        assert!(result.receipt.message.contains("upgraded from schema 3"));
        assert_eq!(marker(&live), "old backup");
        let restored = super::super::Database::open_read_only(&live).unwrap();
        assert_eq!(
            schema_version(&restored.connection).unwrap(),
            latest_known_version(MIGRATIONS)
        );
        assert_eq!(
            sha256_file(&backup).unwrap(),
            before,
            "selected backup must only be read"
        );
        assert!(
            !backup
                .with_file_name("library.sqlite3.schema-3-before-26.backup-journal")
                .exists()
        );
        assert_eq!(marker(&result.emergency_backup_path), "live");
    }

    #[test]
    fn failed_staged_migration_leaves_live_and_backup_untouched() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("old.backup");
        db(&live, "live");
        old_schema_db(&backup, 3, "old backup");
        // Migration 0004 adds scan_runs.archives_unchanged; a backup that
        // already carries that column makes the staged upgrade fail, loudly.
        assert!(MIGRATIONS[3].sql.contains("ADD COLUMN archives_unchanged"));
        {
            let connection = super::super::open_connection(&backup).unwrap();
            connection
                .execute(
                    "ALTER TABLE scan_runs ADD COLUMN archives_unchanged INTEGER NOT NULL DEFAULT 0",
                    [],
                )
                .unwrap();
        }
        let before = sha256_file(&backup).unwrap();
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(
            error.to_string().contains("upgrade the staged copy"),
            "{error}"
        );
        assert_eq!(marker(&live), "live");
        assert_eq!(sha256_file(&backup).unwrap(), before);
        assert_eq!(sha256_file(&live).unwrap(), plan.expected_live_sha256);
        let receipt = &receipts(root.path())[0];
        assert_eq!(receipt.state, DatabaseRestoreState::Failed);
        assert_eq!(receipt.original_intact, Some(true));
        assert!(receipt.staged_replacement_path.is_none());
        let names: Vec<String> = fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.contains("restore-staging")),
            "{names:?}"
        );
    }

    #[test]
    fn missing_or_replaced_backup_is_refused() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        fs::remove_file(&backup).unwrap();
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(
            error.to_string().contains("selected backup is unavailable"),
            "{error}"
        );
        db(&backup, "different");
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(
            error.to_string().contains("hash no longer matches"),
            "{error}"
        );
        assert_eq!(marker(&live), "live");
    }

    #[test]
    fn live_database_as_backup_is_refused() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        db(&live, "live");
        let error = prepare_database_restore(&live, &live).unwrap_err();
        assert!(
            error.to_string().contains("live database itself"),
            "{error}"
        );
    }

    #[test]
    fn receipts_are_written_atomically_and_legacy_receipts_still_parse() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let result = restore_database(&plan, CONFIRMATION).unwrap();
        let path = receipt_path(&result.receipt);
        assert!(
            !path
                .with_file_name(format!(
                    "{}.tmp-{}",
                    path.file_name().unwrap().to_string_lossy(),
                    std::process::id()
                ))
                .exists()
        );
        // A receipt written by the previous build (no new fields) parses.
        let legacy = serde_json::json!({
            "schema_version": 1, "operation_id": "database-restore-1-abc", "state": "completed",
            "plan": {"plan_id": "database-restore-1-abc", "generated_at_unix": 1, "live_database_path": live, "selected_backup_path": backup,
                     "selected_backup_sha256": "x", "backup_schema_version": 26, "expected_live_size_bytes": 1, "expected_live_modified_unix_seconds": null,
                     "expected_live_sha256": "y", "current_live_schema_version": 26},
            "emergency_backup_path": null, "emergency_backup_sha256": null, "applied_database_sha256": null,
            "created_at_unix": 1, "updated_at_unix": 1, "message": "old"
        });
        let parsed: DatabaseRestoreReceipt = serde_json::from_value(legacy).unwrap();
        assert!(!parsed.exclusion_held);
        assert_eq!(parsed.original_intact, None);
        assert!(parsed.plan.pending_migrations.is_empty());
    }

    #[test]
    fn interrupted_before_exchange_is_assessed_as_original_intact() {
        for stage in [
            STAGE_AFTER_EXCLUSION,
            STAGE_AFTER_EMERGENCY_BACKUP,
            STAGE_AFTER_STAGING,
            STAGE_BEFORE_EXCHANGE,
        ] {
            let root = tempdir().unwrap();
            let live = root.path().join("library.sqlite3");
            let backup = root.path().join("selected.backup");
            db(&live, "live");
            db(&backup, "backup");
            let plan = prepare_database_restore(&live, &backup).unwrap();
            let crash_at = stage.to_string();
            let outcome = {
                let _guard = fault_injection::install(move |s| {
                    if s == crash_at {
                        panic!("simulated crash at {s}")
                    } else {
                        Ok(())
                    }
                });
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    restore_database(&plan, CONFIRMATION)
                }))
            };
            assert!(outcome.is_err(), "{stage}: expected a simulated crash");
            assert_eq!(marker(&live), "live", "{stage}");
            assert_eq!(
                sha256_file(&live).unwrap(),
                plan.expected_live_sha256,
                "{stage}"
            );
            let receipt_path = interrupted_restore_receipts(&live)
                .pop()
                .expect("non-terminal receipt");
            let assessment = assess_interrupted_restore(&receipt_path).unwrap();
            assert_eq!(
                assessment.outcome,
                InterruptedRestoreOutcome::OriginalIntact,
                "{stage}: {}",
                assessment.summary
            );
            assert!(assessment.can_finalise());
            // The database still opens normally after the crash: the
            // exclusion died with the process.
            assert_eq!(marker(&live), "live");
            let finalised = finalise_interrupted_restore(&receipt_path, CONFIRMATION).unwrap();
            assert_eq!(finalised.receipt.state, DatabaseRestoreState::Failed);
            assert_eq!(finalised.receipt.original_intact, Some(true));
            assert!(interrupted_restore_receipts(&live).is_empty());
            for leftover in &assessment.leftover_paths {
                assert!(
                    leftover.exists(),
                    "{stage}: finalise must not delete {}",
                    leftover.display()
                );
            }
            // A new restore succeeds afterwards.
            let plan = prepare_database_restore(&live, &backup).unwrap();
            assert_eq!(
                restore_database(&plan, CONFIRMATION).unwrap().receipt.state,
                DatabaseRestoreState::Completed,
                "{stage}"
            );
        }
    }

    #[test]
    fn interrupted_after_exchange_is_assessed_as_replacement_verified() {
        for stage in [
            STAGE_AFTER_EXCHANGE,
            STAGE_BEFORE_VERIFICATION,
            STAGE_AFTER_VERIFICATION,
            STAGE_BEFORE_SUPERSEDED_REMOVAL,
            STAGE_BEFORE_COMPLETED_RECEIPT,
        ] {
            let root = tempdir().unwrap();
            let live = root.path().join("library.sqlite3");
            let backup = root.path().join("selected.backup");
            db(&live, "live");
            db(&backup, "backup");
            let plan = prepare_database_restore(&live, &backup).unwrap();
            let crash_at = stage.to_string();
            let outcome = {
                let _guard = fault_injection::install(move |s| {
                    if s == crash_at {
                        panic!("simulated crash at {s}")
                    } else {
                        Ok(())
                    }
                });
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    restore_database(&plan, CONFIRMATION)
                }))
            };
            assert!(outcome.is_err(), "{stage}");
            assert_eq!(marker(&live), "backup", "{stage}: replacement is in place");
            let receipt_path = interrupted_restore_receipts(&live)
                .pop()
                .expect("non-terminal receipt");
            let assessment = assess_interrupted_restore(&receipt_path).unwrap();
            assert_eq!(
                assessment.outcome,
                InterruptedRestoreOutcome::ReplacementVerified,
                "{stage}: {}",
                assessment.summary
            );
            assert!(assessment.emergency_backup_verified, "{stage}");
            let superseded_expected = stage != STAGE_BEFORE_COMPLETED_RECEIPT;
            assert_eq!(
                assessment.superseded_original_present, superseded_expected,
                "{stage}"
            );
            let finalised = finalise_interrupted_restore(&receipt_path, CONFIRMATION).unwrap();
            assert_eq!(finalised.receipt.state, DatabaseRestoreState::Completed);
            assert_eq!(finalised.receipt.original_intact, Some(false));
            assert_eq!(
                finalised.receipt.applied_database_sha256.as_deref(),
                Some(sha256_file(&live).unwrap().as_str())
            );
            // The finalised receipt supports a rollback, which restores the original.
            let rolled = rollback_database_restore(&finalised.receipt, CONFIRMATION).unwrap();
            assert_eq!(rolled.state, DatabaseRestoreState::RolledBack);
            assert_eq!(marker(&live), "live", "{stage}");
        }
    }

    #[test]
    fn verification_failure_after_exchange_rolls_back_to_original() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let _guard = fault_injection::install(|s| {
            if s == STAGE_BEFORE_VERIFICATION {
                Err("verification could not run".into())
            } else {
                Ok(())
            }
        });
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(
            error.to_string().contains("put back and verified"),
            "{error}"
        );
        assert_eq!(marker(&live), "live");
        assert_eq!(sha256_file(&live).unwrap(), plan.expected_live_sha256);
        let receipt = &receipts(root.path())[0];
        assert_eq!(receipt.state, DatabaseRestoreState::RolledBack);
        assert_eq!(receipt.original_intact, Some(true));
        assert!(receipt.emergency_backup_path.as_ref().unwrap().exists());
        assert!(!root.path().read_dir().unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("restore-staging")
        }));
    }

    #[test]
    fn hashing_the_live_database_under_the_exclusion_keeps_the_lock() {
        // Closing any descriptor on the file would drop this process's SQLite
        // locks; the exclusion hashes through its held handle instead.
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        db(&live, "live");
        let expected = sha256_file(&live).unwrap();
        let held = RestoreExclusion::acquire(&live).unwrap();
        assert_eq!(held.live_sha256().unwrap(), expected);
        assert_eq!(
            held.live_metadata().unwrap().len(),
            fs::metadata(&live).unwrap().len()
        );
        assert!(journal_state::sqlite_write_lock_held(&live).unwrap());
        // A read-only connection can read; a writer in this process is refused.
        let reader = super::super::Database::open_read_only(&live).unwrap();
        reader
            .connection
            .query_row("SELECT COUNT(*) FROM archives", [], |r| r.get::<_, i64>(0))
            .unwrap();
        let writer = rusqlite::Connection::open(&live).unwrap();
        writer.busy_timeout(Duration::from_millis(100)).unwrap();
        let error = writer.execute_batch("BEGIN IMMEDIATE").unwrap_err();
        assert!(error.to_string().contains("locked"), "{error}");
        drop(held);
        writer.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
    }

    #[test]
    fn restore_is_refused_while_a_worker_of_this_process_holds_any_lock() {
        // An in-process scan holding a read transaction must not have its
        // lock dropped by the restore's own file handling.
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let worker = super::super::Database::open_read_only(&live).unwrap();
        worker
            .connection
            .execute_batch("BEGIN; SELECT COUNT(*) FROM archives;")
            .unwrap();
        let error = restore_database(&plan, CONFIRMATION)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("a worker in this EmuWiz window is using"),
            "{error}"
        );
        assert!(
            journal_state::own_process_holds_lock(&live).unwrap(),
            "the worker's lock survived"
        );
        worker.connection.execute_batch("COMMIT").unwrap();
        drop(worker);
        assert_eq!(
            restore_database(&plan, CONFIRMATION).unwrap().receipt.state,
            DatabaseRestoreState::Completed
        );
    }

    #[test]
    fn writer_with_spilled_transaction_makes_restore_refuse_as_busy() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let writer = super::super::Database::open_or_create(&live).unwrap();
        writer
            .connection
            .execute_batch("PRAGMA cache_size = 10; BEGIN IMMEDIATE;")
            .unwrap();
        let payload = "x".repeat(900);
        let journal = live.with_file_name("library.sqlite3-journal");
        for _ in 0..4000 {
            writer
                .connection
                .execute("INSERT INTO restore_marker(value) VALUES (?1)", [&payload])
                .unwrap();
            if fs::read(&journal)
                .is_ok_and(|bytes| bytes.len() >= 8 && bytes[..8].iter().any(|b| *b != 0))
            {
                break;
            }
        }
        let error = restore_database(&plan, CONFIRMATION)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("using the library database")
                || error.contains("writing the library database"),
            "{error}"
        );
        assert!(!error.contains("hot journal"), "{error}");
        writer.connection.execute_batch("ROLLBACK").unwrap();
        drop(writer);
        assert_eq!(marker(&live), "live");
        assert_eq!(sha256_file(&live).unwrap(), plan.expected_live_sha256);
    }

    #[test]
    fn second_restore_is_refused_while_one_holds_the_exclusion() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        db(&live, "live");
        db(&backup, "backup");
        let plan = prepare_database_restore(&live, &backup).unwrap();
        let held = RestoreExclusion::acquire(&live).unwrap();
        let error = restore_database(&plan, CONFIRMATION).unwrap_err();
        assert!(error.to_string().contains("already running"), "{error}");
        drop(held);
        assert_eq!(
            restore_database(&plan, CONFIRMATION).unwrap().receipt.state,
            DatabaseRestoreState::Completed
        );
    }

    #[test]
    fn database_open_is_refused_while_restore_exclusion_is_held() {
        let root = tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        db(&live, "live");
        let held = RestoreExclusion::acquire(&live).unwrap();
        // A writer in this process trying to open the catalogue waits for the
        // upgrade lock and then refuses; it does not block forever.
        let started = std::time::Instant::now();
        let error = super::super::Database::open_or_create(&live).unwrap_err();
        assert!(error.to_string().contains("already running"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(30));
        drop(held);
        assert_eq!(marker(&live), "live");
    }
}
