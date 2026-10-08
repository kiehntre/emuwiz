//! Guarded PS2 PSU restore: the existing executor in the parent module computes
//! and writes the card; this module wraps it with the guarantees the Save Vault
//! directory restore already has.
//!
//! * **Quiescence.** Mutation is refused unless the Save Vault policy
//!   ([`QuiescenceProvider`]) reports `Closed`. `Running` and `Unknown` refuse,
//!   and the check is repeated immediately before publication.
//! * **Durable intent.** A checksummed journal is written (atomically, with
//!   fsync) before the first mutation and at every phase change, so an
//!   interrupted restore can be found and judged after a restart.
//! * **Recheck before publication.** The live card must still be the reviewed
//!   file (identity and SHA-256). After the rename, the old inode that was
//!   pinned before staging is read again: if an external edit slipped into the
//!   last moment, that newest pre-publication content is put back, not lost.
//! * **Backup.** The verified backup is created with `create_new`, fsynced and
//!   never deleted by this module.
//! * **Undo** is bound to the applied card (path, device/inode, size, mtime and
//!   SHA-256 recorded in the journal) and to the verified backup.
//! * **Rollback failures are reported**, never swallowed.
//! * **No ECC guessing.** Cards with spare/ECC bytes are refused at planning.
//!
//! Remaining limitation: the pinned-inode check detects, but cannot prevent, an
//! external in-place write that lands between the final recheck and the rename;
//! it is repaired by restoring the newest content.

use std::io::{Read, Seek, SeekFrom, Write};
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
    let temporary = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("journal")
    ));
    let write = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    write.map_err(|error| {
        let _ = fs::remove_file(&temporary);
        restore_error(error)
    })
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
}

/// Restart discovery: every restore journal in `journal_dir`, including corrupt
/// ones. Read-only.
#[must_use]
pub fn discover_ps2_restore_journals(journal_dir: &Path) -> Vec<Ps2RestoreJournalSummary> {
    let Ok(entries) = fs::read_dir(journal_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(JOURNAL_PREFIX) && name.ends_with(".json"))
        })
        .take(1024)
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| match load_ps2_restore_journal(&path) {
            Ok(journal) => Ps2RestoreJournalSummary {
                operation_id: Some(journal.operation_id.clone()),
                phase: Some(journal.phase),
                card_path: Some(journal.card_path.clone()),
                error: None,
                needs_recovery: journal.phase.needs_recovery(),
                needs_attention: journal.phase.needs_attention(),
                undo_available: journal.phase == Ps2RestorePhase::Published,
                path,
            },
            Err(error) => Ps2RestoreJournalSummary {
                path,
                operation_id: None,
                phase: None,
                card_path: None,
                error: Some(error.to_string()),
                needs_recovery: true,
                needs_attention: true,
                undo_available: false,
            },
        })
        .collect()
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

/// Linux `/proc` observation of PS2 emulators.
///
/// `Running` when any process is named like a known PS2 emulator or holds the
/// card file open. `Closed` only when the whole `/proc` listing was read
/// without error and neither condition was found. Any unreadable process of the
/// current user, or an unreadable `/proc`, is `Unknown`.
///
/// Limits (stated, not hidden): processes of *other* users cannot be inspected
/// for open descriptors, so only their command line is considered; this is not
/// a kernel-level file lock, so something can still open the card after the
/// observation.
pub struct ProcScanQuiescence {
    proc_root: PathBuf,
}

impl ProcScanQuiescence {
    #[must_use]
    pub fn new() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
        }
    }

    /// Scan a different proc-like tree (tests).
    #[must_use]
    pub fn with_root(proc_root: PathBuf) -> Self {
        Self { proc_root }
    }
}

impl Default for ProcScanQuiescence {
    fn default() -> Self {
        Self::new()
    }
}

