//! Guarded PS2 PSU restore: the parent module computes the candidate image.
//! This module owns card publication, Undo, durable journals and recovery,
//! using the Save Vault quiescence contract.
//!
//! * **Quiescence.** Mutation is refused unless the Save Vault policy
//!   ([`QuiescenceProvider`]) reports `Closed`. `Running` and `Unknown` refuse,
//!   and the check is repeated immediately before publication.
//! * **Durable intent.** A checksummed journal is written (atomically, with
//!   fsync) before the first mutation and at every phase change, so an
//!   interrupted restore can be found and judged after a restart.
//! * **Recheck before publication.** The live card must still be the reviewed
//!   file (identity and SHA-256). Publication uses atomic exchange exclusively;
//!   the displaced inode is verified, and a foreign replacement is preserved.
//!   Unsupported filesystems refuse apply and undo without overwriting.
//! * **Backup.** The verified backup is created with `create_new`, fsynced and
//!   never deleted by this module.
//! * **Undo** is bound to the applied card (path, device/inode, size, mtime and
//!   SHA-256 recorded in the journal) and to the verified backup.
//! * **Rollback failures are reported**, never swallowed.
//! * **No ECC guessing.** Cards with spare/ECC bytes are refused at planning.
//!
//! External writers do not cooperate with our directory lock. Any detected
//! uncertainty retains the displaced image and backup for manual inspection.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
mod discovery;
mod exchange;
mod ownership;
mod proc_scan;
mod recovery_evidence;
pub use discovery::{
    PS2_DISCOVERY_LIMIT, Ps2DiscoveryProblem, Ps2RecoveryRun, Ps2RestoreDiscovery,
    discover_ps2_restore_journals, recover_all_interrupted_ps2_restores,
};
pub use proc_scan::{ProcScanQuiescence, ProcScanReport};
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::*;
use crate::save_snapshots::SaveArtifactType;
use crate::save_snapshots::tree_restore::safety::{
    DirectorySaveBinding, EmulatorQuiescence, QuiescenceProvider, ResolvedSaveGame,
};

const JOURNAL_VERSION: u32 = 1;
const JOURNAL_MAGIC: &str = "EMUWIZ-PS2-RESTORE-JOURNAL v1 sha256=";
const MAX_JOURNAL_BYTES: u64 = 256 * 1024;
const JOURNAL_PREFIX: &str = "ps2-psu-restore-";
const STAGE_PREFIX: &str = ".emuwiz-ps2-stage-";
/// Process names that mean a PS2 emulator is (or may be) using cards.
const PS2_EMULATOR_PROCESS_NAMES: &[&str] = &["pcsx2", "aethersx2", "nethersx2", "armsx2"];

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ps2RestorePhase {
    /// Written before anything is mutated.
    Intent,
    BackupVerified,
    Staged,
    /// Written before the publishing rename.
    Publishing,
    /// Published and verified; undo is available.
    Published,
    /// The card was returned to its pre-restore content.
    RolledBack,
    /// Stopped before publication; the card was never touched.
    Abandoned,
    RollbackFailed,
    UndoIntent,
    Undone,
    UndoFailed,
    NeedsAttention,
}

impl Ps2RestorePhase {
    /// A restore or undo was in flight and needs [`recover_ps2_psu_restore`].
    #[must_use]
    pub const fn needs_recovery(self) -> bool {
        matches!(
            self,
            Self::Intent
                | Self::BackupVerified
                | Self::Staged
                | Self::Publishing
                | Self::UndoIntent
        )
    }

    /// A person must look at this; nothing is retried automatically.
    #[must_use]
    pub const fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::RollbackFailed | Self::UndoFailed | Self::NeedsAttention
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_seconds: i64,
    pub mtime_nanoseconds: i64,
}

fn identity_of(metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        mtime_seconds: metadata.mtime(),
        mtime_nanoseconds: metadata.mtime_nsec(),
    }
}

fn path_identity(path: &Path) -> Result<FileIdentity, Ps2PsuRestoreError> {
    let metadata = fs::symlink_metadata(path).map_err(restore_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "card is not a regular non-symlink file".into(),
        ));
    }
    Ok(identity_of(&metadata))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ps2RestoreJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: Ps2RestorePhase,
    pub binding: DirectorySaveBinding,
    pub card_path: PathBuf,
    pub psu_path: PathBuf,
    pub psu_sha256: String,
    pub backup_path: PathBuf,
    pub backup_sha256: Option<String>,
    pub original_sha256: String,
    pub original_size: u64,
    pub original_identity: FileIdentity,
    pub staged_path: Option<PathBuf>,
    pub staged_sha256: Option<String>,
    /// SHA-256 the card has after a successful restore (the staged image).
    pub post_sha256: Option<String>,
    /// Identity of the card file right after publication (what undo is bound to).
    pub post_identity: Option<FileIdentity>,
    #[serde(default)]
    pub undo_identity: Option<FileIdentity>,
    pub save_display_name: String,
    pub file_count: usize,
    pub detail: Option<String>,
    pub history: Vec<(Ps2RestorePhase, u64)>,
}

fn journal_bytes(journal: &Ps2RestoreJournal) -> Result<Vec<u8>, Ps2PsuRestoreError> {
    let body = serde_json::to_vec_pretty(journal).map_err(restore_error)?;
    let mut out = format!("{JOURNAL_MAGIC}{}\n", sha256_hex(&body)).into_bytes();
    out.extend_from_slice(&body);
    Ok(out)
}