impl QuiescenceProvider for ProcScanQuiescence {
    fn observe(&self, binding: &DirectorySaveBinding) -> EmulatorQuiescence {
        let card = PathBuf::from(&binding.profile);
        let canonical = fs::canonicalize(&card).ok();
        let Ok(entries) = fs::read_dir(&self.proc_root) else {
            return EmulatorQuiescence::Unknown;
        };
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        let mut unknown = false;
        for entry in entries {
            let Ok(entry) = entry else {
                unknown = true;
                continue;
            };
            let name = entry.file_name();
            if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let dir = entry.path();
            let owner = fs::metadata(&dir).map(|m| m.uid()).ok();
            for leaf in ["comm", "cmdline"] {
                match fs::read(dir.join(leaf)) {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
                        if PS2_EMULATOR_PROCESS_NAMES.iter().any(|n| text.contains(n)) {
                            return EmulatorQuiescence::Running;
                        }
                    }
                    // The process exited while we were scanning.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(_) => unknown = true,
                }
            }
            match fs::read_dir(dir.join("fd")) {
                Ok(fds) => {
                    for fd in fds.flatten() {
                        if let Ok(target) = fs::read_link(fd.path())
                            && (target == card || canonical.as_ref() == Some(&target))
                        {
                            return EmulatorQuiescence::Running;
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                // Another user's process: its descriptors are not ours to see.
                Err(_) if owner.is_some_and(|uid| uid != me) => {}
                Err(_) => unknown = true,
            }
        }
        if unknown {
            EmulatorQuiescence::Unknown
        } else {
            EmulatorQuiescence::Closed
        }
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

/// Atomically swap two paths (`renameat2(RENAME_EXCHANGE)`).
fn rename_exchange(a: &Path, b: &Path) -> std::io::Result<()> {
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
    if Some(sha.as_str()) == journal.staged_sha256.as_deref() || sha == journal.original_sha256 {
        let _ = fs::remove_file(staged);
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
    /// Pinned read-only descriptor on the card inode that was reviewed.
    pinned: fs::File,
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
    let binding = ps2_card_binding(&plan.source_card_path, &plan.save_display_name);
    check_quiescence(quiescence.observe(&binding))?;
    let psu = fs::read(&plan.source_psu_path).map_err(restore_error)?;
    if sha256_hex(&psu) != plan.source_psu_sha256 {
        return Err(Ps2PsuRestoreError::SourceChanged);
    }
    let mut pinned = fs::File::open(&plan.source_card_path).map_err(restore_error)?;
    let pinned_identity = identity_of(&pinned.metadata().map_err(restore_error)?);
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
        pinned,
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

    // Backup: create_new, fsync, re-read, verify. Never removed afterwards.
    let mut backup = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
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
        let _ = fs::remove_file(&plan.backup_path);
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
        let _ = fs::remove_file(&plan.backup_path);
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
    run.journal.staged_path = Some(staged_path.clone());
    run.journal.staged_sha256 = Some(staged_sha256.clone());
    run.journal.post_sha256 = Some(staged_sha256.clone());
    {
        let permissions = fs::metadata(&plan.source_card_path)
            .map_err(restore_error)?
            .permissions();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_path)
            .map_err(restore_error)?;
        file.write_all(&staged_bytes).map_err(restore_error)?;
        fs::set_permissions(&staged_path, permissions).map_err(restore_error)?;
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
    run.set_phase(Ps2RestorePhase::Staged)?;
    step(hook, Ps2RestoreStep::AfterStaged)?;

    // Publication gate: journal the intent to publish, then recheck everything
    // one more time, immediately before the rename.
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

    run.renamed = true;
    let exchanged = match rename_exchange(&staged_path, &plan.source_card_path) {
        Ok(()) => true,
        Err(error) if exchange_unsupported(&error) => {
            // Filesystem without RENAME_EXCHANGE: plain atomic rename, with the
            // weaker pinned-inode detection below.
            fs::rename(&staged_path, &plan.source_card_path).map_err(|error| {
                run.renamed = false;
                restore_error(error)
            })?;
            false
        }
        Err(error) => {
            run.renamed = false;
            return Err(restore_error(error).into());
        }
    };
    fs::File::open(card_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(restore_error)?;
    step(hook, Ps2RestoreStep::AfterRename)?;

    if exchanged {
        // The staged path now holds whatever the card path held at the instant
        // of the exchange. It must be exactly the reviewed inode with the
        // reviewed bytes; otherwise swap it back untouched and refuse.
        let displaced = fs::read(&staged_path).map_err(restore_error)?;
        let displaced_identity = path_identity(&staged_path)?;
        let same_inode = displaced_identity.device == run.journal.original_identity.device
            && displaced_identity.inode == run.journal.original_identity.inode;
        if !same_inode || sha256_hex(&displaced) != run.journal.original_sha256 {
            rename_exchange(&staged_path, &plan.source_card_path).map_err(|error| {
                // Could not swap back: unwind will write the newest content.
                run.original = displaced.clone();
                restore_error(error)
            })?;
            run.renamed = false;
            return Err(Ps2PsuRestoreError::CardChanged.into());
        }
        fs::remove_file(&staged_path).map_err(restore_error)?;
    } else {
        // The inode we replaced must still hold exactly the reviewed bytes. If
        // an external writer got in after the last recheck, its content is
        // restored by `unwind` (the newest pre-publication state).
        let mut old = Vec::new();
        run.pinned.seek(SeekFrom::Start(0)).map_err(restore_error)?;
        (&mut run.pinned)
            .take(PS2_MAX_CARD_BYTES as u64 + 1)
            .read_to_end(&mut old)
            .map_err(restore_error)?;
        if sha256_hex(&old) != run.journal.original_sha256 {
            run.original = old;
            return Err(Ps2PsuRestoreError::CardChanged.into());
        }
    }

    let live = read_source_card(&plan.source_card_path)
        .map_err(|error| Ps2PsuRestoreError::Io(error.to_string()))?;
    let after = inspect_memory_card(&plan.source_card_path).map_err(Ps2PsuRestoreError::Io)?;
    if sha256_hex(&live) != staged_sha256
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
    remove_owned_temp(&run.journal);
    let note = |run: &mut Run<'_>, phase, text: String| {
        run.journal.detail = Some(text);
        run.set_phase(phase)
    };
    if !run.renamed {
        // Never published: the card was not written.
        let _ = note(run, Ps2RestorePhase::Abandoned, cause.to_string());
        return cause;
    }
    // Published but not accepted: put the pre-restore content back.
    let target = run.original.clone();
    let target_sha = sha256_hex(&target);
    let rollback = (|| -> Result<(), String> {
        hook(Ps2RestoreStep::BeforeRollbackWrite).map_err(|e| e.to_string())?;
        restore_write_card_atomically(&plan.source_card_path, &target)
            .map_err(|e| e.to_string())?;
        let live = read_source_card(&plan.source_card_path).map_err(|e| e.to_string())?;
        if sha256_hex(&live) == target_sha {
            Ok(())
        } else {
            Err("card did not read back as the pre-restore content".into())
        }
    })();
    match rollback {
        Ok(()) => {
            let _ = note(
                run,
                Ps2RestorePhase::RolledBack,
                format!("{cause}; the card was returned to its pre-restore content"),
            );
            cause
        }
        Err(detail) => {
            let _ = note(
                run,
                Ps2RestorePhase::RollbackFailed,
                format!("{cause}; rollback failed: {detail}"),
            );
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
    let identity = identity_of(&pinned.metadata().map_err(restore_error)?);
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
    set(&mut journal, Ps2RestorePhase::UndoIntent, None)?;
    let card_dir = journal
        .card_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Ps2PsuRestoreError::InvalidPlan("card has no parent".into()))?;
    let undo_temp = undo_temp_path(&card_dir, &journal.operation_id);
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
        let permissions = fs::metadata(&journal.card_path)
            .map_err(restore_error)?
            .permissions();
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&undo_temp)
                .map_err(restore_error)?;
            file.write_all(&backup).map_err(restore_error)?;
            fs::set_permissions(&undo_temp, permissions).map_err(restore_error)?;
            file.sync_all().map_err(restore_error)?;
        }
        step(hook, Ps2RestoreStep::BeforeUndoRename)?;
        match rename_exchange(&undo_temp, &journal.card_path) {
            Ok(()) => {
                fs::File::open(&card_dir)
                    .and_then(|directory| directory.sync_all())
                    .map_err(restore_error)?;
                step(hook, Ps2RestoreStep::AfterUndoRename)?;
                // The displaced file must be the applied card, byte for byte;
                // anything else (replaced or edited at the last instant) is
                // swapped straight back and the undo is refused.
                let displaced = fs::read(&undo_temp).map_err(restore_error)?;
                let displaced_identity = path_identity(&undo_temp)?;
                if displaced_identity.device != post_identity.device
                    || displaced_identity.inode != post_identity.inode
                    || sha256_hex(&displaced) != post_sha
                {
                    rename_exchange(&undo_temp, &journal.card_path).map_err(restore_error)?;
                    let _ = fs::remove_file(&undo_temp);
                    return Err(Ps2PsuRestoreError::StaleUndo.into());
                }
                fs::remove_file(&undo_temp).map_err(restore_error)?;
            }
            Err(error) if exchange_unsupported(&error) => {
                let _ = fs::remove_file(&undo_temp);
                restore_write_card_atomically(&journal.card_path, &backup)?;
                step(hook, Ps2RestoreStep::AfterUndoRename)?;
                // Weaker path: detect an in-place write through the pinned inode.
                let mut old = Vec::new();
                pinned.seek(SeekFrom::Start(0)).map_err(restore_error)?;
                (&mut pinned)
                    .take(PS2_MAX_CARD_BYTES as u64 + 1)
                    .read_to_end(&mut old)
                    .map_err(restore_error)?;
                if sha256_hex(&old) != post_sha {
                    restore_write_card_atomically(&journal.card_path, &old)?;
                    return Err(Ps2PsuRestoreError::StaleUndo.into());
                }
            }
            Err(error) => {
                let _ = fs::remove_file(&undo_temp);
                return Err(restore_error(error).into());
            }
        }
        let restored = read_source_card(&journal.card_path)
            .map_err(|e| Ps2PsuRestoreError::Io(e.to_string()))?;
        if sha256_hex(&restored) != journal.original_sha256 {
            return Err(Ps2PsuRestoreError::VerificationFailed(
                "undo did not restore the original card bytes".into(),
            )
            .into());
        }
        Ok(())
    })();
    match result {
        Ok(()) => set(&mut journal, Ps2RestorePhase::Undone, None),
        #[cfg(test)]
        Err(Fail::Crash) => Err(Ps2PsuRestoreError::Io(SIMULATED_CRASH.into())),
        Err(Fail::Error(error)) => {
            // Our own undo temp (original image or displaced applied card) goes.
            if let Ok(bytes) = fs::read(&undo_temp) {
                let sha = sha256_hex(&bytes);
                if sha == journal.original_sha256 || sha == post_sha {
                    let _ = fs::remove_file(&undo_temp);
                }
            }
            // A stale-undo refusal means the card was left exactly as found (a late
            // foreign file, if any, was swapped back untouched): not a failure.
            if error == Ps2PsuRestoreError::StaleUndo {
                let _ = set(
                    &mut journal,
                    Ps2RestorePhase::Published,
                    Some(format!("undo refused: {error}")),
                );
                return Err(error);
            }
            // Is the card still the applied restore (nothing changed), or did
            // something go wrong after we wrote?
            let live = read_source_card(&journal.card_path)
                .ok()
                .map(|b| sha256_hex(&b));
            if live.as_deref() == Some(post_sha.as_str()) {
                let _ = set(
                    &mut journal,
                    Ps2RestorePhase::Published,
                    Some(format!("undo refused: {error}")),
                );
                Err(error)
            } else {
                let detail = format!("{error}; the card may not match either state");
                let _ = set(
                    &mut journal,
                    Ps2RestorePhase::UndoFailed,
                    Some(detail.clone()),
                );
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
    let live_sha = read_source_card(&journal.card_path)
        .ok()
        .map(|bytes| sha256_hex(&bytes));
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
            if live == original {
                remove_staged(&journal);
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
            if live == original {
                remove_staged(&journal);
                finish(
                    &mut journal,
                    Ps2RestorePhase::Abandoned,
                    "interrupted before the publishing rename; the card is still the original",
                )?;
                Ok(Ps2RecoveryOutcome::NotPublished)
            } else if live.is_some() && live == post {
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
            if let Some(parent) = journal.card_path.parent() {
                let temp = undo_temp_path(parent, &journal.operation_id);
                if let Ok(bytes) = fs::read(&temp) {
                    let sha = sha256_hex(&bytes);
                    if sha == journal.original_sha256 || Some(sha.as_str()) == post {
                        let _ = fs::remove_file(&temp);
                    }
                }
            }
            if live.is_some() && live == post {
                finish(
                    &mut journal,
                    Ps2RestorePhase::Published,
                    "interrupted undo had not changed the card; undo is available again",
                )?;
                Ok(Ps2RecoveryOutcome::UndoNotApplied)
            } else if live == original {
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

/// The per-user journal directory (created if missing).
pub fn default_ps2_restore_journal_dir() -> Result<PathBuf, Ps2PsuRestoreError> {
    let dir = crate::app_dirs::data_path("ps2-restore-journals")
        .map_err(|error| Ps2PsuRestoreError::Io(error.to_string()))?;
    fs::create_dir_all(&dir).map_err(restore_error)?;
    Ok(dir)
}

/// Guard options for "now" with the default journal directory.
pub fn default_ps2_restore_guard_options() -> Result<Ps2RestoreGuardOptions, Ps2PsuRestoreError> {
    Ok(Ps2RestoreGuardOptions {
        journal_dir: default_ps2_restore_journal_dir()?,
        unix_seconds: unix_now(),
    })
}

#[must_use]
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Judge every interrupted operation in `journal_dir` (see
/// [`recover_ps2_psu_restore`]); never writes a card.
pub fn recover_all_interrupted_ps2_restores(
    journal_dir: &Path,
    unix_seconds: u64,
) -> Vec<(PathBuf, Result<Ps2RecoveryOutcome, Ps2PsuRestoreError>)> {
    discover_ps2_restore_journals(journal_dir)
        .into_iter()
        .filter(|summary| summary.needs_recovery && summary.error.is_none())
        .map(|summary| {
            let outcome = recover_ps2_psu_restore(&summary.path, unix_seconds);
            (summary.path, outcome)
        })
        .collect()
}

#[cfg(test)]
mod tests;