/// Atomic, durable journal write (temp file, fsync, rename, fsync directory).
fn persist_journal(path: &Path, journal: &Ps2RestoreJournal) -> Result<(), Ps2PsuRestoreError> {
    let bytes = journal_bytes(journal)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(Ps2PsuRestoreError::Io(
            "journal exceeds its size bound".into(),
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("journal has no directory".into()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(restore_error)?;
    temporary.write_all(&bytes).map_err(restore_error)?;
    temporary.as_file().sync_all().map_err(restore_error)?;
    temporary
        .persist(path)
        .map_err(|error| restore_error(error.error))?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(restore_error)
}

/// Load and verify a journal. A bad checksum, truncation, wrong version or
/// impossible content is [`Ps2PsuRestoreError::JournalCorrupt`].
pub fn load_ps2_restore_journal(path: &Path) -> Result<Ps2RestoreJournal, Ps2PsuRestoreError> {
    let corrupt = |detail: &str| Ps2PsuRestoreError::JournalCorrupt(detail.to_string());
    let metadata = fs::symlink_metadata(path).map_err(|e| corrupt(&e.to_string()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_JOURNAL_BYTES
    {
        return Err(corrupt("not a regular journal file within bounds"));
    }
    let bytes = fs::read(path).map_err(|e| corrupt(&e.to_string()))?;
    let newline = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(|| corrupt("missing header"))?;
    let header = std::str::from_utf8(&bytes[..newline]).map_err(|_| corrupt("bad header"))?;
    let expected = header
        .strip_prefix(JOURNAL_MAGIC)
        .ok_or_else(|| corrupt("unknown journal format"))?;
    let body = &bytes[newline + 1..];
    if sha256_hex(body) != expected {
        return Err(corrupt("checksum mismatch"));
    }
    let journal: Ps2RestoreJournal =
        serde_json::from_slice(body).map_err(|e| corrupt(&e.to_string()))?;
    if journal.version != JOURNAL_VERSION
        || journal.operation_id.is_empty()
        || !journal.card_path.is_absolute()
        || !journal.backup_path.is_absolute()
        || !journal.psu_path.is_absolute()
    {
        return Err(corrupt("unsupported version or non-absolute paths"));
    }
    Ok(journal)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ps2RestoreJournalSummary {
    pub path: PathBuf,
    pub operation_id: Option<String>,
    pub phase: Option<Ps2RestorePhase>,
    pub card_path: Option<PathBuf>,
    /// Set when the journal could not be read; it still needs a person.
    pub error: Option<String>,
    pub needs_recovery: bool,
    pub needs_attention: bool,
    /// A completed restore whose undo is still available.
    pub undo_available: bool,
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Quiescence
// ---------------------------------------------------------------------------

/// The Save Vault binding for a memory-card restore.
#[must_use]
pub fn ps2_card_binding(card_path: &Path, save_display_name: &str) -> DirectorySaveBinding {
    DirectorySaveBinding {
        game: ResolvedSaveGame::Unique(save_display_name.to_string()),
        emulator: "pcsx2".to_string(),
        profile: card_path.to_string_lossy().into_owned(),
        artifact_type: SaveArtifactType::MemoryCard,
    }
}

fn check_quiescence(state: EmulatorQuiescence) -> Result<(), Ps2PsuRestoreError> {
    match state {
        EmulatorQuiescence::Closed => Ok(()),
        EmulatorQuiescence::Running => Err(Ps2PsuRestoreError::EmulatorRunning),
        EmulatorQuiescence::Unknown => Err(Ps2PsuRestoreError::EmulatorStateUnknown),
    }
}

// ---------------------------------------------------------------------------
// Guarded apply
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Ps2RestoreGuardOptions {
    /// Existing real directory that holds restore journals.
    pub journal_dir: PathBuf,
    pub unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ps2GuardedRestore {
    pub result: Ps2PsuRestoreResult,
    pub journal_path: PathBuf,
}

/// Points at which a test (or a future progress UI) may observe or interrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ps2RestoreStep {
    AfterIntent,
    AfterBackup,
    BeforeBackupCreate,
    BeforeStageCreate,
    AfterStaged,
    BeforeRename,
    AfterRename,
    BeforeRollbackWrite,
    BeforeUndoRename,
    AfterUndoRename,
}

type Hook<'a> = &'a dyn Fn(Ps2RestoreStep) -> std::io::Result<()>;

fn no_hook(_: Ps2RestoreStep) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(super) const SIMULATED_CRASH: &str = "simulated crash";

/// Internal control flow: an error, or (tests only) a process death that must
/// leave the disk exactly as it is.
enum Fail {
    Error(Ps2PsuRestoreError),
    #[cfg(test)]
    Crash,
}

impl From<Ps2PsuRestoreError> for Fail {
    fn from(error: Ps2PsuRestoreError) -> Self {
        Self::Error(error)
    }
}

fn step(hook: Hook<'_>, which: Ps2RestoreStep) -> Result<(), Fail> {
    hook(which).map_err(|error| {
        #[cfg(test)]
        if error.kind() == std::io::ErrorKind::Interrupted && error.to_string() == SIMULATED_CRASH {
            return Fail::Crash;
        }
        Fail::Error(Ps2PsuRestoreError::Io(error.to_string()))
    })
}

#[cfg(test)]
thread_local! {
    static EXCHANGE_ERRORS: std::cell::RefCell<std::collections::VecDeque<i32>> = const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// Atomically swap two paths (`renameat2(RENAME_EXCHANGE)`).
fn rename_exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(errno) = EXCHANGE_ERRORS.with(|errors| errors.borrow_mut().pop_front()) {
        if errno != 0 {
            return Err(std::io::Error::from_raw_os_error(errno));
        }
    }
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let a = CString::new(a.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("path contains NUL"))?;
    let b = CString::new(b.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("path contains NUL"))?;
    // SAFETY: both pointers are valid NUL-terminated strings for the call.
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn exchange_unsupported(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS | libc::EINVAL | libc::ENOTSUP)
    )
}

fn undo_temp_path(card_dir: &Path, operation_id: &str) -> PathBuf {
    card_dir.join(format!("{STAGE_PREFIX}undo-{operation_id}.tmp"))
}

/// Remove this operation's own staged/displaced temp file, but only when its
/// content is one this operation produced.
fn remove_owned_temp(journal: &Ps2RestoreJournal) {
    let Some(staged) = &journal.staged_path else {
        return;
    };
    if staged == &journal.card_path || staged == &journal.backup_path {
        return;
    }
    if !staged
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(STAGE_PREFIX))
    {
        return;
    }
    let Ok(bytes) = fs::read(staged) else {
        return;
    };
    let sha = sha256_hex(&bytes);
    let identity = path_identity(staged).ok();
    if identity.is_some()
        && (identity == path_identity(&journal.card_path).ok()
            || identity == path_identity(&journal.backup_path).ok())
    {
        return;
    }
    if (Some(sha.as_str()) == journal.staged_sha256.as_deref()
        && identity.is_some()
        && identity == journal.post_identity)
        || (sha == journal.original_sha256 && identity == Some(journal.original_identity))
    {
        let _ = fs::remove_file(staged);
    }
}

fn clean_undo_temp(journal: &Ps2RestoreJournal) {
    let Some(parent) = journal.card_path.parent() else {
        return;
    };
    let temp = undo_temp_path(parent, &journal.operation_id);
    if temp == journal.card_path || temp == journal.backup_path {
        return;
    }
    let identity = path_identity(&temp).ok();
    if identity.is_some()
        && (identity == path_identity(&journal.card_path).ok()
            || identity == path_identity(&journal.backup_path).ok())
    {
        return;
    }
    if identity.is_some()
        && fs::read(&temp).is_ok_and(|bytes| {
            let sha = sha256_hex(&bytes);
            (identity == journal.undo_identity && sha == journal.original_sha256)
                || (identity == journal.post_identity
                    && Some(sha.as_str()) == journal.post_sha256.as_deref())
        })
    {
        let _ = fs::remove_file(temp);
    }
}

static OPERATION_COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_operation_id(seconds: u64) -> String {
    format!(
        "{seconds:010x}-{:x}-{:04x}",
        std::process::id(),
        OPERATION_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn validate_journal_dir(dir: &Path) -> Result<(), Ps2PsuRestoreError> {
    let metadata = fs::symlink_metadata(dir).map_err(restore_error)?;
    if !dir.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "journal directory must be an absolute real directory".into(),
        ));
    }
    validate_restore_parent(&dir.join("x"))
}

struct Run<'a> {
    journal: Ps2RestoreJournal,
    journal_path: PathBuf,
    seconds: u64,

    original: Vec<u8>,
    renamed: bool,
    quiescence: &'a dyn QuiescenceProvider,
}

impl Run<'_> {
    fn set_phase(&mut self, phase: Ps2RestorePhase) -> Result<(), Ps2PsuRestoreError> {
        self.journal.phase = phase;
        self.journal.history.push((phase, self.seconds));
        persist_journal(&self.journal_path, &self.journal)
    }

    fn quiet(&self) -> Result<(), Ps2PsuRestoreError> {
        check_quiescence(self.quiescence.observe(&self.journal.binding))
    }

    /// The reviewed card, unchanged: same inode/size/mtime and same bytes.
    fn card_is_reviewed(&self) -> Result<(), Ps2PsuRestoreError> {
        if path_identity(&self.journal.card_path)? != self.journal.original_identity {
            return Err(Ps2PsuRestoreError::CardChanged);
        }
        let bytes = read_source_card(&self.journal.card_path)
            .map_err(|e| Ps2PsuRestoreError::Io(e.to_string()))?;
        if sha256_hex(&bytes) != self.journal.original_sha256 {
            return Err(Ps2PsuRestoreError::CardChanged);
        }
        Ok(())
    }
}

/// Guarded restore. See the module documentation for the guarantees.
pub fn apply_ps2_psu_restore_guarded(
    plan: &Ps2PsuRestorePlan,
    quiescence: &dyn QuiescenceProvider,
    options: &Ps2RestoreGuardOptions,
) -> Result<Ps2GuardedRestore, Ps2PsuRestoreError> {
    apply_with_hook(plan, quiescence, options, &no_hook)
}

pub(crate) fn apply_with_hook(
    plan: &Ps2PsuRestorePlan,
    quiescence: &dyn QuiescenceProvider,
    options: &Ps2RestoreGuardOptions,
    hook: Hook<'_>,
) -> Result<Ps2GuardedRestore, Ps2PsuRestoreError> {
    // Everything that can be refused without side effects happens first.
    validate_journal_dir(&options.journal_dir)?;
    validate_source_path(&plan.source_card_path)
        .map_err(|error| Ps2PsuRestoreError::InvalidPlan(error.to_string()))?;
    validate_backup_path(&plan.backup_path, &plan.source_card_path)?;
    restore_require_data_only_pages(&plan.geometry)?;
    let _operation_lock = ownership::lock(&plan.source_card_path)?;
    let binding = ps2_card_binding(&plan.source_card_path, &plan.save_display_name);
    check_quiescence(quiescence.observe(&binding))?;
    let psu = fs::read(&plan.source_psu_path).map_err(restore_error)?;
    if sha256_hex(&psu) != plan.source_psu_sha256 {
        return Err(Ps2PsuRestoreError::SourceChanged);
    }
    let mut pinned = fs::File::open(&plan.source_card_path).map_err(restore_error)?;
    let pinned_identity = identity_of(&ownership::validate(&pinned)?);
    if path_identity(&plan.source_card_path)? != pinned_identity {
        return Err(Ps2PsuRestoreError::CardChanged);
    }
    let mut original = Vec::new();
    (&mut pinned)
        .take(PS2_MAX_CARD_BYTES as u64 + 1)
        .read_to_end(&mut original)
        .map_err(restore_error)?;
    if sha256_hex(&original) != plan.target_card_sha256
        || original.len() as u64 != plan.target_card_size_bytes
    {
        return Err(Ps2PsuRestoreError::CardChanged);
    }

    let operation_id = new_operation_id(options.unix_seconds);
    let journal_path = options
        .journal_dir
        .join(format!("{JOURNAL_PREFIX}{operation_id}.json"));
    let journal = Ps2RestoreJournal {
        version: JOURNAL_VERSION,
        operation_id,
        phase: Ps2RestorePhase::Intent,
        binding,
        card_path: plan.source_card_path.clone(),
        psu_path: plan.source_psu_path.clone(),
        psu_sha256: plan.source_psu_sha256.clone(),
        backup_path: plan.backup_path.clone(),
        backup_sha256: None,
        original_sha256: plan.target_card_sha256.clone(),
        original_size: plan.target_card_size_bytes,
        original_identity: pinned_identity,
        staged_path: None,
        staged_sha256: None,
        post_sha256: None,
        post_identity: None,
        undo_identity: None,
        save_display_name: plan.save_display_name.clone(),
        file_count: plan.file_count,
        detail: None,
        history: vec![(Ps2RestorePhase::Intent, options.unix_seconds)],
    };
    // Durable intent, before the first mutation.
    persist_journal(&journal_path, &journal)?;
    let mut run = Run {
        journal,
        journal_path,
        seconds: options.unix_seconds,
        original,
        renamed: false,
        quiescence,
    };
    match execute(plan, &mut run, &psu, hook) {
        Ok(result) => Ok(Ps2GuardedRestore {
            result,
            journal_path: run.journal_path.clone(),
        }),
        #[cfg(test)]
        Err(Fail::Crash) => Err(Ps2PsuRestoreError::Io(SIMULATED_CRASH.into())),
        Err(Fail::Error(error)) => Err(unwind(plan, &mut run, error, hook)),
    }
}

fn execute(
    plan: &Ps2PsuRestorePlan,
    run: &mut Run<'_>,
    psu: &[u8],
    hook: Hook<'_>,
) -> Result<Ps2PsuRestoreResult, Fail> {
    step(hook, Ps2RestoreStep::AfterIntent)?;

    step(hook, Ps2RestoreStep::BeforeBackupCreate)?;
    // Backup path is already durable in Intent. Partial backups are retained and flagged.
    let mut backup = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&plan.backup_path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ps2PsuRestoreError::BackupExists(plan.backup_path.clone())
            } else {
                restore_error(error)
            }
        })?;
    let written = backup
        .write_all(&run.original)
        .and_then(|()| backup.sync_all());
    drop(backup);
    if let Err(error) = written {
        return Err(restore_error(error).into());
    }
    if let Some(parent) = plan.backup_path.parent() {
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(restore_error)?;
    }
    let backup_bytes = fs::read(&plan.backup_path).map_err(restore_error)?;
    let backup_sha256 = sha256_hex(&backup_bytes);
    if backup_bytes.len() != run.original.len() || backup_sha256 != run.journal.original_sha256 {
        return Err(Ps2PsuRestoreError::BackupVerificationFailed.into());
    }
    run.journal.backup_sha256 = Some(backup_sha256.clone());
    run.set_phase(Ps2RestorePhase::BackupVerified)?;
    step(hook, Ps2RestoreStep::AfterBackup)?;

    // Stage the full candidate next to the card and verify it before any
    // publication. The live card is untouched until the rename.
    let staged_bytes = build_restored_card(plan, &run.original)?;
    let staged_sha256 = sha256_hex(&staged_bytes);
    let card_dir = plan
        .source_card_path
        .parent()
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("card has no parent".into()))?;
    let staged_path = card_dir.join(format!("{STAGE_PREFIX}{}.tmp", run.journal.operation_id));
    if staged_path == plan.source_card_path || staged_path == plan.backup_path {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "Staging path collides with the card or backup; publication refused".into(),
        )
        .into());
    }
    run.journal.staged_path = Some(staged_path.clone());
    run.journal.staged_sha256 = Some(staged_sha256.clone());
    run.journal.post_sha256 = Some(staged_sha256.clone());
    run.set_phase(Ps2RestorePhase::BackupVerified)?;
    step(hook, Ps2RestoreStep::BeforeStageCreate)?;
    {
        let permissions = fs::metadata(&plan.source_card_path).map_err(restore_error)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_path)
            .map_err(restore_error)?;
        file.write_all(&staged_bytes).map_err(restore_error)?;
        ownership::preserve(&file, &permissions)?;
        file.sync_all().map_err(restore_error)?;
    }
    let inspected = inspect_memory_card(&staged_path).map_err(Ps2PsuRestoreError::Io)?;
    if inspected.card_size_bytes != plan.target_card_size_bytes
        || inspected.health != MemoryCardHealth::Healthy
        || !restore_contains_expected_at(&inspected, plan, &staged_path)
    {
        return Err(Ps2PsuRestoreError::VerificationFailed(
            "the staged card did not match the reviewed package".into(),
        )
        .into());
    }
    run.journal.post_identity = Some(path_identity(&staged_path)?);
    run.set_phase(Ps2RestorePhase::Staged)?;
    step(hook, Ps2RestoreStep::AfterStaged)?;

    // Publication gate: journal the intent to publish, then recheck everything
    // one more time, immediately before the rename.
    run.journal.post_identity = Some(path_identity(&staged_path)?);
    run.set_phase(Ps2RestorePhase::Publishing)?;
    run.quiet()?;
    if sha256_hex(&fs::read(&plan.source_psu_path).map_err(restore_error)?)
        != run.journal.psu_sha256
        || sha256_hex(psu) != run.journal.psu_sha256
    {
        return Err(Ps2PsuRestoreError::SourceChanged.into());
    }
    run.card_is_reviewed()?;
    step(hook, Ps2RestoreStep::BeforeRename)?;

    exchange::publish(
        &staged_path,
        &plan.source_card_path,
        run.journal.original_identity,
        &run.journal.original_sha256,
        &staged_sha256,
        &mut run.renamed,
        hook,
        Some(Ps2RestoreStep::AfterRename),
    )?;
    fs::File::open(card_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(restore_error)?;

    let live = read_source_card(&plan.source_card_path)
        .map_err(|error| Ps2PsuRestoreError::Io(error.to_string()))?;
    let after = inspect_memory_card(&plan.source_card_path).map_err(Ps2PsuRestoreError::Io)?;
    if path_identity(&plan.source_card_path)?
        != run.journal.post_identity.ok_or_else(|| {
            Ps2PsuRestoreError::RecoveryRequired("Missing publication identity".into())
        })?
        || sha256_hex(&live) != staged_sha256
        || after.card_size_bytes != plan.target_card_size_bytes
        || after.health != MemoryCardHealth::Healthy
        || !restore_contains_expected(&after, plan)
    {
        return Err(Ps2PsuRestoreError::VerificationFailed(
            "card structure or restored save did not match the reviewed package".into(),
        )
        .into());
    }
    run.journal.post_identity = Some(path_identity(&plan.source_card_path)?);
    run.set_phase(Ps2RestorePhase::Published)?;
    remove_owned_temp(&run.journal);
    Ok(Ps2PsuRestoreResult {
        source_card_path: plan.source_card_path.clone(),
        source_psu_path: plan.source_psu_path.clone(),
        backup_path: plan.backup_path.clone(),
        backup_sha256,
        original_card_sha256: run.journal.original_sha256.clone(),
        post_restore_card_sha256: staged_sha256,
        card_size_bytes: staged_bytes.len() as u64,
        save_display_name: plan.save_display_name.clone(),
        file_count: plan.file_count,
    })
}

/// A failure happened: leave the card as it was and say so in the journal.
fn unwind(
    plan: &Ps2PsuRestorePlan,
    run: &mut Run<'_>,
    cause: Ps2PsuRestoreError,
    hook: Hook<'_>,
) -> Ps2PsuRestoreError {
    let note = |run: &mut Run<'_>, phase, text: String| {
        run.journal.detail = Some(text);
        run.set_phase(phase)
    };
    if !run.renamed {
        // Never published: the card was not written.
        remove_owned_temp(&run.journal);
        let incomplete = run
            .journal
            .staged_path
            .as_ref()
            .is_some_and(|path| !recovery_evidence::confirmed_absent(path))
            || (!recovery_evidence::confirmed_absent(&run.journal.backup_path)
                && run.journal.backup_sha256.is_none());
        let phase = if incomplete {
            Ps2RestorePhase::NeedsAttention
        } else {
            Ps2RestorePhase::Abandoned
        };
        if let Err(error) = note(run, phase, cause.to_string()) {
            return Ps2PsuRestoreError::RecoveryRequired(format!(
                "{cause}; journal update failed: {error}. Journal retained at {}",
                run.journal_path.display()
            ));
        }
        return cause;
    }
    // A failed exchange reversal must never be retried by an unconditional write.
    let rollback = if matches!(cause, Ps2PsuRestoreError::RecoveryRequired(_)) {
        Err(cause.to_string())
    } else {
        exchange::rollback(run, hook)
    };
    match rollback {
        Ok(()) => {
            if let Err(error) = note(
                run,
                Ps2RestorePhase::RolledBack,
                format!("{cause}; the card was returned to its pre-restore content"),
            ) {
                return Ps2PsuRestoreError::RecoveryRequired(format!(
                    "{cause}; rollback completed but journal update failed: {error}. Journal: {}",
                    run.journal_path.display()
                ));
            }
            cause
        }
        Err(mut detail) => {
            if let Err(error) = note(
                run,
                Ps2RestorePhase::RollbackFailed,
                format!("{cause}; rollback failed: {detail}"),
            ) {
                detail.push_str(&format!("; journal update failed: {error}"));
            }
            Ps2PsuRestoreError::RollbackFailed {
                detail,
                backup_path: plan.backup_path.clone(),
                journal_path: Some(run.journal_path.clone()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Guarded undo
// ---------------------------------------------------------------------------

/// Undo a published restore. Bound to the applied card (identity and SHA-256
/// recorded at publication) and to the verified backup. Refused if the card was
/// edited, replaced or is in use since.
pub fn undo_ps2_psu_restore_guarded(
    journal_path: &Path,
    quiescence: &dyn QuiescenceProvider,
    unix_seconds: u64,
) -> Result<(), Ps2PsuRestoreError> {
    undo_with_hook(journal_path, quiescence, unix_seconds, &no_hook)
}

pub(crate) fn undo_with_hook(
    journal_path: &Path,
    quiescence: &dyn QuiescenceProvider,
    unix_seconds: u64,
    hook: Hook<'_>,
) -> Result<(), Ps2PsuRestoreError> {
    let journal = load_ps2_restore_journal(journal_path)?;
    let _operation_lock = ownership::lock(&journal.card_path)?;
    let locked_card = journal.card_path.clone();
    let journal = load_ps2_restore_journal(journal_path)?;
    if journal.card_path != locked_card {
        return Err(Ps2PsuRestoreError::JournalCorrupt(
            "Journal binding changed while acquiring the operation lock".into(),
        ));
    }

    if journal.phase != Ps2RestorePhase::Published {
        return Err(match journal.phase {
            Ps2RestorePhase::Undone => Ps2PsuRestoreError::StaleUndo,
            phase => Ps2PsuRestoreError::RecoveryRequired(format!(
                "undo is only available for a published restore (journal is {phase:?})"
            )),
        });
    }
    let (Some(post_sha), Some(post_identity), Some(backup_sha)) = (
        journal.post_sha256.clone(),
        journal.post_identity,
        journal.backup_sha256.clone(),
    ) else {
        return Err(Ps2PsuRestoreError::JournalCorrupt(
            "published journal lacks its recorded output".into(),
        ));
    };
    check_quiescence(quiescence.observe(&journal.binding))?;
    validate_source_path(&journal.card_path)
        .map_err(|error| Ps2PsuRestoreError::InvalidPlan(error.to_string()))?;
    let mut pinned = fs::File::open(&journal.card_path).map_err(restore_error)?;
    let identity = identity_of(&ownership::validate(&pinned)?);
    if identity != post_identity || path_identity(&journal.card_path)? != post_identity {
        return Err(Ps2PsuRestoreError::StaleUndo);
    }
    let mut current = Vec::new();
    (&mut pinned)
        .take(PS2_MAX_CARD_BYTES as u64 + 1)
        .read_to_end(&mut current)
        .map_err(restore_error)?;
    if sha256_hex(&current) != post_sha {
        return Err(Ps2PsuRestoreError::StaleUndo);
    }
    let backup_meta = fs::symlink_metadata(&journal.backup_path).map_err(restore_error)?;
    if !backup_meta.is_file() || backup_meta.file_type().is_symlink() {
        return Err(Ps2PsuRestoreError::BackupVerificationFailed);
    }
    let backup = fs::read(&journal.backup_path).map_err(restore_error)?;
    if sha256_hex(&backup) != backup_sha || backup_sha != journal.original_sha256 {
        return Err(Ps2PsuRestoreError::BackupVerificationFailed);
    }

    let mut journal = journal;
    let set = |journal: &mut Ps2RestoreJournal, phase, detail: Option<String>| {
        journal.phase = phase;
        journal.detail = detail;
        journal.history.push((phase, unix_seconds));
        persist_journal(journal_path, journal)
    };
    let card_dir = journal
        .card_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("card has no parent".into()))?;
    let undo_temp = undo_temp_path(&card_dir, &journal.operation_id);
    if undo_temp == journal.card_path || undo_temp == journal.backup_path {
        return Err(Ps2PsuRestoreError::InvalidPlan(
            "Undo staging path collides with the card or backup; Undo refused".into(),
        ));
    }
    set(&mut journal, Ps2RestorePhase::UndoIntent, None)?;
    let mut undo_published = false;
    let result = (|| -> Result<(), Fail> {
        check_quiescence(quiescence.observe(&journal.binding))?;
        if path_identity(&journal.card_path)? != post_identity {
            return Err(Ps2PsuRestoreError::StaleUndo.into());
        }
        let live = read_source_card(&journal.card_path)
            .map_err(|e| Ps2PsuRestoreError::Io(e.to_string()))?;
        if sha256_hex(&live) != post_sha {
            return Err(Ps2PsuRestoreError::StaleUndo.into());
        }
        // Stage the original next to the card (mode preserved), then swap it in.
        let permissions = fs::metadata(&journal.card_path).map_err(restore_error)?;
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&undo_temp)
                .map_err(restore_error)?;
            file.write_all(&backup).map_err(restore_error)?;
            ownership::preserve(&file, &permissions)?;
            file.sync_all().map_err(restore_error)?;
        }
        journal.undo_identity = Some(path_identity(&undo_temp)?);
        set(&mut journal, Ps2RestorePhase::UndoIntent, None)?;
        step(hook, Ps2RestoreStep::BeforeUndoRename)?;
        match exchange::publish(
            &undo_temp,
            &journal.card_path,
            post_identity,
            &post_sha,
            &journal.original_sha256,
            &mut undo_published,
            hook,
            Some(Ps2RestoreStep::AfterUndoRename),
        ) {
            Err(Fail::Error(Ps2PsuRestoreError::CardChanged)) => {
                return Err(Ps2PsuRestoreError::StaleUndo.into());
            }
            other => other?,
        }
        fs::File::open(&card_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(restore_error)?;
        let restored = read_source_card(&journal.card_path)
            .map_err(|e| Ps2PsuRestoreError::Io(e.to_string()))?;
        if path_identity(&journal.card_path).ok() != journal.undo_identity
            || sha256_hex(&restored) != journal.original_sha256
        {
            return Err(Ps2PsuRestoreError::VerificationFailed(
                "undo did not restore the original card bytes".into(),
            )
            .into());
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            set(&mut journal, Ps2RestorePhase::Undone, None)?;
            fs::remove_file(&undo_temp).map_err(restore_error)
        }
        #[cfg(test)]
        Err(Fail::Crash) => Err(Ps2PsuRestoreError::Io(SIMULATED_CRASH.into())),
        Err(Fail::Error(error)) => {
            if !undo_published {
                clean_undo_temp(&journal);
            }
            // A stale-undo refusal means the card was left exactly as found (a late
            // foreign file, if any, was swapped back untouched): not a failure.
            if !undo_published {
                set(
                    &mut journal,
                    if !recovery_evidence::confirmed_absent(&undo_temp) {
                        Ps2RestorePhase::NeedsAttention
                    } else {
                        Ps2RestorePhase::Published
                    },
                    Some(format!(
                        "undo refused: {error}; inspect retained staging artifacts if present"
                    )),
                )?;
                return Err(error);
            }
            // Is the card still the applied restore (nothing changed), or did
            // something go wrong after we wrote?
            let live = read_source_card(&journal.card_path)
                .ok()
                .map(|b| sha256_hex(&b));
            if live.as_deref() == Some(post_sha.as_str()) {
                set(
                    &mut journal,
                    Ps2RestorePhase::Published,
                    Some(format!("undo refused: {error}")),
                )?;
                Err(error)
            } else {
                let mut detail = format!(
                    "{error}; the card may not match either state. Displaced file retained at {}",
                    undo_temp.display()
                );
                if let Err(journal_error) = set(
                    &mut journal,
                    Ps2RestorePhase::UndoFailed,
                    Some(detail.clone()),
                ) {
                    detail.push_str(&format!("; journal update failed: {journal_error}"));
                }
                Err(Ps2PsuRestoreError::RollbackFailed {
                    detail,
                    backup_path: journal.backup_path.clone(),
                    journal_path: Some(journal_path.to_path_buf()),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ps2RecoveryOutcome {
    /// Nothing was in flight.
    NothingToDo(Ps2RestorePhase),
    /// Stopped before publication; the card was never touched.
    AbandonedBeforePublication,
    /// The publishing rename did not happen; the card is still the original.
    NotPublished,
    /// The rename happened and the card is the verified restored image.
    ConfirmedPublished,
    /// An interrupted undo had not yet changed the card.
    UndoNotApplied,
    /// An interrupted undo had completed.
    UndoCompleted,
    /// Neither expected state; nothing was written. A person must look.
    NeedsAttention(String),
}

/// Judge an interrupted operation after a restart. **Never writes the card**:
/// it only reads it, removes this operation's own staged temp file, and updates
/// the journal. Anything it cannot prove is `NeedsAttention`.
pub fn recover_ps2_psu_restore(
    journal_path: &Path,
    unix_seconds: u64,
) -> Result<Ps2RecoveryOutcome, Ps2PsuRestoreError> {
    let mut journal = load_ps2_restore_journal(journal_path)?;
    let phase = journal.phase;
    if !phase.needs_recovery() {
        return Ok(if phase.needs_attention() {
            Ps2RecoveryOutcome::NeedsAttention(
                journal
                    .detail
                    .clone()
                    .unwrap_or_else(|| format!("{phase:?}")),
            )
        } else {
            Ps2RecoveryOutcome::NothingToDo(phase)
        });
    }
    let _operation_lock = ownership::lock(&journal.card_path)?;
    let locked_card = journal.card_path.clone();
    journal = load_ps2_restore_journal(journal_path)?;
    if journal.card_path != locked_card {
        return Err(Ps2PsuRestoreError::JournalCorrupt(
            "Journal binding changed while acquiring the operation lock".into(),
        ));
    }
    let phase = journal.phase;
    if !phase.needs_recovery() {
        return Ok(if phase.needs_attention() {
            Ps2RecoveryOutcome::NeedsAttention(
                journal
                    .detail
                    .clone()
                    .unwrap_or_else(|| "Manual inspection required".into()),
            )
        } else {
            Ps2RecoveryOutcome::NothingToDo(phase)
        });
    }

    if let Some(detail) = recovery_evidence::problem(&journal) {
        // Keep the exact in-flight receipt for restart visibility and safe retry.
        return Ok(Ps2RecoveryOutcome::NeedsAttention(detail));
    }
    let live_sha = match read_source_card(&journal.card_path) {
        Ok(bytes) => Some(sha256_hex(&bytes)),
        Err(error) => {
            return Ok(Ps2RecoveryOutcome::NeedsAttention(format!(
                "Live card evidence unavailable: {error}; receipt retained for retry"
            )));
        }
    };
    let remove_staged = remove_owned_temp;
    let finish =
        |journal: &mut Ps2RestoreJournal, phase, detail: &str| -> Result<(), Ps2PsuRestoreError> {
            journal.phase = phase;
            journal.detail = Some(detail.to_string());
            journal.history.push((phase, unix_seconds));
            persist_journal(journal_path, journal)
        };
    let live = live_sha.as_deref();
    let original = Some(journal.original_sha256.as_str());
    let post = journal.post_sha256.as_deref();
    match phase {
        Ps2RestorePhase::Intent | Ps2RestorePhase::BackupVerified | Ps2RestorePhase::Staged => {
            if live == original
                && path_identity(&journal.card_path).ok() == Some(journal.original_identity)
            {
                remove_staged(&journal);
                if let Some(path) = &journal.staged_path
                    && !recovery_evidence::confirmed_absent(path)
                {
                    let detail = format!(
                        "Staged artifact identity or cleanup could not be verified. File retained at {}. The live card was not written by recovery.",
                        path.display()
                    );
                    return Ok(Ps2RecoveryOutcome::NeedsAttention(detail));
                }
                finish(
                    &mut journal,
                    Ps2RestorePhase::Abandoned,
                    "interrupted before publication; the card was not touched",
                )?;
                Ok(Ps2RecoveryOutcome::AbandonedBeforePublication)
            } else {
                let detail = "the card differs from the reviewed original although no publication was recorded".to_string();
                finish(&mut journal, Ps2RestorePhase::NeedsAttention, &detail)?;
                Ok(Ps2RecoveryOutcome::NeedsAttention(detail))
            }
        }
        Ps2RestorePhase::Publishing => {
            if live == original
                && path_identity(&journal.card_path).ok() == Some(journal.original_identity)
            {
                remove_staged(&journal);
                if let Some(path) = &journal.staged_path
                    && !recovery_evidence::confirmed_absent(path)
                {
                    let detail = format!(
                        "Staged artifact identity or cleanup could not be verified. File retained at {}. The live card was not written by recovery.",
                        path.display()
                    );
                    return Ok(Ps2RecoveryOutcome::NeedsAttention(detail));
                }
                finish(
                    &mut journal,
                    Ps2RestorePhase::Abandoned,
                    "interrupted before the publishing rename; the card is still the original",
                )?;
                Ok(Ps2RecoveryOutcome::NotPublished)
            } else if live.is_some() && live == post {
                if journal.staged_path.as_ref().is_some_and(|path| {
                    !recovery_evidence::confirmed_absent(path)
                        && path_identity(path).ok() != Some(journal.original_identity)
                }) || journal.post_identity != path_identity(&journal.card_path).ok()
                    || fs::read(&journal.backup_path)
                        .ok()
                        .is_none_or(|bytes| sha256_hex(&bytes) != journal.original_sha256)
                {
                    let detail = "Publication identity or backup could not be verified; all artifacts were retained.".to_string();
                    finish(&mut journal, Ps2RestorePhase::NeedsAttention, &detail)?;
                    return Ok(Ps2RecoveryOutcome::NeedsAttention(detail));
                }
                let inspected =
                    inspect_memory_card(&journal.card_path).map_err(Ps2PsuRestoreError::Io)?;
                if inspected.health != MemoryCardHealth::Healthy {
                    let detail = "the restored card is not structurally healthy".to_string();
                    finish(&mut journal, Ps2RestorePhase::NeedsAttention, &detail)?;
                    return Ok(Ps2RecoveryOutcome::NeedsAttention(detail));
                }
                journal.post_identity = Some(path_identity(&journal.card_path)?);
                // After an exchange the staged path holds the displaced original.
                remove_staged(&journal);
                if let Some(path) = &journal.staged_path
                    && !recovery_evidence::confirmed_absent(path)
                {
                    return Ok(Ps2RecoveryOutcome::NeedsAttention("Displaced artifact cleanup could not be verified; receipt retained for retry".into()));
                }
                finish(
                    &mut journal,
                    Ps2RestorePhase::Published,
                    "publication completed before the interruption and was verified on restart",
                )?;
                Ok(Ps2RecoveryOutcome::ConfirmedPublished)
            } else {
                let detail =
                    "the card is neither the original nor the restored image (changed externally?)"
                        .to_string();
                finish(&mut journal, Ps2RestorePhase::NeedsAttention, &detail)?;
                Ok(Ps2RecoveryOutcome::NeedsAttention(detail))
            }
        }
        Ps2RestorePhase::UndoIntent => {
            if live.is_some()
                && live == post
                && path_identity(&journal.card_path).ok() == journal.post_identity
            {
                clean_undo_temp(&journal);
                if let Some(parent) = journal.card_path.parent()
                    && !recovery_evidence::confirmed_absent(&undo_temp_path(
                        parent,
                        &journal.operation_id,
                    ))
                {
                    return Ok(Ps2RecoveryOutcome::NeedsAttention(
                        "Undo artifact cleanup could not be verified; receipt retained for retry"
                            .into(),
                    ));
                }
                finish(
                    &mut journal,
                    Ps2RestorePhase::Published,
                    "interrupted undo had not changed the card; undo is available again",
                )?;
                Ok(Ps2RecoveryOutcome::UndoNotApplied)
            } else if live == original
                && path_identity(&journal.card_path).ok() == journal.undo_identity
            {
                clean_undo_temp(&journal);
                if let Some(parent) = journal.card_path.parent()
                    && !recovery_evidence::confirmed_absent(&undo_temp_path(
                        parent,
                        &journal.operation_id,
                    ))
                {
                    return Ok(Ps2RecoveryOutcome::NeedsAttention(
                        "Undo artifact cleanup could not be verified; receipt retained for retry"
                            .into(),
                    ));
                }
                finish(&mut journal, Ps2RestorePhase::Undone, "undo had completed")?;
                Ok(Ps2RecoveryOutcome::UndoCompleted)
            } else {
                let detail = "the card is neither the restored image nor the original after an interrupted undo".to_string();
                finish(&mut journal, Ps2RestorePhase::NeedsAttention, &detail)?;
                Ok(Ps2RecoveryOutcome::NeedsAttention(detail))
            }
        }
        _ => Ok(Ps2RecoveryOutcome::NothingToDo(phase)),
    }
}

// ---------------------------------------------------------------------------
// Application defaults
// ---------------------------------------------------------------------------

/// Resolve the journal directory without creating it (read-only viewing).
pub fn default_ps2_restore_journal_dir() -> Result<PathBuf, Ps2PsuRestoreError> {
    let dir = crate::app_dirs::data_path("ps2-restore-journals")
        .map_err(|error| Ps2PsuRestoreError::Io(error.to_string()))?;
    Ok(dir)
}

/// Guard options for "now" with the default journal directory.
pub fn default_ps2_restore_guard_options() -> Result<Ps2RestoreGuardOptions, Ps2PsuRestoreError> {
    let journal_dir = default_ps2_restore_journal_dir()?;
    fs::create_dir_all(&journal_dir).map_err(restore_error)?;
    Ok(Ps2RestoreGuardOptions {
        journal_dir,
        unix_seconds: unix_now(),
    })
}

#[must_use]
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests;
