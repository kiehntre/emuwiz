//! Bounded shared apply, journal, history, and rollback pipeline.
//!
//! Writes are available only for an explicitly confirmed, exact plan produced
//! from the shared preview. PCSX2, Dolphin, and Xenia use their verified
//! transaction paths
//! until they expose an independent, adapter-approved materialized source.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::destination_safety::{
    DestinationRootState, DestinationState, assess_destination, validate_destination_root,
    validate_single_component,
};
use super::shared_preview::{
    PreviewAdapter, PreviewDestinationState, PreviewEligibility, PreviewProposedAction,
    SharedPreviewReport,
};
use crate::default_database_path;
use crate::identity_source::hackhash::HackHashPatchProvenance;

pub const SHARED_APPLY_SCHEMA_VERSION: u32 = 1;
/// Version for the durable intent/checkpoint envelope.  It is deliberately
/// separate from `SharedApplyJournal`: old final-only receipts remain
/// readable as legacy completed receipts.
pub const SHARED_DURABLE_JOURNAL_SCHEMA_VERSION: u32 = 1;
pub const SHARED_MAX_ENTRIES: usize = 128;
pub const SHARED_MAX_SOURCE_BYTES: u64 = 1024 * 1024;
/// ROM-sized generated outputs use a separate bounded in-memory lane. The
/// ordinary shared source lane remains deliberately small for config files.
pub const SHARED_MAX_MATERIALIZED_BYTES: u64 = 512 * 1024 * 1024;
pub const SHARED_MAX_TOTAL_WRITTEN_BYTES: u64 = 32 * 1024 * 1024;
pub const SHARED_MAX_BACKUP_BYTES: u64 = 32 * 1024 * 1024;
pub const SHARED_MAX_JOURNAL_BYTES: u64 = 2 * 1024 * 1024;
pub const SHARED_MAX_HISTORY_JOURNALS: usize = 512;
pub const SHARED_MAX_ROLLBACK_ENTRIES: usize = 128;
pub const SHARED_MAX_WARNINGS: usize = 64;
pub const SHARED_MAX_FAILURES: usize = 128;
pub const SHARED_MAX_CREATED_DIRECTORIES: usize = 32;
pub const SHARED_MAX_TEMP_FILES: usize = 128;
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_RETRY: Duration = Duration::from_millis(25);

pub fn default_shared_history_root() -> Result<PathBuf, SharedApplyFailure> {
    default_managed_root("shared-cheat-history")
}

pub fn default_shared_backup_root() -> Result<PathBuf, SharedApplyFailure> {
    default_managed_root("shared-cheat-backups")
}

fn default_managed_root(name: &str) -> Result<PathBuf, SharedApplyFailure> {
    let database = default_database_path().map_err(|error_value| {
        failure(
            SharedApplyFailureKind::ManagedRootUnsafe,
            None,
            &error_value.to_string(),
        )
    })?;
    Ok(database
        .parent()
        .expect("default database path always has a parent")
        .join(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FaultPoint {
    BackupWrite,
    TemporaryWrite,
    Flush,
    Rename,
    Verification,
    JournalWrite,
    ParentCreationRace,
    SourceMutation,
    DestinationMutation,
    RollbackRemovalVerification,
    RollbackRestore,
    ApplyCheckpoint,
    RollbackCheckpoint,
    RollbackAfterMutation,
    TerminalJournal,
    DurableJournalWrite,
    Permissions,
    ExchangeUnsupported,
    ReverseExchange,
    /// The durable checkpoint right after the destination was replaced.
    ReplacedCheckpoint,
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static INJECTED_FAULT: std::cell::Cell<Option<FaultPoint>> = const { std::cell::Cell::new(None) };
    static INJECTED_AFTER: std::cell::Cell<Option<(FaultPoint, usize)>> = const { std::cell::Cell::new(None) };
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn inject_fault(point: Option<FaultPoint>) {
    INJECTED_FAULT.with(|fault| fault.set(point));
    INJECTED_AFTER.with(|fault| fault.set(None));
}

#[cfg(test)]
pub(super) fn inject_fault_after(point: FaultPoint, occurrences: usize) {
    INJECTED_FAULT.with(|fault| fault.set(None));
    INJECTED_AFTER.with(|fault| fault.set(Some((point, occurrences))));
}

#[cfg(any(test, feature = "test-support"))]
fn should_inject(point: FaultPoint) -> bool {
    INJECTED_FAULT.with(|fault| fault.get() == Some(point))
}

#[cfg(any(test, feature = "test-support"))]
fn should_inject_after(point: FaultPoint) -> bool {
    INJECTED_AFTER.with(|fault| {
        let Some((configured, remaining)) = fault.get() else {
            return false;
        };
        if configured != point {
            return false;
        }
        if remaining == 0 {
            true
        } else {
            fault.set(Some((configured, remaining - 1)));
            false
        }
    })
}

#[cfg(not(any(test, feature = "test-support")))]
fn should_inject(_point: FaultPoint) -> bool {
    false
}

/// Synthetic GUI tests make every rollback restore on this thread fail, the
/// way `FaultPoint::RollbackRestore` does in this module's own tests, until
/// called again with `false`. Unavailable in ordinary application builds.
#[cfg(feature = "test-support")]
pub fn set_test_rollback_restore_fault(enabled: bool) {
    inject_fault(enabled.then_some(FaultPoint::RollbackRestore));
}

/// Synthetic GUI tests make the durable checkpoint taken right after a
/// destination is replaced fail on this thread, until called with `false`.
/// Unavailable in ordinary application builds.
#[cfg(feature = "test-support")]
pub fn set_test_replaced_checkpoint_fault(enabled: bool) {
    inject_fault(enabled.then_some(FaultPoint::ReplacedCheckpoint));
}

/// Synthetic GUI tests make an apply on this thread stop right before its
/// terminal journal is written (every file is already published and
/// checkpointed), which is what a crash at that moment leaves behind.
/// Unavailable in ordinary application builds.
#[cfg(feature = "test-support")]
pub fn set_test_terminal_journal_fault(enabled: bool) {
    inject_fault(enabled.then_some(FaultPoint::TerminalJournal));
}

#[cfg(not(any(test, feature = "test-support")))]
fn should_inject_after(_point: FaultPoint) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedAdapterWriteSupport {
    ApplyAndRollback,
    PreviewOnlySourceNotMaterialized,
}

pub fn adapter_write_support(adapter: PreviewAdapter) -> SharedAdapterWriteSupport {
    match adapter {
        PreviewAdapter::RetroArch
        | PreviewAdapter::Pcsx2
        | PreviewAdapter::DuckStation
        | PreviewAdapter::MelonDs
        | PreviewAdapter::Dolphin
        | PreviewAdapter::Ppsspp
        | PreviewAdapter::Xenia
        | PreviewAdapter::LocalModPackage
        | PreviewAdapter::CemuGraphicPack
        | PreviewAdapter::Rpcs3OrdinaryMod
        | PreviewAdapter::Mame
        | PreviewAdapter::Flycast
        | PreviewAdapter::Mgba
        | PreviewAdapter::Rpcs3Patch
        | PreviewAdapter::Mednafen
        | PreviewAdapter::Fbneo
        | PreviewAdapter::AmigaWhdloadTrainer
        | PreviewAdapter::ScummVmTrainer => SharedAdapterWriteSupport::ApplyAndRollback,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedTransactionPath {
    pub display: String,
    pub unix_bytes_hex: Option<String>,
}

impl SharedTransactionPath {
    pub fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bytes = path.as_os_str().as_bytes();
            Self {
                display: path.to_string_lossy().into_owned(),
                unix_bytes_hex: Some(hex_bytes(bytes)),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                display: path.to_string_lossy().into_owned(),
                unix_bytes_hex: None,
            }
        }
    }

    pub fn to_path_buf(&self) -> Result<PathBuf, SharedApplyFailureKind> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let encoded = self
                .unix_bytes_hex
                .as_deref()
                .ok_or(SharedApplyFailureKind::InvalidJournal)?;
            let bytes = decode_hex(encoded).ok_or(SharedApplyFailureKind::InvalidJournal)?;
            Ok(PathBuf::from(OsString::from_vec(bytes)))
        }
        #[cfg(not(unix))]
        {
            if self.unix_bytes_hex.is_some() {
                return Err(SharedApplyFailureKind::UnsupportedJournal);
            }
            Ok(PathBuf::from(&self.display))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedTransactionStage {
    DryRun,
    InstallNew,
    ReplaceExisting,
    AlreadyInstalled,
    SkippedNotEligible,
    SkippedConflict,
    SkippedReplacementNotApproved,
    SourceChanged,
    DestinationChanged,
    BackupCreated,
    BackupFailed,
    WriteFailed,
    VerificationFailed,
    JournalWritten,
    JournalFailedAfterSuccessfulWrite,
    Success,
    PartialFailure,
    Failed,
    RollbackAvailable,
    RollbackUnavailable,
    RollbackBlocked,
    RollbackSucceeded,
    RollbackFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedApplyFailureKind {
    ConfirmationRequired,
    ConfirmationPlanMismatch,
    ReplacementNotApproved,
    UnsupportedAdapter,
    InvalidPlan,
    DuplicateOperationId,
    DuplicateDestination,
    ResourceLimitReached,
    SourceOutsideApprovedScope,
    SourceMissing,
    SourceSymlink,
    SourceSpecialFile,
    SourceChanged,
    DestinationUnsafe,
    DestinationChanged,
    RootChanged,
    LockUnsupported,
    LockTimeout,
    ManagedRootUnsafe,
    ParentCreationFailed,
    BackupFailed,
    WriteFailed,
    VerificationFailed,
    JournalFailed,
    InvalidJournal,
    UnsupportedJournal,
    BackupMissing,
    BackupChanged,
    AlreadyRolledBack,
    RollbackBlocked,
    /// The folder's filesystem cannot exchange two names atomically, so a
    /// file there cannot be replaced or restored safely; nothing changed.
    AtomicReplaceUnsupported,
    /// The file being displaced had changed and could not be put back: ours
    /// is published and the displaced file is kept under the temporary name.
    DisplacedFileKept,
    /// EmuWiz's file was published, but the destination no longer matched
    /// it when checked straight afterwards: something changed it.
    PublishedUnverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedApplyFailure {
    pub kind: SharedApplyFailureKind,
    pub path: Option<SharedTransactionPath>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedPlanEntry {
    pub adapter: PreviewAdapter,
    pub selected_archive: SharedTransactionPath,
    pub verified_game_identity: String,
    pub source_path: SharedTransactionPath,
    pub source_digest: String,
    pub destination_root: SharedTransactionPath,
    pub destination_relative_path: SharedTransactionPath,
    pub destination_pre_state: PreviewDestinationState,
    pub destination_pre_digest: Option<String>,
    pub proposed_action: PreviewProposedAction,
    pub backup_required: bool,
    pub parent_creation_approved: bool,
    #[serde(default)]
    pub content_verification: Option<SharedContentVerification>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedContentVerification {
    /// Marks a plan produced by the local generic-mod adapter.  Unlike the
    /// emulator-specific adapters, its destination may contain any number of
    /// safe normal path components beneath the approved game root.
    LocalModPackage,
    CemuGraphicPack,
    Rpcs3OrdinaryMod,
    RetroArchBezel,
    HackHashPatch {
        base_hash: String,
        patch_hash: String,
        expected_output_sha1: String,
        expected_output_md5: String,
        expected_output_crc32: String,
        expected_output_hash: String,
        provider_snapshot_hash: String,
        tool: String,
        #[serde(default)]
        provenance: Option<HackHashPatchProvenance>,
    },
    DolphinManagedGameHacking {
        expected_managed_names: Vec<String>,
        require_managed_section: bool,
        require_code_section: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedApplyContext {
    pub adapter: PreviewAdapter,
    pub selected_archive: SharedTransactionPath,
    pub verified_game_identity: String,
    pub profile_id: String,
    pub source_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedTransactionPlan {
    pub schema_version: u32,
    pub plan_id: String,
    pub context: SharedApplyContext,
    pub approved_source_root: SharedTransactionPath,
    pub destination_root: SharedTransactionPath,
    pub entries: Vec<SharedPlanEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedApplyConfirmation {
    pub plan_id: String,
    pub general_approved: bool,
    pub replacement_approved: bool,
}

#[derive(Debug, Clone)]
pub struct SharedApplyOptions {
    pub dry_run: bool,
    pub confirmation: Option<SharedApplyConfirmation>,
    pub operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub current_context: SharedApplyContext,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

/// Generated output handed directly to the shared atomic writer. No staging
/// file is created outside the shared journal.
#[derive(Debug, Clone)]
pub struct SharedMaterializedOutput {
    pub source_path: PathBuf,
    pub source_digest: String,
    pub output_digest: String,
    pub bytes: Vec<u8>,
    pub guard_paths: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedApplyOutcome {
    DryRun,
    InstalledNew,
    ReplacedExisting,
    AlreadyInstalled,
    SkippedNotEligible,
    SkippedConflict,
    SkippedReplacementNotApproved,
    SourceChanged,
    DestinationChanged,
    BackupFailed,
    WriteFailed,
    VerificationFailed,
}

/// Whether this entry's output reached the live destination. Recorded by the
/// engine at the point publication happens (or is ruled out), so callers never
/// infer it from outcomes or failure wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedPublicationState {
    /// A journal written before this was recorded. Readers must treat a
    /// failed entry with this state as uncertain.
    #[default]
    NotRecorded,
    /// Definitely nothing was left published: the entry stopped before the
    /// destination changed, or a refused exchange was put back at once (its
    /// bytes may have been briefly visible to a concurrent reader).
    NotPublished,
    /// Published and verified in place.
    Published,
    /// Published, then a later step (verification, durable checkpoint)
    /// failed. The destination may hold this entry's output.
    PublishedThenFailed,
    /// Published, failed verification, and the previous live state was
    /// restored and checked. The destination no longer holds this output.
    PublishedThenRestored,
    /// The engine cannot tell whether the destination holds this output
    /// (for example a displaced file was kept under the temporary name).
    Uncertain,
}

/// What an Apply (and any rollback of it) left at the destination, for
/// callers that must say "Installed", "nothing was written" or "Recovery
/// required" truthfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedDestinationOutcome {
    /// Status Success and its journal was saved durably.
    DurableSuccess,
    /// Status Success, but the journal could not be saved: the files are in
    /// place and verified, and there is no record to undo from.
    SuccessWithoutDurableRecord,
    /// No entry published anything that is still in place, proven per entry.
    NothingPublished,
    /// Something was published, and every such entry was reversed: by a
    /// rollback, or by the engine restoring the previous file after a failed
    /// check. Never reported as "nothing was written".
    Reversed,
    /// At least one entry's output definitely reached the destination and
    /// was not reversed.
    PublishedNeedsRecovery,
    /// No definite unreversed publication, but at least one entry's
    /// publication state is uncertain.
    Uncertain,
}

/// Per-entry view of [`SharedDestinationOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedEntryDestination {
    NothingPublished,
    Published,
    /// Reversed by a rollback.
    Reversed,
    /// Published, failed its check, and the engine restored the previous
    /// live file (which may now be a new file with owner-only permissions).
    RestoredAfterFailedCheck,
    NeedsRecovery,
    Uncertain,
}

/// Whether a rollback of this journal can reverse the entry. Rollback only
/// acts on completed InstalledNew/ReplacedExisting entries; every other
/// entry previews as NoChangeRequired or DestinationChanged.
pub fn shared_rollback_can_reverse(entry: &SharedApplyEntry) -> bool {
    matches!(
        entry.outcome,
        SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
    )
}

/// Classify one entry. `rollback_entry` is the same-index entry of a
/// rollback of this journal, if one ran.
pub fn shared_entry_destination(
    entry: &SharedApplyEntry,
    rollback_entry: Option<&SharedRollbackEntry>,
) -> SharedEntryDestination {
    let reversed = rollback_entry.is_some_and(|rollback| {
        rollback.failure.is_none()
            && matches!(
                rollback.outcome,
                SharedRollbackOutcome::RestoredBackup | SharedRollbackOutcome::RemovedInstalledFile
            )
    });
    let completed = matches!(
        entry.outcome,
        SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
    );
    let state = match entry.publication {
        SharedPublicationState::NotRecorded => match entry.outcome {
            _ if completed => SharedPublicationState::Published,
            SharedApplyOutcome::WriteFailed
            | SharedApplyOutcome::VerificationFailed
            | SharedApplyOutcome::DestinationChanged => SharedPublicationState::Uncertain,
            _ => SharedPublicationState::NotPublished,
        },
        state => state,
    };
    match state {
        SharedPublicationState::NotPublished => SharedEntryDestination::NothingPublished,
        SharedPublicationState::PublishedThenRestored => {
            SharedEntryDestination::RestoredAfterFailedCheck
        }
        _ if reversed => SharedEntryDestination::Reversed,
        SharedPublicationState::Published if completed => SharedEntryDestination::Published,
        SharedPublicationState::Published | SharedPublicationState::PublishedThenFailed => {
            SharedEntryDestination::NeedsRecovery
        }
        SharedPublicationState::Uncertain | SharedPublicationState::NotRecorded => {
            SharedEntryDestination::Uncertain
        }
    }
}

/// Classify an Apply result and, if one ran, its rollback (whose entries are
/// index-aligned with the journal's).
pub fn shared_destination_outcome(
    result: &SharedApplyResult,
    rollback: Option<&SharedRollbackResult>,
) -> SharedDestinationOutcome {
    if result.journal.status == SharedApplyStatus::Success && rollback.is_none() {
        return if result.journal_path.is_some() && result.journal_failure.is_none() {
            SharedDestinationOutcome::DurableSuccess
        } else {
            SharedDestinationOutcome::SuccessWithoutDurableRecord
        };
    }
    let mut any_reversed = false;
    let mut any_uncertain = false;
    for (index, entry) in result.journal.entries.iter().enumerate() {
        let rollback_entry = rollback.and_then(|value| value.preview.entries.get(index));
        match shared_entry_destination(entry, rollback_entry) {
            SharedEntryDestination::NeedsRecovery | SharedEntryDestination::Published => {
                return SharedDestinationOutcome::PublishedNeedsRecovery;
            }
            SharedEntryDestination::Uncertain => any_uncertain = true,
            SharedEntryDestination::Reversed | SharedEntryDestination::RestoredAfterFailedCheck => {
                any_reversed = true
            }
            SharedEntryDestination::NothingPublished => {}
        }
    }
    if any_uncertain {
        SharedDestinationOutcome::Uncertain
    } else if any_reversed {
        SharedDestinationOutcome::Reversed
    } else {
        SharedDestinationOutcome::NothingPublished
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedApplyEntry {
    pub plan_entry: SharedPlanEntry,
    /// Explicit filesystem facts observed under the destination lock before
    /// apply. These are not inferred from byte length: an existing empty file
    /// is still an existing file and must be restored, not removed.
    #[serde(default)]
    pub destination_existed_before_apply: Option<bool>,
    #[serde(default)]
    pub destination_parent_existed_before_apply: Option<bool>,
    pub observed_source_digest: Option<String>,
    pub observed_destination_digest: Option<String>,
    pub backup_path: Option<SharedTransactionPath>,
    pub backup_digest: Option<String>,
    /// Permission bits (`st_mode & 0o7777`) of the replaced file, read from
    /// the same descriptor whose bytes became `backup_digest`. Absent in
    /// journals written before it was recorded: Undo then never guesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_pre_mode: Option<u32>,
    pub temporary_path: Option<SharedTransactionPath>,
    pub final_destination_digest: Option<String>,
    pub created_directories: Vec<SharedTransactionPath>,
    pub replacement_approved: bool,
    pub verification_succeeded: bool,
    pub outcome: SharedApplyOutcome,
    /// Whether this entry's output reached the destination; see
    /// [`SharedPublicationState`]. Absent in older journals.
    #[serde(default)]
    pub publication: SharedPublicationState,
    pub stages: Vec<SharedTransactionStage>,
    pub warnings: Vec<String>,
    pub failures: Vec<SharedApplyFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedApplyStatus {
    DryRun,
    Success,
    PartialFailure,
    Failed,
}

/// Durable state of the shared transaction envelope.  These states are
/// descriptive only; discovery never resumes or mutates a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedRecoveryState {
    Planned,
    Applying,
    Applied,
    ApplyFailed,
    RollingBack,
    RolledBack,
    RollbackFailed,
    NeedsReview,
    UnsafeToResume,
    LegacyComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedEntryRecoveryState {
    Planned,
    Applying,
    BackupCreated,
    DestinationTempWritten,
    DestinationReplaced,
    DestinationVerified,
    Applied,
    RollingBack,
    RolledBack,
    NeedsReview,
    UnsafeToResume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedDurableEntry {
    pub plan_entry: SharedPlanEntry,
    pub state: SharedEntryRecoveryState,
    pub observed_destination_digest: Option<String>,
    pub backup_path: Option<SharedTransactionPath>,
    pub backup_digest: Option<String>,
    /// Copied from [`SharedApplyEntry::destination_pre_mode`] once the backup
    /// exists, so an interrupted transaction keeps the same evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_pre_mode: Option<u32>,
    pub resulting_destination_digest: Option<String>,
    pub temporary_path: Option<SharedTransactionPath>,
    pub destination_existed_before_apply: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedDurableJournal {
    pub schema_version: u32,
    pub transaction_state: SharedRecoveryState,
    pub operation_id: String,
    pub operation_type: String,
    pub plan_id: String,
    pub timestamp_unix_seconds: u64,
    pub context: SharedApplyContext,
    pub approved_source_root: SharedTransactionPath,
    pub destination_root: SharedTransactionPath,
    pub backup_root: SharedTransactionPath,
    pub entries: Vec<SharedDurableEntry>,
    #[serde(default)]
    pub created_root_directories: Vec<SharedCreatedRootDirectory>,
    pub rollback_operation_id: Option<String>,
    #[serde(default)]
    pub rollback_of_operation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedPendingOperation {
    pub journal_path: SharedTransactionPath,
    pub journal: SharedDurableJournal,
    pub reconciled_entries: Vec<SharedEntryRecoveryState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedPendingRecoveryPlan {
    pub operation_id: String,
    pub state: SharedRecoveryState,
    pub can_safe_rollback: bool,
    pub can_resume: bool,
    pub reason: String,
}

/// What [`recover_interrupted_shared_applies`] did with one pending apply
/// journal whose process never wrote the terminal journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedInterruptedApplyOutcome {
    /// A terminal journal now records exactly which files reached the
    /// destination, so History shows the operation and Undo can reverse it.
    SettledForUndo,
    /// The destination root is still locked by a running apply; nothing
    /// was touched. Not an error.
    StillRunning,
    /// The terminal journal already existed (the interruption came after it
    /// was written); only the stale pending record was retired.
    AlreadyRecorded,
    /// The pending record could not be settled safely (unsafe or missing
    /// root, unreadable record, journal write failure). It is kept as it is.
    NeedsReview,
}

/// One pending apply journal [`recover_interrupted_shared_applies`] looked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedInterruptedApplyRecovery {
    pub operation_id: String,
    pub pending_journal_path: SharedTransactionPath,
    pub outcome: SharedInterruptedApplyOutcome,
    /// Per entry, what the destination proves now (see
    /// [`SharedEntryRecoveryState`]); the order is the plan's.
    pub entry_states: Vec<SharedEntryRecoveryState>,
    /// The terminal journal written (or found) for the operation.
    pub journal_path: Option<SharedTransactionPath>,
    pub detail: String,
}

/// The stable filesystem identity of one directory, captured via `fstat` on
/// an already-open, no-follow-opened descriptor at the moment
/// [`bootstrap_missing_destination_root`] created it - never re-derived
/// from a pathname later. This is the proof rollback checks before it will
/// ever remove a bootstrap-created directory: a directory that now exists
/// at the same *path* but has a different `(device, inode)` pair is a
/// replacement, never the one this transaction made, and is never touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedDirectoryIdentity {
    pub device: u64,
    pub inode: u64,
}

/// One destination-root directory level [`bootstrap_missing_destination_root`]
/// created. `identity` is `Option` only so that a malformed/tampered
/// journal entry that omits it deserializes instead of poisoning the whole
/// journal - [`validate_created_root_chain`] treats a missing identity as
/// unprovable ownership and refuses to act on it, and no code path this
/// module controls ever writes an entry without one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedCreatedRootDirectory {
    pub path: SharedTransactionPath,
    #[serde(default)]
    pub identity: Option<SharedDirectoryIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedApplyJournal {
    pub schema_version: u32,
    pub operation_id: String,
    pub plan_id: String,
    pub timestamp_unix_seconds: u64,
    pub context: SharedApplyContext,
    pub approved_source_root: SharedTransactionPath,
    pub destination_root: SharedTransactionPath,
    /// Destination-root directory levels this transaction itself created
    /// because `destination_root` (or one of its ancestors up to the
    /// nearest already-existing directory) did not exist yet - see
    /// `bootstrap_missing_destination_root`. Ordered outermost-created
    /// first, so `destination_root` itself (when created) is always last.
    ///
    /// Deliberately separate from each [`SharedApplyEntry::created_directories`]
    /// (ordinary child directories, e.g. a platform folder, created *below*
    /// an already-existing or already-bootstrapped root): this field alone
    /// is how rollback tells "the root existed before EmuWiz" from "this
    /// transaction created the root," which an ordinary child-directory
    /// entry can never express.
    ///
    /// Untrusted, persisted input by the time rollback reads it back -
    /// never acted on directly. See [`validate_created_root_chain`] for the
    /// full set of structural checks a claimed chain must pass before
    /// anything here is even opened, let alone removed.
    ///
    /// `#[serde(default)]` so a journal written before this field existed
    /// deserializes with an empty list - meaning nothing here for it, since
    /// no journal written by older code could ever have bootstrapped a
    /// root. Its rollback behaviour is therefore unchanged for every old
    /// journal: an empty list carries no root-cleanup authority at all, and
    /// there is no way to reinterpret an absent field into one - see
    /// [`cleanup_transaction_created_root_directories`].
    #[serde(default)]
    pub created_root_directories: Vec<SharedCreatedRootDirectory>,
    pub dry_run: bool,
    pub entries: Vec<SharedApplyEntry>,
    pub status: SharedApplyStatus,
    pub rollback_operation_id: Option<String>,
}

#[derive(Debug)]
pub struct SharedApplyResult {
    pub journal: SharedApplyJournal,
    pub journal_path: Option<PathBuf>,
    pub journal_failure: Option<SharedApplyFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedJournalWarning {
    pub path: SharedTransactionPath,
    pub failure: SharedApplyFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedHistoryReport {
    pub journals: Vec<(SharedTransactionPath, SharedApplyJournal)>,
    pub warnings: Vec<SharedJournalWarning>,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedRollbackOutcome {
    Available,
    NoChangeRequired,
    DestinationChanged,
    DestinationMissing,
    DestinationUnsafe,
    BackupMissing,
    BackupChanged,
    JournalMalformed,
    JournalUnsupported,
    RootMismatch,
    AlreadyRolledBack,
    RemovedInstalledFile,
    RestoredBackup,
    Failed,
    /// Deliberately left as it is: an earlier file could not be restored,
    /// so Undo stopped before removing or changing anything else.
    Retained,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedRollbackEntry {
    pub destination: Option<SharedTransactionPath>,
    pub backup: Option<SharedTransactionPath>,
    pub expected_installed_digest: Option<String>,
    pub observed_destination_digest: Option<String>,
    pub observed_backup_digest: Option<String>,
    pub outcome: SharedRollbackOutcome,
    pub failure: Option<SharedApplyFailure>,
    /// What a restored backup's permission bits will be; only set for a
    /// replaced file. Shown in the preview, so it is part of what is confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<SharedRollbackPermissions>,
}

/// Permission bits for a restored backup. Only a mode the journal recorded
/// at apply time is restored, and only ordinary owner-readable bits; anything
/// else keeps EmuWiz's owner-only default and says so instead of guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedRollbackPermissions {
    Original { mode: u32 },
    NotRecorded,
    NotRestorable { mode: u32 },
}

impl SharedRollbackPermissions {
    pub fn for_recorded_mode(mode: Option<u32>) -> Self {
        match mode {
            None => Self::NotRecorded,
            Some(mode) if mode & !0o777 == 0 && mode & 0o400 != 0 => Self::Original { mode },
            Some(mode) => Self::NotRestorable { mode },
        }
    }

    /// The mode the restored file is written with.
    pub fn restored_mode(self) -> u32 {
        match self {
            Self::Original { mode } => mode,
            Self::NotRecorded | Self::NotRestorable { .. } => OWNER_ONLY_MODE,
        }
    }

    pub fn summary(self) -> String {
        match self {
            Self::Original { mode } => {
                format!("original {mode:04o}, as recorded before the change")
            }
            Self::NotRecorded => format!(
                "owner-only {OWNER_ONLY_MODE:04o}; this older change did not record the original permissions"
            ),
            Self::NotRestorable { mode } => format!(
                "owner-only {OWNER_ONLY_MODE:04o}; the original {mode:04o} is not restored automatically"
            ),
        }
    }
}

const OWNER_ONLY_MODE: u32 = 0o600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedRollbackPreview {
    pub schema_version: u32,
    pub preview_id: String,
    pub journal_path: SharedTransactionPath,
    pub original_operation_id: String,
    pub destination_root: SharedTransactionPath,
    pub entries: Vec<SharedRollbackEntry>,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedRollbackConfirmation {
    pub preview_id: String,
    pub approved: bool,
}

#[derive(Debug, Clone)]
pub struct SharedRollbackOptions {
    pub confirmation: SharedRollbackConfirmation,
    pub rollback_operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

#[derive(Debug)]
pub struct SharedRollbackResult {
    pub preview: SharedRollbackPreview,
    pub journal_path: Option<PathBuf>,
    pub status: SharedApplyStatus,
}

pub fn build_shared_transaction_plan(
    preview: &SharedPreviewReport,
    profile_id: &str,
    source_mode: &str,
    approved_source_root: &Path,
) -> Result<SharedTransactionPlan, SharedApplyFailure> {
    if adapter_write_support(preview.adapter) != SharedAdapterWriteSupport::ApplyAndRollback {
        return Err(failure(
            SharedApplyFailureKind::UnsupportedAdapter,
            None,
            "adapter has no independent approved materialized source",
        ));
    }
    if preview.entries.len() > SHARED_MAX_ENTRIES {
        return Err(failure(
            SharedApplyFailureKind::ResourceLimitReached,
            None,
            "apply entry limit reached",
        ));
    }
    strict_absolute_root(approved_source_root)?;
    let first = preview.entries.first().ok_or_else(|| {
        failure(
            SharedApplyFailureKind::InvalidPlan,
            None,
            "preview has no entries",
        )
    })?;
    let destination_root = first.destination_root.clone();
    strict_absolute_root(&destination_root)?;
    let identity = first.verified_identity.clone().ok_or_else(|| {
        failure(
            SharedApplyFailureKind::InvalidPlan,
            None,
            "verified identity is required",
        )
    })?;
    let mut destinations = BTreeSet::new();
    let mut entries = Vec::new();
    for entry in &preview.entries {
        if entry.adapter != preview.adapter
            || entry.selected_archive != preview.request_archive
            || entry.destination_root != destination_root
            || entry.verified_identity.as_deref() != Some(identity.as_str())
        {
            return Err(failure(
                SharedApplyFailureKind::InvalidPlan,
                entry.destination_path.as_deref(),
                "preview entry context is inconsistent",
            ));
        }
        if entry.eligibility != PreviewEligibility::Eligible
            || !matches!(
                entry.proposed_action,
                PreviewProposedAction::Install
                    | PreviewProposedAction::Replace
                    | PreviewProposedAction::Skip
            )
        {
            continue;
        }
        let source = entry.source_path.as_ref().ok_or_else(|| {
            failure(
                SharedApplyFailureKind::InvalidPlan,
                None,
                "eligible entry has no source",
            )
        })?;
        let source_digest = entry.source_digest.clone().ok_or_else(|| {
            failure(
                SharedApplyFailureKind::InvalidPlan,
                Some(source),
                "eligible entry has no source digest",
            )
        })?;
        let relative = entry.destination_relative_path.as_ref().ok_or_else(|| {
            failure(
                SharedApplyFailureKind::InvalidPlan,
                None,
                "eligible entry has no relative destination",
            )
        })?;
        let destination = entry.destination_path.as_ref().ok_or_else(|| {
            failure(
                SharedApplyFailureKind::InvalidPlan,
                None,
                "eligible entry has no final destination",
            )
        })?;
        if !destinations.insert(destination.clone()) {
            return Err(failure(
                SharedApplyFailureKind::DuplicateDestination,
                Some(destination),
                "duplicate destination in transaction",
            ));
        }
        entries.push(SharedPlanEntry {
            adapter: entry.adapter,
            selected_archive: SharedTransactionPath::from_path(&entry.selected_archive),
            verified_game_identity: identity.clone(),
            source_path: SharedTransactionPath::from_path(source),
            source_digest,
            destination_root: SharedTransactionPath::from_path(&destination_root),
            destination_relative_path: SharedTransactionPath::from_path(relative),
            destination_pre_state: entry.destination_state,
            destination_pre_digest: entry.existing_destination_digest.clone(),
            proposed_action: entry.proposed_action,
            backup_required: entry.backup_required,
            parent_creation_approved: entry.warnings.iter().any(|warning| {
                warning.kind == super::shared_preview::PreviewWarningKind::DestinationParentsMissing
            }),
            content_verification: None,
        });
    }
    if entries.is_empty() {
        return Err(failure(
            SharedApplyFailureKind::InvalidPlan,
            None,
            "preview has no eligible materialized entries",
        ));
    }
    entries.sort_by(|left, right| {
        left.destination_relative_path
            .unix_bytes_hex
            .cmp(&right.destination_relative_path.unix_bytes_hex)
    });
    let context = SharedApplyContext {
        adapter: preview.adapter,
        selected_archive: SharedTransactionPath::from_path(&preview.request_archive),
        verified_game_identity: identity,
        profile_id: profile_id.to_owned(),
        source_mode: source_mode.to_owned(),
    };
    let mut plan = SharedTransactionPlan {
        schema_version: SHARED_APPLY_SCHEMA_VERSION,
        plan_id: String::new(),
        context,
        approved_source_root: SharedTransactionPath::from_path(approved_source_root),
        destination_root: SharedTransactionPath::from_path(&destination_root),
        entries,
    };
    plan.plan_id = plan_digest(&plan)?;
    Ok(plan)
}

/// Adds GameCube GameHacking's semantic post-write contract to an already
/// built shared plan and re-seals its digest. The generated source remains
/// staging; verification always reads the exact live destination.
pub fn require_dolphin_managed_gamehacking_verification(
    plan: &mut SharedTransactionPlan,
    expected_managed_names: Vec<String>,
) -> Result<(), SharedApplyFailure> {
    if plan.context.adapter != PreviewAdapter::Dolphin || plan.entries.is_empty() {
        return Err(failure(
            SharedApplyFailureKind::InvalidPlan,
            None,
            "Dolphin managed-INI verification requires a Dolphin transaction",
        ));
    }
    let require_sections = !expected_managed_names.is_empty();
    for entry in &mut plan.entries {
        entry.content_verification = Some(SharedContentVerification::DolphinManagedGameHacking {
            expected_managed_names: expected_managed_names.clone(),
            require_managed_section: require_sections,
            require_code_section: require_sections,
        });
    }
    plan.plan_id.clear();
    plan.plan_id = plan_digest(plan)?;
    Ok(())
}

/// Marks a validated local generic-mod plan as using the generic safe-path
/// destination contract and reseals its plan digest.
pub fn require_local_mod_package_verification(
    plan: &mut SharedTransactionPlan,
) -> Result<(), SharedApplyFailure> {
    if plan.context.source_mode != "local_mod_package" || plan.entries.is_empty() {
        return Err(failure(
            SharedApplyFailureKind::InvalidPlan,
            None,
            "local-mod verification requires a local package transaction",
        ));
    }
    for entry in &mut plan.entries {
        entry.content_verification = Some(SharedContentVerification::LocalModPackage);
    }
    plan.plan_id.clear();
    plan.plan_id = plan_digest(plan)?;
    Ok(())
}

/// Marks a Cemu graphic-pack plan as using its reviewed nested-path contract
/// and reseals the plan before it can be applied.
pub fn require_cemu_graphic_pack_verification(
    plan: &mut SharedTransactionPlan,
) -> Result<(), SharedApplyFailure> {
    if plan.context.adapter != PreviewAdapter::CemuGraphicPack
        || plan.context.source_mode != "cemu_graphic_pack"
        || plan.entries.is_empty()
    {
        return Err(failure(
            SharedApplyFailureKind::InvalidPlan,
            None,
            "Cemu graphic-pack verification requires a Cemu graphic-pack transaction",
        ));
    }
    for entry in &mut plan.entries {
        entry.content_verification = Some(SharedContentVerification::CemuGraphicPack);
    }
    plan.plan_id.clear();
    plan.plan_id = plan_digest(plan)?;
    Ok(())
}

/// Marks a validated ordinary RPCS3 file-layer mod as using the reviewed
/// nested-path contract and reseals its plan before it can be applied.
pub fn require_rpcs3_ordinary_mod_verification(
    plan: &mut SharedTransactionPlan,
) -> Result<(), SharedApplyFailure> {
    if plan.context.adapter != PreviewAdapter::Rpcs3OrdinaryMod
        || plan.context.source_mode != "rpcs3_ordinary_mod"
        || plan.entries.is_empty()
    {
        return Err(failure(
            SharedApplyFailureKind::InvalidPlan,
            None,
            "RPCS3 ordinary-mod verification requires an RPCS3 ordinary-mod transaction",
        ));
    }
    for entry in &mut plan.entries {
        entry.content_verification = Some(SharedContentVerification::Rpcs3OrdinaryMod);
    }
    plan.plan_id.clear();
    plan.plan_id = plan_digest(plan)?;
    Ok(())
}

/// Marks a RetroArch bezel transaction as using the reviewed nested override
/// path contract and reseals its digest before apply.
pub fn require_retroarch_bezel_verification(
    plan: &mut SharedTransactionPlan,
) -> Result<(), SharedApplyFailure> {
    if plan.context.adapter != PreviewAdapter::RetroArch
        || plan.context.source_mode != "retroarch_bezel"
        || plan.entries.is_empty()
    {
        return Err(failure(
            SharedApplyFailureKind::InvalidPlan,
            None,
            "RetroArch bezel verification requires a RetroArch bezel transaction",
        ));
    }
    for entry in &mut plan.entries {
        entry.content_verification = Some(SharedContentVerification::RetroArchBezel);
    }
    plan.plan_id.clear();
    plan.plan_id = plan_digest(plan)?;
    Ok(())
}

pub fn execute_shared_apply(
    plan: &SharedTransactionPlan,
    options: &SharedApplyOptions,
) -> SharedApplyResult {
    execute_shared_apply_internal(plan, options, None)
}

pub fn execute_shared_materialized_apply(
    plan: &SharedTransactionPlan,
    options: &SharedApplyOptions,
    materialized: &SharedMaterializedOutput,
) -> SharedApplyResult {
    execute_shared_apply_internal(plan, options, Some(materialized))
}

fn execute_shared_apply_internal(
    plan: &SharedTransactionPlan,
    options: &SharedApplyOptions,
    materialized: Option<&SharedMaterializedOutput>,
) -> SharedApplyResult {
    let effective_dry_run = options.dry_run
        || options
            .confirmation
            .as_ref()
            .is_none_or(|confirmation| !confirmation.general_approved);
    let mut journal = SharedApplyJournal {
        schema_version: SHARED_APPLY_SCHEMA_VERSION,
        operation_id: options.operation_id.clone(),
        plan_id: plan.plan_id.clone(),
        timestamp_unix_seconds: options.timestamp_unix_seconds,
        context: plan.context.clone(),
        approved_source_root: plan.approved_source_root.clone(),
        destination_root: plan.destination_root.clone(),
        created_root_directories: Vec::new(),
        dry_run: effective_dry_run,
        entries: Vec::new(),
        status: SharedApplyStatus::DryRun,
        rollback_operation_id: None,
    };
    let confirmation = options.confirmation.as_ref();
    let context_valid = options.current_context == plan.context;
    let plan_valid = plan_digest(plan).ok().as_deref() == Some(plan.plan_id.as_str());
    let confirmation_valid =
        confirmation.is_some_and(|confirmation| confirmation.plan_id == plan.plan_id);
    let destination_root = plan.destination_root.to_path_buf();
    let source_root = plan.approved_source_root.to_path_buf();
    if !context_valid || !plan_valid || (!effective_dry_run && !confirmation_valid) {
        let kind = if !confirmation_valid {
            SharedApplyFailureKind::ConfirmationPlanMismatch
        } else {
            SharedApplyFailureKind::InvalidPlan
        };
        journal.entries = plan
            .entries
            .iter()
            .map(|entry| failed_entry(entry, kind, "plan or context changed before apply"))
            .collect();
        journal.status = SharedApplyStatus::Failed;
        return SharedApplyResult {
            journal,
            journal_path: None,
            journal_failure: None,
        };
    }
    let (Ok(destination_root), Ok(source_root)) = (destination_root, source_root) else {
        journal.entries = plan
            .entries
            .iter()
            .map(|entry| {
                failed_entry(
                    entry,
                    SharedApplyFailureKind::InvalidPlan,
                    "path identity cannot be reconstructed",
                )
            })
            .collect();
        journal.status = SharedApplyStatus::Failed;
        return SharedApplyResult {
            journal,
            journal_path: None,
            journal_failure: None,
        };
    };
    if !effective_dry_run {
        let operation = safe_identifier(&options.operation_id);
        let duplicate = operation.as_ref().ok().is_some_and(|operation| {
            fs::symlink_metadata(options.history_root.join(format!("{operation}.json"))).is_ok()
        });
        let pending_duplicate = operation.as_ref().ok().is_some_and(|operation| {
            fs::symlink_metadata(
                options
                    .history_root
                    .join(format!("{operation}.pending.json")),
            )
            .is_ok()
        });
        let managed_overlap = roots_overlap(&options.history_root, &source_root)
            || roots_overlap(&options.history_root, &destination_root)
            || roots_overlap(&options.backup_root, &source_root)
            || roots_overlap(&options.backup_root, &destination_root);
        if operation.is_err() || duplicate || pending_duplicate || managed_overlap {
            let (kind, detail) = if duplicate || pending_duplicate {
                (
                    SharedApplyFailureKind::DuplicateOperationId,
                    "operation ID already has a journal",
                )
            } else if managed_overlap {
                (
                    SharedApplyFailureKind::ManagedRootUnsafe,
                    "managed history or backup roots overlap source or destination scope",
                )
            } else {
                (
                    SharedApplyFailureKind::InvalidPlan,
                    "operation ID is invalid",
                )
            };
            journal.entries = plan
                .entries
                .iter()
                .map(|entry| failed_entry(entry, kind, detail))
                .collect();
            journal.status = SharedApplyStatus::Failed;
            return SharedApplyResult {
                journal,
                journal_path: None,
                journal_failure: None,
            };
        }
    }
    let pending_path = pending_journal_path(&options.history_root, &options.operation_id).ok();
    let mut durable = durable_journal_from_plan(plan, options);
    if !effective_dry_run {
        if let Err(error) = write_pending_journal(&durable, &options.history_root) {
            journal.entries = plan
                .entries
                .iter()
                .map(|entry| failed_entry(entry, error.kind, "durable intent could not be written"))
                .collect();
            journal.status = SharedApplyStatus::Failed;
            return SharedApplyResult {
                journal,
                journal_path: None,
                journal_failure: Some(error),
            };
        }
        durable.transaction_state = SharedRecoveryState::Applying;
        if let Err(error) = write_pending_journal(&durable, &options.history_root) {
            journal.status = SharedApplyStatus::Failed;
            return SharedApplyResult {
                journal,
                journal_path: None,
                journal_failure: Some(error),
            };
        }
    }
    // Before anything in the destination is touched, the backup store is
    // told which install this is: this operation, for this destination
    // root, with exactly these files. Undo later holds the journal to it.
    if !effective_dry_run
        && let Err(kind) = write_operation_receipt(&journal, plan, &options.backup_root)
    {
        journal.entries = plan
            .entries
            .iter()
            .map(|entry| {
                failed_entry(
                    entry,
                    kind,
                    "EmuWiz could not record which install this is, so nothing was changed",
                )
            })
            .collect();
        journal.status = SharedApplyStatus::Failed;
        return SharedApplyResult {
            journal,
            journal_path: None,
            journal_failure: None,
        };
    }
    let mut lock = None;
    let mut created_root_directories: Vec<CreatedRootDirectory> = Vec::new();
    if !effective_dry_run {
        // Bootstrap is only ever attempted for a plan the preview step
        // already flagged as needing parent creation (`DestinationParentsMissing`,
        // surfaced through `SharedPlanEntry::parent_creation_approved`) -
        // the exact same consent an ordinary missing child directory
        // already requires under `apply_one`. A missing root the user was
        // never shown falls straight through to `RootLock::acquire` below,
        // unchanged from before this existed.
        let root_creation_approved = plan
            .entries
            .iter()
            .any(|entry| entry.parent_creation_approved);
        if root_creation_approved {
            match bootstrap_missing_destination_root(&destination_root) {
                Ok(created) => created_root_directories = created,
                Err(kind) => {
                    journal.entries = plan
                        .entries
                        .iter()
                        .map(|entry| {
                            failed_entry(
                                entry,
                                kind,
                                "destination root could not be safely created",
                            )
                        })
                        .collect();
                    journal.status = SharedApplyStatus::Failed;
                    return SharedApplyResult {
                        journal,
                        journal_path: None,
                        journal_failure: None,
                    };
                }
            }
        }
        match RootLock::acquire(&destination_root, LOCK_TIMEOUT) {
            Ok(guard) => lock = Some(guard),
            Err(kind) => {
                // Nothing was journaled yet, so an empty root this call
                // itself just created would otherwise be orphaned with no
                // journal ever able to clean it up later. Descriptor-
                // anchored and identity-verified, exactly like real
                // rollback - see `remove_verified_root_chain`.
                remove_verified_root_chain(&created_root_directories);
                journal.entries = plan
                    .entries
                    .iter()
                    .map(|entry| failed_entry(entry, kind, "destination root is busy"))
                    .collect();
                journal.status = SharedApplyStatus::Failed;
                return SharedApplyResult {
                    journal,
                    journal_path: None,
                    journal_failure: None,
                };
            }
        }
        durable.created_root_directories = created_root_directories
            .iter()
            .map(|entry| SharedCreatedRootDirectory {
                path: SharedTransactionPath::from_path(&entry.path),
                identity: Some(entry.identity),
            })
            .collect();
        if let Err(error) = write_pending_journal(&durable, &options.history_root) {
            journal.status = SharedApplyStatus::Failed;
            return SharedApplyResult {
                journal,
                journal_path: None,
                journal_failure: Some(error),
            };
        }
    }
    let replacement_approved = confirmation.is_some_and(|value| value.replacement_approved);
    let mut written = 0_u64;
    let mut backup_bytes = 0_u64;
    for (index, entry) in plan.entries.iter().enumerate() {
        let mut checkpoint = |state: SharedEntryRecoveryState, value: &SharedApplyEntry| {
            update_durable_entry(&mut durable, index, value);
            if let Some(durable_entry) = durable.entries.get_mut(index) {
                durable_entry.state = state;
            }
            write_pending_journal(&durable, &options.history_root).is_ok()
        };
        let applied_entry = apply_one(
            entry,
            &source_root,
            &destination_root,
            &options.backup_root,
            &options.operation_id,
            effective_dry_run,
            replacement_approved,
            &mut written,
            &mut backup_bytes,
            materialized,
            &mut checkpoint,
        );
        journal.entries.push(applied_entry.clone());
        update_durable_entry(&mut durable, index, &applied_entry);
        if !effective_dry_run {
            if should_inject(FaultPoint::ApplyCheckpoint)
                || should_inject_after(FaultPoint::ApplyCheckpoint)
            {
                durable.transaction_state = SharedRecoveryState::Applying;
                let error = failure(
                    SharedApplyFailureKind::JournalFailed,
                    None,
                    "injected interruption before apply checkpoint",
                );
                return SharedApplyResult {
                    journal,
                    journal_path: None,
                    journal_failure: Some(error),
                };
            }
            if let Err(error) = write_pending_journal(&durable, &options.history_root) {
                durable.transaction_state = SharedRecoveryState::ApplyFailed;
                let _ = write_pending_journal(&durable, &options.history_root);
                return SharedApplyResult {
                    journal,
                    journal_path: None,
                    journal_failure: Some(error),
                };
            }
        }
    }
    if !effective_dry_run && !created_root_directories.is_empty() {
        let any_write = journal.entries.iter().any(|entry| {
            matches!(
                entry.outcome,
                SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
            )
        });
        if !any_write {
            // Nothing was actually installed under the bootstrap-created
            // root: clean it up immediately, in this same call (the lock
            // is still held), rather than leaving an empty root a journal
            // would otherwise claim ownership of that normal rollback can
            // never reach - rollback requires at least one installed,
            // `Available` per-file entry, which an all-failed apply never
            // has. Truncated to whatever `remove_verified_root_chain`
            // actually removed (deepest-first, stopping at the first
            // unremovable level), so the journal below never claims
            // ownership of something already gone from disk.
            let removed = remove_verified_root_chain(&created_root_directories);
            created_root_directories.truncate(created_root_directories.len() - removed);
        }
    }
    // The root lock is held until the receipt is durable and the pending
    // record retired (below): startup recovery in another thread or
    // process treats a locked root as a running apply and leaves it alone,
    // so it can never settle this apply as "interrupted" in the window
    // between the last write and the receipt.
    journal.created_root_directories = created_root_directories
        .iter()
        .map(|entry| SharedCreatedRootDirectory {
            path: SharedTransactionPath::from_path(&entry.path),
            identity: Some(entry.identity),
        })
        .collect();
    journal.status = derive_status(&journal.entries, effective_dry_run);
    log::info!(
        "shared apply {}: {:?}, {} entr(y/ies), {} byte(s) written",
        journal.operation_id,
        journal.status,
        journal.entries.len(),
        written,
    );
    if effective_dry_run {
        drop(lock);
        return SharedApplyResult {
            journal,
            journal_path: None,
            journal_failure: None,
        };
    }
    if should_inject(FaultPoint::TerminalJournal) {
        drop(lock);
        return SharedApplyResult {
            journal,
            journal_path: None,
            journal_failure: Some(failure(
                SharedApplyFailureKind::JournalFailed,
                None,
                "injected interruption before terminal journal write",
            )),
        };
    }
    let result = match write_journal_once(&journal, &options.history_root) {
        Ok(path) => {
            log::info!(
                "shared apply {}: journal written to {}",
                journal.operation_id,
                path.display(),
            );
            durable.transaction_state = match journal.status {
                SharedApplyStatus::Success => SharedRecoveryState::Applied,
                SharedApplyStatus::PartialFailure => SharedRecoveryState::ApplyFailed,
                SharedApplyStatus::Failed => SharedRecoveryState::ApplyFailed,
                SharedApplyStatus::DryRun => SharedRecoveryState::Planned,
            };
            let _ = write_pending_journal(&durable, &options.history_root);
            let _ = pending_path.and_then(|pending| fs::remove_file(pending).ok());
            // PCSX2 may chain a legacy-PNACH migration that re-reads this
            // same staged root, so its caller retires the root afterwards.
            if journal.status == SharedApplyStatus::Success
                && plan.context.adapter != PreviewAdapter::Pcsx2
            {
                // The terminal journal and backups are all Undo needs; the
                // job-private staged source is spent.
                super::staging_job::retire_completed_staging_job(
                    &plan.approved_source_root.to_path_buf().unwrap_or_default(),
                );
            }
            SharedApplyResult {
                journal,
                journal_path: Some(path),
                journal_failure: None,
            }
        }
        Err(error) => {
            log::warn!(
                "shared apply {}: journal write failed: {:?} ({})",
                journal.operation_id,
                error.kind,
                error.detail,
            );
            let any_write = journal.entries.iter().any(|entry| {
                matches!(
                    entry.outcome,
                    SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
                )
            });
            if any_write {
                journal.status = SharedApplyStatus::PartialFailure;
                for entry in &mut journal.entries {
                    if matches!(
                        entry.outcome,
                        SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
                    ) {
                        entry
                            .stages
                            .push(SharedTransactionStage::JournalFailedAfterSuccessfulWrite);
                    }
                }
            }
            durable.transaction_state = SharedRecoveryState::ApplyFailed;
            let _ = write_pending_journal(&durable, &options.history_root);
            SharedApplyResult {
                journal,
                journal_path: None,
                journal_failure: Some(error),
            }
        }
    };
    drop(lock);
    result
}

#[allow(clippy::too_many_arguments)]
fn apply_one(
    plan: &SharedPlanEntry,
    source_root: &Path,
    destination_root: &Path,
    backup_root: &Path,
    operation_id: &str,
    dry_run: bool,
    replacement_approved: bool,
    written: &mut u64,
    backup_bytes: &mut u64,
    materialized: Option<&SharedMaterializedOutput>,
    checkpoint: &mut dyn FnMut(SharedEntryRecoveryState, &SharedApplyEntry) -> bool,
) -> SharedApplyEntry {
    let mut result = SharedApplyEntry {
        plan_entry: plan.clone(),
        destination_existed_before_apply: None,
        destination_parent_existed_before_apply: None,
        observed_source_digest: None,
        observed_destination_digest: None,
        backup_path: None,
        backup_digest: None,
        destination_pre_mode: None,
        temporary_path: None,
        final_destination_digest: None,
        created_directories: Vec::new(),
        replacement_approved,
        verification_succeeded: false,
        outcome: SharedApplyOutcome::SkippedNotEligible,
        publication: SharedPublicationState::NotPublished,
        stages: vec![if dry_run {
            SharedTransactionStage::DryRun
        } else {
            SharedTransactionStage::InstallNew
        }],
        warnings: Vec::new(),
        failures: Vec::new(),
    };
    let Ok(source) = plan.source_path.to_path_buf() else {
        return fail_result(
            result,
            SharedApplyOutcome::SourceChanged,
            SharedApplyFailureKind::InvalidPlan,
            None,
            "invalid source path identity",
        );
    };
    if should_inject(FaultPoint::SourceMutation) {
        return fail_result(
            result,
            SharedApplyOutcome::SourceChanged,
            SharedApplyFailureKind::SourceChanged,
            Some(&source),
            "injected source mutation",
        );
    }
    let Ok(relative) = plan.destination_relative_path.to_path_buf() else {
        return fail_result(
            result,
            SharedApplyOutcome::SkippedConflict,
            SharedApplyFailureKind::InvalidPlan,
            None,
            "invalid destination path identity",
        );
    };
    if source.strip_prefix(source_root).is_err() || source == source_root {
        return fail_result(
            result,
            SharedApplyOutcome::SourceChanged,
            SharedApplyFailureKind::SourceOutsideApprovedScope,
            Some(&source),
            "source is outside approved adapter scope",
        );
    }
    let source_hash_limit = if materialized.is_some() {
        SHARED_MAX_MATERIALIZED_BYTES
    } else {
        SHARED_MAX_SOURCE_BYTES
    };
    let source_hash = match stable_hash(&source, source_hash_limit) {
        Ok(value) => value,
        Err(kind) => {
            return fail_result(
                result,
                SharedApplyOutcome::SourceChanged,
                kind,
                Some(&source),
                "source could not be revalidated",
            );
        }
    };
    result.observed_source_digest = Some(source_hash.digest.clone());
    if source_hash.digest != plan.source_digest {
        return fail_result(
            result,
            SharedApplyOutcome::SourceChanged,
            SharedApplyFailureKind::SourceChanged,
            Some(&source),
            "source digest changed since approved preview",
        );
    }
    let materialized = materialized.filter(|value| value.source_path == source);
    if let Some(value) = materialized {
        if value.source_digest != source_hash.digest
            || value.bytes.len() as u64 > SHARED_MAX_MATERIALIZED_BYTES
            || digest_bytes(&value.bytes) != value.output_digest
        {
            return fail_result(
                result,
                SharedApplyOutcome::SourceChanged,
                SharedApplyFailureKind::SourceChanged,
                Some(&source),
                "materialized output or source precondition changed",
            );
        }
        for (guard, expected) in &value.guard_paths {
            let observed = match stable_hash(guard, SHARED_MAX_MATERIALIZED_BYTES) {
                Ok(observed) => observed.digest,
                Err(kind) => {
                    return fail_result(
                        result,
                        SharedApplyOutcome::SourceChanged,
                        kind,
                        Some(guard),
                        "materialized output guard could not be revalidated",
                    );
                }
            };
            if &observed != expected {
                return fail_result(
                    result,
                    SharedApplyOutcome::SourceChanged,
                    SharedApplyFailureKind::SourceChanged,
                    Some(guard),
                    "materialized output guard changed since review",
                );
            }
        }
    }
    let output_bytes = materialized.map_or(source_hash.bytes, |value| value.bytes.len() as u64);
    if output_bytes > SHARED_MAX_TOTAL_WRITTEN_BYTES.saturating_sub(*written) {
        return fail_result(
            result,
            SharedApplyOutcome::WriteFailed,
            SharedApplyFailureKind::ResourceLimitReached,
            Some(&source),
            "transaction write-byte limit reached",
        );
    }
    let nested_mod_package = matches!(
        plan.content_verification.as_ref(),
        Some(
            SharedContentVerification::LocalModPackage
                | SharedContentVerification::CemuGraphicPack
                | SharedContentVerification::Rpcs3OrdinaryMod
                | SharedContentVerification::RetroArchBezel
                | SharedContentVerification::HackHashPatch { .. }
        )
    ) || matches!(
        plan.adapter,
        PreviewAdapter::Flycast
            | PreviewAdapter::MelonDs
            | PreviewAdapter::Mgba
            | PreviewAdapter::AmigaWhdloadTrainer
            | PreviewAdapter::ScummVmTrainer
    ) || (plan.adapter == PreviewAdapter::Ppsspp
        && relative == Path::new(&format!("{}.ini", plan.verified_game_identity)))
        || (plan.adapter == PreviewAdapter::Rpcs3Patch
            && relative == Path::new("patch_config.yml"));
    let assessment = if nested_mod_package {
        assess_local_mod_destination(destination_root, &relative)
    } else {
        let Some((category, filename)) = exactly_two_components(&relative) else {
            return fail_result(
                result,
                SharedApplyOutcome::SkippedConflict,
                SharedApplyFailureKind::DestinationUnsafe,
                None,
                "destination must contain exactly two normal components",
            );
        };
        assess_destination(destination_root, &category, &filename)
            .map(|assessment| {
                (
                    assessment.destination_state,
                    assessment.proposed_destination.path().to_path_buf(),
                )
            })
            .map_err(|error| {
                failure(
                    SharedApplyFailureKind::DestinationUnsafe,
                    Some(&error.path),
                    &error.to_string(),
                )
            })
    };
    let (destination_state, destination) = match assessment {
        Ok(value) => value,
        Err(error) => {
            return fail_result(
                result,
                SharedApplyOutcome::DestinationChanged,
                SharedApplyFailureKind::DestinationUnsafe,
                Some(destination_root),
                &error.detail,
            );
        }
    };
    let parent = destination.parent().unwrap_or(destination_root);
    let current = if destination_state == DestinationState::RegularFile {
        match stable_hash(&destination, SHARED_MAX_SOURCE_BYTES) {
            Ok(value) => Some(value),
            Err(kind) => {
                return fail_result(
                    result,
                    SharedApplyOutcome::DestinationChanged,
                    kind,
                    Some(&destination),
                    "destination could not be revalidated",
                );
            }
        }
    } else {
        None
    };
    result.destination_existed_before_apply = Some(current.is_some());
    result.destination_parent_existed_before_apply = Some(parent.exists());
    result.observed_destination_digest = current.as_ref().map(|value| value.digest.clone());
    if should_inject(FaultPoint::DestinationMutation) {
        return fail_result(
            result,
            SharedApplyOutcome::DestinationChanged,
            SharedApplyFailureKind::DestinationChanged,
            Some(&destination),
            "injected destination mutation",
        );
    }
    let expected_state_matches = match plan.proposed_action {
        PreviewProposedAction::Install => destination_state == DestinationState::Absent,
        PreviewProposedAction::Replace | PreviewProposedAction::Skip => {
            destination_state == DestinationState::RegularFile
        }
        PreviewProposedAction::Blocked => false,
    };
    if !expected_state_matches
        || current.as_ref().map(|value| value.digest.as_str())
            != plan.destination_pre_digest.as_deref()
    {
        return fail_result(
            result,
            SharedApplyOutcome::DestinationChanged,
            SharedApplyFailureKind::DestinationChanged,
            Some(&destination),
            "destination state or digest changed since approved preview (another install may have written it); nothing was changed - review the current file and preview again",
        );
    }
    if plan.proposed_action == PreviewProposedAction::Skip {
        if let Err(detail) = verify_entry_content(plan, &destination) {
            return fail_result(
                result,
                SharedApplyOutcome::VerificationFailed,
                SharedApplyFailureKind::VerificationFailed,
                Some(&destination),
                &detail,
            );
        }
        result.outcome = SharedApplyOutcome::AlreadyInstalled;
        result.final_destination_digest = Some(source_hash.digest);
        result.verification_succeeded = true;
        result.stages.push(SharedTransactionStage::AlreadyInstalled);
        return result;
    }
    if plan.proposed_action == PreviewProposedAction::Replace && !replacement_approved {
        return fail_result(
            result,
            SharedApplyOutcome::SkippedReplacementNotApproved,
            SharedApplyFailureKind::ReplacementNotApproved,
            Some(&destination),
            "replacement requires separate explicit permission",
        );
    }
    if dry_run {
        result.outcome = SharedApplyOutcome::DryRun;
        return result;
    }
    if !parent.exists() {
        let adapter_allows_parent_creation = matches!(
            plan.adapter,
            PreviewAdapter::RetroArch
                | PreviewAdapter::Pcsx2
                | PreviewAdapter::DuckStation
                | PreviewAdapter::MelonDs
                | PreviewAdapter::Dolphin
                | PreviewAdapter::Ppsspp
                | PreviewAdapter::Xenia
                | PreviewAdapter::LocalModPackage
                | PreviewAdapter::CemuGraphicPack
                | PreviewAdapter::Rpcs3OrdinaryMod
                | PreviewAdapter::Mednafen
                | PreviewAdapter::Fbneo
                | PreviewAdapter::AmigaWhdloadTrainer
                | PreviewAdapter::ScummVmTrainer
        );
        if !plan.parent_creation_approved || !adapter_allows_parent_creation {
            return fail_result(
                result,
                SharedApplyOutcome::WriteFailed,
                SharedApplyFailureKind::ParentCreationFailed,
                Some(parent),
                "parent creation was not approved by preview and adapter contract",
            );
        }
        let create = if nested_mod_package {
            create_local_mod_parents(destination_root, parent)
        } else {
            create_one_parent(destination_root, parent).map(|()| vec![parent.to_path_buf()])
        };
        let created_dirs = match create {
            Ok(dirs) => dirs,
            Err(kind) => {
                return fail_result(
                    result,
                    SharedApplyOutcome::WriteFailed,
                    kind,
                    Some(parent),
                    "approved destination parent could not be created safely",
                );
            }
        };
        // Every directory level this transaction actually created, so rollback
        // removes each one (innermost first) and leaves no empty scaffold
        // behind - not just the immediate parent.
        for dir in &created_dirs {
            result
                .created_directories
                .push(SharedTransactionPath::from_path(dir));
        }
    }
    let output_digest = materialized.map_or_else(
        || source_hash.digest.clone(),
        |value| value.output_digest.clone(),
    );
    if plan.proposed_action == PreviewProposedAction::Replace {
        let Some(existing) = current.as_ref() else {
            return fail_result(
                result,
                SharedApplyOutcome::DestinationChanged,
                SharedApplyFailureKind::DestinationChanged,
                Some(&destination),
                "replacement destination disappeared",
            );
        };
        if existing.bytes > SHARED_MAX_BACKUP_BYTES.saturating_sub(*backup_bytes) {
            return fail_result(
                result,
                SharedApplyOutcome::BackupFailed,
                SharedApplyFailureKind::ResourceLimitReached,
                Some(&destination),
                "backup-byte limit reached",
            );
        }
        match create_backup(&destination, &existing.digest, backup_root, operation_id) {
            Ok(path) => {
                *backup_bytes += existing.bytes;
                result.backup_path = Some(SharedTransactionPath::from_path(&path));
                result.backup_digest = Some(existing.digest.clone());
                result.destination_pre_mode = existing.pin.map(|pin| pin.mode);
                result.stages.push(SharedTransactionStage::BackupCreated);
                if !checkpoint(SharedEntryRecoveryState::BackupCreated, &result) {
                    return fail_result(
                        result,
                        SharedApplyOutcome::BackupFailed,
                        SharedApplyFailureKind::JournalFailed,
                        Some(&destination),
                        "backup was created but its durable checkpoint failed",
                    );
                }
            }
            Err(error) => {
                result.stages.push(SharedTransactionStage::BackupFailed);
                return fail_result(
                    result,
                    SharedApplyOutcome::BackupFailed,
                    error,
                    Some(&destination),
                    "verified backup could not be created; original left untouched",
                );
            }
        }
    }
    if !checkpoint(SharedEntryRecoveryState::Applying, &result) {
        return fail_result(
            result,
            SharedApplyOutcome::WriteFailed,
            SharedApplyFailureKind::JournalFailed,
            Some(&destination),
            "destination mutation intent checkpoint failed",
        );
    }
    // What is about to be put at the destination goes into the backup
    // store first, so that Undo later has something other than the journal
    // to show what this install wrote: for a new file, that the file is
    // this install's at all; for a replacement, what was put over the
    // original.
    let receipt_kind = match plan.proposed_action {
        PreviewProposedAction::Install => Some(INSTALL_RECEIPT_SUFFIX),
        PreviewProposedAction::Replace => Some(REPLACEMENT_RECEIPT_SUFFIX),
        _ => None,
    };
    if let Some(kind) = receipt_kind
        && let Err(error) = write_receipt(
            kind,
            destination_root,
            &relative,
            &output_digest,
            backup_root,
            operation_id,
        )
    {
        return fail_result(
            result,
            SharedApplyOutcome::BackupFailed,
            error,
            Some(&destination),
            "EmuWiz could not record what it is about to install here, so the file was \
             left as it was",
        );
    }
    // A replaced user file keeps the bits recorded with its backup; a new
    // file keeps the owner-only creation policy.
    let write_mode = if plan.proposed_action == PreviewProposedAction::Replace {
        let permissions = SharedRollbackPermissions::for_recorded_mode(result.destination_pre_mode);
        if let SharedRollbackPermissions::NotRestorable { mode } = permissions {
            result.warnings.push(format!(
                "existing permissions {mode:04o} were not kept; the replaced file is owner-only {OWNER_ONLY_MODE:04o}"
            ));
        }
        permissions.restored_mode()
    } else {
        OWNER_ONLY_MODE
    };
    // A replacement may only displace the file observed above, whose bytes
    // were backed up and whose mode was recorded.
    let displaces = (plan.proposed_action == PreviewProposedAction::Replace)
        .then(|| {
            current.as_ref().and_then(|existing| {
                existing.pin.map(|pin| Displaced {
                    pin,
                    digest: existing.digest.clone(),
                })
            })
        })
        .flatten();
    let write_result = match materialized {
        Some(value) => atomic_write_bytes(
            &value.bytes,
            &destination,
            &output_digest,
            plan.proposed_action == PreviewProposedAction::Install,
            write_mode,
            displaces.clone(),
        ),
        None => atomic_write(
            &source,
            &destination,
            &output_digest,
            plan.proposed_action == PreviewProposedAction::Install,
            write_mode,
            displaces,
        ),
    };
    match write_result {
        Ok(temp) => {
            result.temporary_path = Some(SharedTransactionPath::from_path(&temp));
            // The output is live from here on; any later failure leaves it.
            result.publication = SharedPublicationState::PublishedThenFailed;
            if should_inject(FaultPoint::ReplacedCheckpoint)
                || !checkpoint(SharedEntryRecoveryState::DestinationReplaced, &result)
            {
                return fail_result(
                    result,
                    SharedApplyOutcome::WriteFailed,
                    SharedApplyFailureKind::JournalFailed,
                    Some(&destination),
                    "destination was replaced but its durable checkpoint failed",
                );
            }
            match verify_entry_content(plan, &destination) {
                Ok(()) => {
                    result.final_destination_digest = Some(output_digest.clone());
                    result.verification_succeeded = true;
                    result.outcome = if plan.proposed_action == PreviewProposedAction::Install {
                        SharedApplyOutcome::InstalledNew
                    } else {
                        SharedApplyOutcome::ReplacedExisting
                    };
                    result.stages.push(SharedTransactionStage::Success);
                    result.publication = SharedPublicationState::Published;
                    if !checkpoint(SharedEntryRecoveryState::DestinationVerified, &result) {
                        result.publication = SharedPublicationState::PublishedThenFailed;
                        return fail_result(
                            result,
                            SharedApplyOutcome::VerificationFailed,
                            SharedApplyFailureKind::JournalFailed,
                            Some(&destination),
                            "verified destination checkpoint failed",
                        );
                    }
                    *written += output_bytes;
                }
                Err(detail) => {
                    let restore = restore_after_failed_verification(
                        plan,
                        &result,
                        &destination,
                        &output_digest,
                    );
                    result.publication = if restore.is_ok() {
                        SharedPublicationState::PublishedThenRestored
                    } else {
                        SharedPublicationState::PublishedThenFailed
                    };
                    let detail = match restore {
                        Ok(()) => format!("{detail}; previous live file state restored"),
                        Err(kind) => format!(
                            "{detail}; restoring the previous live file state failed: {kind:?}"
                        ),
                    };
                    result = fail_result(
                        result,
                        SharedApplyOutcome::VerificationFailed,
                        SharedApplyFailureKind::VerificationFailed,
                        Some(&destination),
                        &detail,
                    );
                }
            }
        }
        Err((kind, temp)) => {
            result.temporary_path = temp.as_deref().map(SharedTransactionPath::from_path);
            result.publication = publication_after_write_error(kind);
            let (outcome, detail) = publication_failure(kind, temp.as_deref());
            result = fail_result(result, outcome, kind, Some(&destination), &detail);
        }
    }
    // The receipt was written before the file. If what it describes is
    // definitely not there now, the receipt goes too. The early returns
    // above all leave the file published, and keep it.
    if let Some(kind) = receipt_kind
        && matches!(
            result.publication,
            SharedPublicationState::NotPublished | SharedPublicationState::PublishedThenRestored
        )
    {
        withdraw_receipt(kind, destination_root, &relative, backup_root, operation_id);
    }
    result
}

fn verify_entry_content(plan: &SharedPlanEntry, destination: &Path) -> Result<(), String> {
    let Some(contract) = &plan.content_verification else {
        return Ok(());
    };
    if let SharedContentVerification::HackHashPatch {
        expected_output_hash,
        ..
    } = contract
    {
        let observed = stable_hash(destination, SHARED_MAX_MATERIALIZED_BYTES)
            .map_err(|kind| format!("live patched ROM could not be re-read: {kind:?}"))?;
        return if observed.digest == *expected_output_hash {
            Ok(())
        } else {
            Err("live patched ROM hash differs from the prepared output".into())
        };
    }
    // These contracts have no text-level check, and their files (bezel
    // images, mod packages) are often binary, so a UTF-8 read would only
    // reject correct installs.
    if matches!(
        contract,
        SharedContentVerification::LocalModPackage
            | SharedContentVerification::CemuGraphicPack
            | SharedContentVerification::Rpcs3OrdinaryMod
            | SharedContentVerification::RetroArchBezel
    ) {
        return Ok(());
    }
    let bytes = read_bounded(destination, SHARED_MAX_SOURCE_BYTES)
        .map_err(|kind| format!("live target could not be re-read: {kind:?}"))?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| "live target is not valid UTF-8".to_string())?;
    match contract {
        SharedContentVerification::DolphinManagedGameHacking {
            expected_managed_names,
            require_managed_section,
            require_code_section,
        } => {
            let document = super::gecko_document::parse_dolphin_ini(text);
            let section_names = document.section_names();
            let has_managed = section_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case("ArchiveFS_Managed_GameHacking"));
            if *require_managed_section && !has_managed {
                return Err("live target is missing [ArchiveFS_Managed_GameHacking]".to_string());
            }
            let has_code_section = section_names.iter().any(|name| {
                name.eq_ignore_ascii_case("ActionReplay") || name.eq_ignore_ascii_case("Gecko")
            });
            if *require_code_section && !has_code_section {
                return Err("live target has neither [ActionReplay] nor [Gecko]".to_string());
            }
            let managed: BTreeSet<String> = document
                .named_section_lines("ArchiveFS_Managed_GameHacking")
                .into_iter()
                .filter_map(|line| line.trim().strip_prefix('$').map(str::to_string))
                .collect();
            for expected in expected_managed_names {
                if !managed.contains(expected) {
                    return Err(format!(
                        "live target managed section is missing '${expected}'"
                    ));
                }
                let defined = document
                    .action_replay_codes
                    .iter()
                    .chain(document.gecko_codes.iter())
                    .any(|code| code.name == *expected);
                if !defined {
                    return Err(format!(
                        "live target has no ActionReplay/Gecko definition for '${expected}'"
                    ));
                }
            }
            Ok(())
        }
        SharedContentVerification::LocalModPackage
        | SharedContentVerification::CemuGraphicPack
        | SharedContentVerification::Rpcs3OrdinaryMod
        | SharedContentVerification::RetroArchBezel
        | SharedContentVerification::HackHashPatch { .. } => Ok(()),
    }
}

/// Revalidate an arbitrary, safe relative path for a local mod package. The
/// existing adapter contracts intentionally restrict their destinations to
/// two components; the genuinely-new part here is walking N parents. Root
/// validation and per-component traversal/absolute-path checks are delegated
/// to `destination_safety` rather than reimplemented.
fn assess_local_mod_destination(
    root: &Path,
    relative: &Path,
) -> Result<(DestinationState, PathBuf), SharedApplyFailure> {
    validate_destination_root(root).map_err(|error| {
        failure(
            SharedApplyFailureKind::DestinationUnsafe,
            Some(root),
            &error.to_string(),
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Err(failure(
            SharedApplyFailureKind::DestinationUnsafe,
            Some(relative),
            "local mod destination is empty",
        ));
    }
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(failure(
                SharedApplyFailureKind::DestinationUnsafe,
                Some(relative),
                "local mod destination must contain only normal relative components",
            ));
        };
        validate_single_component(name, root).map_err(|error| {
            failure(
                SharedApplyFailureKind::DestinationUnsafe,
                Some(&error.path),
                &error.to_string(),
            )
        })?;
    }
    let destination = root.join(relative);
    let mut current = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for component in &components[..components.len().saturating_sub(1)] {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(failure(
                    SharedApplyFailureKind::DestinationUnsafe,
                    Some(&current),
                    "local mod destination parent is a symlink",
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(failure(
                    SharedApplyFailureKind::DestinationUnsafe,
                    Some(&current),
                    "local mod destination parent is not a directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(failure(
                    SharedApplyFailureKind::DestinationUnsafe,
                    Some(&current),
                    &format!("cannot inspect local mod destination parent: {error}"),
                ));
            }
        }
    }
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(failure(
            SharedApplyFailureKind::DestinationUnsafe,
            Some(&destination),
            "local mod destination is a symlink",
        )),
        Ok(metadata) if metadata.is_file() => Ok((DestinationState::RegularFile, destination)),
        Ok(_) => Err(failure(
            SharedApplyFailureKind::DestinationUnsafe,
            Some(&destination),
            "local mod destination is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok((DestinationState::Absent, destination))
        }
        Err(error) => Err(failure(
            SharedApplyFailureKind::DestinationUnsafe,
            Some(&destination),
            &format!("cannot inspect local mod destination: {error}"),
        )),
    }
}

fn restore_after_failed_verification(
    plan: &SharedPlanEntry,
    result: &SharedApplyEntry,
    destination: &Path,
    installed_digest: &str,
) -> Result<(), SharedApplyFailureKind> {
    if plan.proposed_action == PreviewProposedAction::Install {
        return remove_and_verify_new_destination(destination, installed_digest)
            .map_err(|(kind, _kept)| kind);
    }
    let backup = result
        .backup_path
        .as_ref()
        .ok_or(SharedApplyFailureKind::BackupMissing)?
        .to_path_buf()?;
    let digest = result
        .backup_digest
        .as_deref()
        .ok_or(SharedApplyFailureKind::BackupChanged)?;
    let mode =
        SharedRollbackPermissions::for_recorded_mode(result.destination_pre_mode).restored_mode();
    let installed = pinned_installed_file(destination, installed_digest)?;
    atomic_write(&backup, destination, digest, false, mode, installed)
        .map(|_| ())
        .map_err(|(kind, _)| kind)
}

/// Where a failed `atomic_write`/`atomic_write_bytes` left the destination.
/// Every error kind but these two is returned before the rename or exchange
/// that publishes, or after a refused exchange was put back.
fn publication_after_write_error(kind: SharedApplyFailureKind) -> SharedPublicationState {
    match kind {
        // Published, then the final re-read did not match.
        SharedApplyFailureKind::PublishedUnverified => SharedPublicationState::PublishedThenFailed,
        // Either ours stayed published (reverse exchange failed) or a file
        // that is not ours came back: which one is live is not known.
        SharedApplyFailureKind::DisplacedFileKept => SharedPublicationState::Uncertain,
        _ => SharedPublicationState::NotPublished,
    }
}

/// The outcome and plain detail for a refused or failed publication.
fn publication_failure(
    kind: SharedApplyFailureKind,
    temp: Option<&Path>,
) -> (SharedApplyOutcome, String) {
    match kind {
        SharedApplyFailureKind::VerificationFailed => (
            SharedApplyOutcome::VerificationFailed,
            "atomic destination write failed".into(),
        ),
        SharedApplyFailureKind::DestinationChanged => (
            SharedApplyOutcome::DestinationChanged,
            "the file changed after it was checked and backed up; it was left as it is and nothing was changed".into(),
        ),
        SharedApplyFailureKind::AtomicReplaceUnsupported => (
            SharedApplyOutcome::WriteFailed,
            "this folder's filesystem cannot replace files atomically, so EmuWiz refused and changed nothing".into(),
        ),
        SharedApplyFailureKind::DisplacedFileKept => (
            SharedApplyOutcome::WriteFailed,
            format!(
                "The file changed while EmuWiz was writing it. To avoid losing anything, a file EmuWiz did not write is kept at {}. Review it and the live file before retrying",
                temp.map_or_else(|| "an unknown temporary name".into(), |path| path.display().to_string())
            ),
        ),
        SharedApplyFailureKind::PublishedUnverified => (
            SharedApplyOutcome::VerificationFailed,
            "EmuWiz wrote this file, but it changed straight afterwards, so the result could not be confirmed. Review it before retrying".into(),
        ),
        _ => (SharedApplyOutcome::WriteFailed, "atomic destination write failed".into()),
    }
}

/// The pin of the file a restore displaces: it must still hold exactly the
/// bytes this transaction installed.
fn pinned_installed_file(
    destination: &Path,
    installed_digest: &str,
) -> Result<Option<Displaced>, SharedApplyFailureKind> {
    let installed = stable_hash(destination, SHARED_MAX_MATERIALIZED_BYTES)
        .map_err(|_| SharedApplyFailureKind::DestinationChanged)?;
    if installed.digest != installed_digest {
        return Err(SharedApplyFailureKind::DestinationChanged);
    }
    Ok(installed.pin.map(|pin| Displaced {
        pin,
        digest: installed.digest,
    }))
}

pub fn discover_shared_apply_history(history_root: &Path) -> SharedHistoryReport {
    let mut report = SharedHistoryReport {
        journals: Vec::new(),
        warnings: Vec::new(),
        complete: true,
    };
    if strict_absolute_root(history_root).is_err() || !history_root.exists() {
        return report;
    }
    let Ok(read_dir) = fs::read_dir(history_root) else {
        report.complete = false;
        return report;
    };
    let mut paths = read_dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension() == Some(OsStr::new("json"))
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".rollback.json"))
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".pending.json"))
                // Adapter receipts that group several journals into one
                // logical operation are not journals themselves.
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".receipt.json"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    if paths.len() > SHARED_MAX_HISTORY_JOURNALS {
        paths.truncate(SHARED_MAX_HISTORY_JOURNALS);
        report.complete = false;
    }
    for path in paths {
        match read_journal(&path) {
            Ok(journal) => report
                .journals
                .push((SharedTransactionPath::from_path(&path), journal)),
            Err(error) if report.warnings.len() < SHARED_MAX_WARNINGS => {
                report.warnings.push(SharedJournalWarning {
                    path: SharedTransactionPath::from_path(&path),
                    failure: error,
                });
            }
            Err(_) => report.complete = false,
        }
    }
    report
}

pub fn preview_shared_rollback(
    journal_path: &Path,
    expected_destination_root: &Path,
    backup_root: &Path,
) -> SharedRollbackPreview {
    let journal = match read_journal(journal_path) {
        Ok(value) => value,
        Err(error) => {
            return SharedRollbackPreview {
                schema_version: SHARED_APPLY_SCHEMA_VERSION,
                preview_id: digest_text(&format!("{}:{:?}", journal_path.display(), error.kind)),
                journal_path: SharedTransactionPath::from_path(journal_path),
                original_operation_id: String::new(),
                destination_root: SharedTransactionPath::from_path(expected_destination_root),
                entries: vec![SharedRollbackEntry {
                    destination: None,
                    backup: None,
                    expected_installed_digest: None,
                    observed_destination_digest: None,
                    observed_backup_digest: None,
                    outcome: if error.kind == SharedApplyFailureKind::UnsupportedJournal {
                        SharedRollbackOutcome::JournalUnsupported
                    } else {
                        SharedRollbackOutcome::JournalMalformed
                    },
                    failure: Some(error),
                    permissions: None,
                }],
                available: false,
            };
        }
    };
    rollback_preview_of(
        &journal,
        journal_path,
        expected_destination_root,
        backup_root,
    )
}

/// The Undo preview for a journal that has already been read. Undo runs
/// this on the very journal it is about to act on, so that what was shown,
/// what was checked and what is done are the same bytes.
fn rollback_preview_of(
    journal: &SharedApplyJournal,
    journal_path: &Path,
    expected_destination_root: &Path,
    backup_root: &Path,
) -> SharedRollbackPreview {
    let journal_root = journal.destination_root.to_path_buf().ok();
    // The folder this Undo was opened for and the folder the install
    // recorded must be the same folder: the same names in the same order.
    // They need not be spelled alike (a trailing slash, a doubled slash).
    // Nothing is resolved, so a link that leads to the same folder under
    // another name is another path, and does not match.
    let root_matches = journal_root.as_deref() == Some(expected_destination_root);
    // From here on the install's own spelling of that folder is the one
    // used, for where Undo acts and for the names of the backups and
    // receipts the install made from it. A caller that spells the same
    // folder differently finds the same evidence; a caller that names a
    // different folder has already not matched.
    let expected_destination_root = match journal_root.as_deref() {
        Some(recorded) if root_matches => recorded,
        _ => expected_destination_root,
    };
    let rollback_marker_exists = journal_path.parent().is_some_and(|parent| {
        safe_identifier(&journal.operation_id)
            .ok()
            .is_some_and(|operation| parent.join(format!("{operation}.rollback.json")).exists())
    });
    // An Undo interrupted part-way leaves its pending record. An entry it
    // had reached is recorded `RollingBack` (checkpointed before the
    // removal) or `RolledBack` (after it); either is the proof that a
    // now-missing new file was removed by that Undo, which is exactly what
    // the approved Undo set out to do, and not by something else.
    let recorded_rolled_back: Vec<bool> = journal_path
        .parent()
        .zip(safe_identifier(&journal.operation_id).ok())
        .and_then(|(parent, operation)| {
            read_pending_journal(&parent.join(format!("{operation}.pending.json"))).ok()
        })
        .filter(|pending| {
            pending.operation_type == "shared_mod_rollback"
                && pending.rollback_of_operation_id.as_deref()
                    == Some(journal.operation_id.as_str())
        })
        .map(|pending| {
            pending
                .entries
                .iter()
                .map(|entry| {
                    matches!(
                        entry.state,
                        SharedEntryRecoveryState::RolledBack
                            | SharedEntryRecoveryState::RollingBack
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // Nothing in a journal decides where Undo acts: the folder this Undo
    // was opened for does. A journal that names anything outside it is
    // refused as a whole, never trimmed to the entries that look fine, and
    // before any file it names is opened.
    let violation = if root_matches {
        journal_boundary_violation(journal, expected_destination_root)
            .or_else(|| operation_receipt_violation(journal, journal_path, backup_root))
    } else {
        None
    };
    let mut entries = Vec::new();
    for (index, entry) in journal
        .entries
        .iter()
        .take(SHARED_MAX_ROLLBACK_ENTRIES)
        .enumerate()
    {
        if let Some(reason) = violation {
            entries.push(SharedRollbackEntry {
                destination: None,
                backup: None,
                expected_installed_digest: entry.final_destination_digest.clone(),
                observed_destination_digest: None,
                observed_backup_digest: None,
                outcome: SharedRollbackOutcome::JournalMalformed,
                failure: Some(failure(
                    SharedApplyFailureKind::InvalidJournal,
                    None,
                    reason,
                )),
                permissions: None,
            });
            continue;
        }
        entries.push(rollback_entry_preview(
            entry,
            expected_destination_root,
            backup_root,
            &journal.operation_id,
            root_matches,
            journal.rollback_operation_id.is_some() || rollback_marker_exists,
            recorded_rolled_back.get(index).copied().unwrap_or(false),
        ));
    }
    let available = entries
        .iter()
        .any(|entry| entry.outcome == SharedRollbackOutcome::Available)
        && entries.iter().all(|entry| {
            matches!(
                entry.outcome,
                SharedRollbackOutcome::Available | SharedRollbackOutcome::NoChangeRequired
            )
        });
    let mut preview = SharedRollbackPreview {
        schema_version: SHARED_APPLY_SCHEMA_VERSION,
        preview_id: String::new(),
        journal_path: SharedTransactionPath::from_path(journal_path),
        original_operation_id: journal.operation_id.clone(),
        destination_root: SharedTransactionPath::from_path(expected_destination_root),
        entries,
        available,
    };
    preview.preview_id = rollback_preview_digest(&preview);
    preview
}

/// Records that an entry Undo refused to act on needs review. Its pending
/// record otherwise goes on saying the entry is being undone, which a later
/// preview takes as proof that a file now missing was removed by that Undo.
fn mark_needs_review(durable: &mut SharedDurableJournal, index: usize) {
    if let Some(entry) = durable.entries.get_mut(index) {
        entry.state = SharedEntryRecoveryState::NeedsReview;
    }
}

pub fn execute_shared_rollback(
    preview: &SharedRollbackPreview,
    options: &SharedRollbackOptions,
) -> SharedRollbackResult {
    if !options.confirmation.approved
        || options.confirmation.preview_id != preview.preview_id
        || rollback_preview_digest(preview) != preview.preview_id
        || !preview.available
    {
        return SharedRollbackResult {
            preview: preview.clone(),
            journal_path: None,
            status: SharedApplyStatus::Failed,
        };
    }
    let journal_path = match preview.journal_path.to_path_buf() {
        Ok(value) => value,
        Err(_) => {
            return SharedRollbackResult {
                preview: preview.clone(),
                journal_path: None,
                status: SharedApplyStatus::Failed,
            };
        }
    };
    let root = preview.destination_root.to_path_buf().unwrap_or_default();
    let fresh = preview_shared_rollback(&journal_path, &root, &options.backup_root);
    if fresh.preview_id != preview.preview_id || !fresh.available {
        return SharedRollbackResult {
            preview: fresh,
            journal_path: None,
            status: SharedApplyStatus::Failed,
        };
    }
    let Ok(_lock) = RootLock::acquire(&root, LOCK_TIMEOUT) else {
        return SharedRollbackResult {
            preview: fresh,
            journal_path: None,
            status: SharedApplyStatus::Failed,
        };
    };
    // The journal is read once more, under the lock, and it is this copy
    // that is both checked and acted on. A journal rewritten since the
    // preview above either gives the same preview or stops here.
    let Ok(original) = read_journal(&journal_path) else {
        return SharedRollbackResult {
            preview: fresh,
            journal_path: None,
            status: SharedApplyStatus::Failed,
        };
    };
    let fresh = rollback_preview_of(&original, &journal_path, &root, &options.backup_root);
    if fresh.preview_id != preview.preview_id || !fresh.available {
        return SharedRollbackResult {
            preview: fresh,
            journal_path: None,
            status: SharedApplyStatus::Failed,
        };
    }
    let mut durable = durable_journal_from_completed(
        &original,
        &options.rollback_operation_id,
        &options.backup_root,
    );
    if write_pending_journal(&durable, &options.history_root).is_err() {
        return SharedRollbackResult {
            preview: fresh,
            journal_path: None,
            status: SharedApplyStatus::Failed,
        };
    }
    let mut applied = fresh.clone();
    // Restores first: removing the files a change created while the file they
    // belong with cannot be put back would leave that file pointing at
    // missing files. The first restore that does not complete stops Undo, so
    // nothing else is removed or changed.
    let mut order: Vec<usize> = (0..original.entries.len().min(applied.entries.len())).collect();
    order.sort_by_key(|&index| {
        original.entries[index].outcome != SharedApplyOutcome::ReplacedExisting
    });
    let mut processed: Vec<usize> = Vec::new();
    let mut halted_by: Option<usize> = None;
    for index in order {
        if halted_by.is_none() {
            halted_by = processed.iter().copied().find(|&done| {
                original.entries[done].outcome == SharedApplyOutcome::ReplacedExisting
                    && !matches!(
                        applied.entries[done].outcome,
                        SharedRollbackOutcome::RestoredBackup
                            | SharedRollbackOutcome::NoChangeRequired
                    )
            });
        }
        if halted_by.is_some() {
            if let Some(entry) = durable.entries.get_mut(index) {
                entry.state = SharedEntryRecoveryState::NeedsReview;
            }
            let kept = &mut applied.entries[index];
            kept.outcome = SharedRollbackOutcome::Retained;
            kept.failure = Some(SharedApplyFailure {
                kind: SharedApplyFailureKind::RollbackBlocked,
                path: kept.destination.clone(),
                detail: "Kept as it is: an earlier file could not be restored, so EmuWiz stopped before removing or changing this one.".into(),
            });
            continue;
        }
        processed.push(index);
        // The fresh preview found nothing left to do for this file: a
        // replaced file already back to its backup, or a new file an
        // earlier, partly finished Undo already removed. Never touch it.
        if applied.entries[index].outcome == SharedRollbackOutcome::NoChangeRequired
            && matches!(
                original.entries[index].outcome,
                SharedApplyOutcome::ReplacedExisting | SharedApplyOutcome::InstalledNew
            )
        {
            if let Some(entry) = durable.entries.get_mut(index) {
                entry.state = SharedEntryRecoveryState::RolledBack;
            }
            continue;
        }
        let (rollback, install) = (&mut applied.entries[index], &original.entries[index]);
        durable.transaction_state = SharedRecoveryState::RollingBack;
        if let Some(entry) = durable.entries.get_mut(index) {
            entry.state = SharedEntryRecoveryState::RollingBack;
        }
        if should_inject(FaultPoint::RollbackCheckpoint) {
            return SharedRollbackResult {
                preview: applied,
                journal_path: None,
                status: SharedApplyStatus::PartialFailure,
            };
        }
        if write_pending_journal(&durable, &options.history_root).is_err() {
            return SharedRollbackResult {
                preview: applied,
                journal_path: None,
                status: SharedApplyStatus::PartialFailure,
            };
        }
        // Where to act is the folder this Undo was opened for, joined with
        // a plain relative path. The root an entry names is never used: it
        // is only ever compared with that folder.
        let relative = install
            .plan_entry
            .destination_relative_path
            .to_path_buf()
            .ok()
            .filter(|relative| is_plain_relative(relative));
        let Some(relative) = relative else {
            rollback.outcome = SharedRollbackOutcome::Failed;
            mark_needs_review(&mut durable, index);
            continue;
        };
        let destination = root.join(&relative);
        let existed_before = install.destination_existed_before_apply.unwrap_or(matches!(
            install.outcome,
            SharedApplyOutcome::ReplacedExisting
        ));
        match (install.outcome, existed_before) {
            (SharedApplyOutcome::InstalledNew, false) => {
                if !install_receipt_matches(
                    &options.backup_root,
                    &original.operation_id,
                    &root,
                    &relative,
                    install.final_destination_digest.as_deref(),
                ) {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    rollback.failure = Some(failure(
                        SharedApplyFailureKind::BackupMissing,
                        Some(&destination),
                        "EmuWiz has no record of its own that it installed this file; it was \
                         kept as it is",
                    ));
                    mark_needs_review(&mut durable, index);
                    continue;
                }
                match remove_and_verify_new_destination(
                    &destination,
                    install
                        .final_destination_digest
                        .as_deref()
                        .unwrap_or_default(),
                ) {
                    Ok(()) => {
                        rollback.outcome = SharedRollbackOutcome::RemovedInstalledFile;
                        withdraw_receipt(
                            INSTALL_RECEIPT_SUFFIX,
                            &root,
                            &relative,
                            &options.backup_root,
                            &original.operation_id,
                        );
                        cleanup_created_directories(install, &root);
                    }
                    Err((kind, kept)) => {
                        let detail = match (kind, kept) {
                            (SharedApplyFailureKind::DestinationChanged, _) => format!(
                                "The file at {} no longer holds what EmuWiz installed, so it was kept as it is and not deleted.",
                                destination.display()
                            ),
                            (SharedApplyFailureKind::DisplacedFileKept, Some(kept)) => format!(
                                "The file at {} no longer holds what EmuWiz installed, and something else now occupies that name, so the changed file was kept at {} rather than deleted. Review both by hand.",
                                destination.display(),
                                kept.display()
                            ),
                            (SharedApplyFailureKind::SourceMissing, _) => format!(
                                "The file at {} is already gone.",
                                destination.display()
                            ),
                            _ => "newly installed destination could not be removed and verified absent".into(),
                        };
                        rollback.outcome = SharedRollbackOutcome::Failed;
                        rollback.failure = Some(failure(kind, Some(&destination), &detail));
                    }
                }
            }
            (SharedApplyOutcome::ReplacedExisting, true) => {
                let Some(backup) = install.backup_path.as_ref() else {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    mark_needs_review(&mut durable, index);
                    continue;
                };
                let Ok(backup) = backup.to_path_buf() else {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    mark_needs_review(&mut durable, index);
                    continue;
                };
                // The backup is read only from inside the backup folder the
                // caller named, and neither it nor the file it restores may
                // be reached through a link below those folders.
                let backup_is_ours = operation_evidence_path(
                    &options.backup_root,
                    &original.operation_id,
                    &destination,
                    BACKUP_SUFFIX,
                )
                .is_ok_and(|expected| expected == backup)
                    && strictly_inside(&backup, &options.backup_root).is_some_and(|below| {
                        reject_links_below(&options.backup_root, below).is_ok()
                    });
                let destination_is_plain = destination
                    .strip_prefix(&root)
                    .is_ok_and(|below| reject_links_below(&root, below).is_ok());
                if !backup_is_ours || !destination_is_plain {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    rollback.failure = Some(failure(
                        SharedApplyFailureKind::InvalidJournal,
                        Some(&destination),
                        "the backup is not the one EmuWiz keeps for this file, or a link \
                         stands between a folder this Undo was opened for and the file",
                    ));
                    mark_needs_review(&mut durable, index);
                    continue;
                }
                if let Err(reason) = replacement_receipt_agrees(
                    &options.backup_root,
                    &original.operation_id,
                    &root,
                    &relative,
                    install.final_destination_digest.as_deref(),
                ) {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    rollback.failure = Some(failure(
                        SharedApplyFailureKind::InvalidJournal,
                        Some(&destination),
                        reason,
                    ));
                    mark_needs_review(&mut durable, index);
                    continue;
                }
                let expected = install.backup_digest.as_deref().unwrap_or_default();
                if should_inject(FaultPoint::RollbackRestore)
                    || should_inject_after(FaultPoint::RollbackRestore)
                {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    mark_needs_review(&mut durable, index);
                    continue;
                }
                // The confirmed preview fixed the permissions; fall back to the
                // journal only if a preview somehow lacks them.
                let mode = rollback
                    .permissions
                    .unwrap_or_else(|| {
                        SharedRollbackPermissions::for_recorded_mode(install.destination_pre_mode)
                    })
                    .restored_mode();
                // A refusal falls through to the durable NeedsReview update.
                match pinned_installed_file(
                    &destination,
                    install
                        .final_destination_digest
                        .as_deref()
                        .unwrap_or_default(),
                ) {
                    Err(kind) => {
                        rollback.outcome = SharedRollbackOutcome::Failed;
                        rollback.failure = Some(failure(
                            kind,
                            Some(&destination),
                            "the installed file changed before it could be restored",
                        ));
                    }
                    Ok(installed) => {
                        match atomic_write(&backup, &destination, expected, false, mode, installed)
                        {
                            Ok(_) => {
                                rollback.outcome = SharedRollbackOutcome::RestoredBackup;
                                withdraw_receipt(
                                    REPLACEMENT_RECEIPT_SUFFIX,
                                    &root,
                                    &relative,
                                    &options.backup_root,
                                    &original.operation_id,
                                );
                            }
                            Err((kind, temp)) => {
                                let detail = match kind {
                                    // Nothing was undone: say so, and how to
                                    // recover by hand from the kept backup.
                                    SharedApplyFailureKind::AtomicReplaceUnsupported => format!(
                                        "This folder's filesystem cannot replace files atomically, so EmuWiz did not restore this file. The original file is kept in EmuWiz's backup at {}. To restore it by hand, close the emulator and copy that file over {}. This change stays in History.",
                                        backup.display(),
                                        destination.display()
                                    ),
                                    SharedApplyFailureKind::DisplacedFileKept
                                    | SharedApplyFailureKind::DestinationChanged => {
                                        publication_failure(kind, temp.as_deref()).1
                                    }
                                    SharedApplyFailureKind::PublishedUnverified => format!(
                                        "EmuWiz restored this file, but it changed straight afterwards, so the result could not be confirmed. Review {} before retrying; the original is still kept in EmuWiz's backup at {}.",
                                        destination.display(),
                                        backup.display()
                                    ),
                                    _ => "previous destination bytes could not be restored and verified".into(),
                                };
                                rollback.outcome = SharedRollbackOutcome::Failed;
                                rollback.failure = Some(failure(kind, Some(&destination), &detail));
                            }
                        }
                    }
                }
            }
            (SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting, _) => {
                rollback.outcome = SharedRollbackOutcome::Failed;
                rollback.failure = Some(failure(
                    SharedApplyFailureKind::InvalidJournal,
                    Some(&destination),
                    "journal outcome contradicts explicit pre-apply destination existence",
                ));
            }
            _ => rollback.outcome = SharedRollbackOutcome::NoChangeRequired,
        }
        if let Some(entry) = durable.entries.get_mut(index) {
            entry.state = match rollback.outcome {
                SharedRollbackOutcome::RemovedInstalledFile
                | SharedRollbackOutcome::RestoredBackup
                | SharedRollbackOutcome::NoChangeRequired => SharedEntryRecoveryState::RolledBack,
                SharedRollbackOutcome::Failed => SharedEntryRecoveryState::NeedsReview,
                _ => SharedEntryRecoveryState::RollingBack,
            };
        }
        if should_inject(FaultPoint::RollbackAfterMutation) {
            durable.transaction_state = SharedRecoveryState::RollingBack;
            return SharedRollbackResult {
                preview: applied,
                journal_path: None,
                status: SharedApplyStatus::PartialFailure,
            };
        }
        if write_pending_journal(&durable, &options.history_root).is_err() {
            durable.transaction_state = SharedRecoveryState::RollbackFailed;
            let _ = write_pending_journal(&durable, &options.history_root);
            return SharedRollbackResult {
                preview: applied,
                journal_path: None,
                status: SharedApplyStatus::PartialFailure,
            };
        }
    }
    // Every per-entry installed file/child directory above has already
    // been removed (or left in place on failure) - only now can a
    // transaction-created root directory possibly be empty, so this always
    // runs last, and is a no-op for a journal that never bootstrapped a
    // root at all (including every journal written before that field
    // existed).
    cleanup_transaction_created_root_directories(&original, &root);
    // A late restore failure (the last entry processed) also halts nothing
    // further but still decides the summary below.
    let halted_by = halted_by.or_else(|| {
        processed.iter().copied().find(|&done| {
            original.entries[done].outcome == SharedApplyOutcome::ReplacedExisting
                && !matches!(
                    applied.entries[done].outcome,
                    SharedRollbackOutcome::RestoredBackup | SharedRollbackOutcome::NoChangeRequired
                )
        })
    });
    // A restore that published but kept a changed displaced file did change
    // the destination, even though it is reported as a failure.
    let changed = applied.entries.iter().any(|entry| {
        matches!(
            entry.outcome,
            SharedRollbackOutcome::RestoredBackup | SharedRollbackOutcome::RemovedInstalledFile
        ) || entry.failure.as_ref().is_some_and(|failure| {
            matches!(
                failure.kind,
                SharedApplyFailureKind::DisplacedFileKept
                    | SharedApplyFailureKind::PublishedUnverified
            )
        })
    });
    // Say exactly what this Undo changed; never claim nothing changed when
    // something did.
    if let Some(index) = halted_by
        && let Some(entry) = applied.entries.get_mut(index)
    {
        let summary = if changed {
            "Files EmuWiz had already restored stay restored; every other file was kept as it is."
        } else {
            "Nothing was changed: every file was kept as it is."
        };
        match entry.failure.as_mut() {
            Some(failure) => {
                failure.detail = format!("{} {summary}", failure.detail);
            }
            None => {
                entry.failure = Some(SharedApplyFailure {
                    kind: SharedApplyFailureKind::RollbackBlocked,
                    path: entry.destination.clone(),
                    detail: format!("This file could not be restored. {summary}"),
                });
            }
        }
    }
    let success = applied.entries.iter().all(|entry| {
        matches!(
            entry.outcome,
            SharedRollbackOutcome::RemovedInstalledFile
                | SharedRollbackOutcome::RestoredBackup
                | SharedRollbackOutcome::NoChangeRequired
        )
    });
    let marker = options.history_root.join(format!(
        "{}.rollback.json",
        safe_identifier(&preview.original_operation_id).unwrap_or_else(|_| "invalid".into())
    ));
    let journal_path = success
        .then(|| serde_json::to_vec_pretty(&applied).ok())
        .flatten()
        .and_then(|bytes| {
            if bytes.len() as u64 > SHARED_MAX_JOURNAL_BYTES {
                return None;
            }
            atomic_managed_write(&marker, &bytes).ok().map(|_| marker)
        });
    let marker_written = journal_path.is_some();
    durable.transaction_state = if success && marker_written {
        SharedRecoveryState::RolledBack
    } else {
        SharedRecoveryState::RollbackFailed
    };
    if success && marker_written {
        let _ = write_pending_journal(&durable, &options.history_root);
        if let Ok(path) = pending_journal_path(&options.history_root, &original.operation_id) {
            let _ = fs::remove_file(path);
        }
    } else {
        let _ = write_pending_journal(&durable, &options.history_root);
    }
    SharedRollbackResult {
        preview: applied,
        journal_path,
        status: if success && marker_written {
            SharedApplyStatus::Success
        } else if success || changed {
            SharedApplyStatus::PartialFailure
        } else {
            SharedApplyStatus::Failed
        },
    }
}

#[derive(Debug)]
struct StableHash {
    digest: String,
    bytes: u64,
    /// Identity of the descriptor that was hashed (Unix only).
    pin: Option<FilePin>,
}

/// What a replacement or restore may displace: the identity and permission
/// bits of the exact file that was hashed (and, on Apply, whose mode was
/// recorded). Publication refuses unless the file it displaces still matches
/// all of it. ctime is deliberately absent: a rename updates it, while mode,
/// owner and link count are compared directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FilePin {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
    /// `st_mode & 0o7777`.
    mode: u32,
    uid: u32,
    gid: u32,
    nlink: u64,
}

impl FilePin {
    #[cfg(unix)]
    fn of(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
            mode: metadata.mode() & 0o7777,
            uid: metadata.uid(),
            gid: metadata.gid(),
            nlink: metadata.nlink(),
        }
    }
}

/// The file a replacement or restore must displace: its pin and the digest of
/// the bytes it held when observed (the backed-up or installed bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Displaced {
    pin: FilePin,
    digest: String,
}

fn stable_hash(path: &Path, max: u64) -> Result<StableHash, SharedApplyFailureKind> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(SharedApplyFailureKind::InvalidPlan);
    }
    reject_symlink_components(path)?;
    let before = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SharedApplyFailureKind::SourceMissing
        } else {
            SharedApplyFailureKind::SourceChanged
        }
    })?;
    if before.file_type().is_symlink() {
        return Err(SharedApplyFailureKind::SourceSymlink);
    }
    if !before.is_file() {
        return Err(SharedApplyFailureKind::SourceSpecialFile);
    }
    if before.len() > max {
        return Err(SharedApplyFailureKind::ResourceLimitReached);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(path)
        .map_err(|_| SharedApplyFailureKind::SourceChanged)?;
    let opened = file
        .metadata()
        .map_err(|_| SharedApplyFailureKind::SourceChanged)?;
    if !same_file(&before, &opened) {
        return Err(SharedApplyFailureKind::SourceChanged);
    }
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| SharedApplyFailureKind::SourceChanged)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > max {
            return Err(SharedApplyFailureKind::ResourceLimitReached);
        }
        digest.update(&buffer[..read]);
    }
    let after = fs::symlink_metadata(path).map_err(|_| SharedApplyFailureKind::SourceChanged)?;
    if !same_file(&before, &after) {
        return Err(SharedApplyFailureKind::SourceChanged);
    }
    #[cfg(unix)]
    let pin = Some(FilePin::of(&opened));
    #[cfg(not(unix))]
    let pin = None;
    Ok(StableHash {
        digest: hex_bytes(&digest.finalize()),
        bytes,
        pin,
    })
}

fn create_backup(
    destination: &Path,
    expected: &str,
    backup_root: &Path,
    operation_id: &str,
) -> Result<PathBuf, SharedApplyFailureKind> {
    if should_inject(FaultPoint::BackupWrite) {
        return Err(SharedApplyFailureKind::BackupFailed);
    }
    // A later file in the same transaction reuses the operation's folder.
    // Each file in it is created without overwriting, so a name collision
    // is refused.
    operation_evidence_folder(backup_root, operation_id)?;
    let final_path =
        operation_evidence_path(backup_root, operation_id, destination, BACKUP_SUFFIX)?;
    let bytes = read_bounded(destination, SHARED_MAX_SOURCE_BYTES)?;
    if digest_bytes(&bytes) != expected {
        return Err(SharedApplyFailureKind::DestinationChanged);
    }
    atomic_managed_write(&final_path, &bytes)?;
    let verified = stable_hash(&final_path, SHARED_MAX_SOURCE_BYTES)?;
    if verified.digest != expected {
        return Err(SharedApplyFailureKind::BackupFailed);
    }
    Ok(final_path)
}

/// `displaces`: for a replacement or restore, the pinned file it must
/// displace (see [`publish_displacing`]); `None` publishes as before.
fn atomic_write(
    source: &Path,
    destination: &Path,
    expected: &str,
    no_replace: bool,
    mode: u32,
    displaces: Option<Displaced>,
) -> Result<PathBuf, (SharedApplyFailureKind, Option<PathBuf>)> {
    if should_inject(FaultPoint::TemporaryWrite) {
        return Err((SharedApplyFailureKind::WriteFailed, None));
    }
    let bytes = read_bounded(source, SHARED_MAX_SOURCE_BYTES).map_err(|kind| (kind, None))?;
    if digest_bytes(&bytes) != expected {
        return Err((SharedApplyFailureKind::SourceChanged, None));
    }
    atomic_write_bytes(&bytes, destination, expected, no_replace, mode, displaces)
}

fn atomic_write_bytes(
    bytes: &[u8],
    destination: &Path,
    expected: &str,
    no_replace: bool,
    mode: u32,
    displaces: Option<Displaced>,
) -> Result<PathBuf, (SharedApplyFailureKind, Option<PathBuf>)> {
    if bytes.len() as u64 > SHARED_MAX_MATERIALIZED_BYTES {
        return Err((SharedApplyFailureKind::ResourceLimitReached, None));
    }
    let parent = destination
        .parent()
        .ok_or((SharedApplyFailureKind::DestinationUnsafe, None))?;
    let temp = parent.join(format!(
        ".archivefs-apply-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    // `keep_temp`: the temporary name holds a file that is not ours (a
    // displaced file that could not be put back) and must never be deleted.
    let mut keep_temp = false;
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        // Owner-only from creation, so the bytes are never briefly readable
        // under the process umask before the final mode is set.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(OWNER_ONLY_MODE);
        }
        let mut file = options
            .open(&temp)
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        file.write_all(bytes)
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        if should_inject(FaultPoint::Flush) {
            return Err(SharedApplyFailureKind::WriteFailed);
        }
        // Set through the open descriptor of the new temporary file, before
        // it is synced and renamed into place: never via a path that could be
        // swapped, and durable together with the bytes.
        if should_inject(FaultPoint::Permissions) {
            return Err(SharedApplyFailureKind::WriteFailed);
        }
        set_permissions(&file, mode).map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        file.sync_all()
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        let temp_hash = stable_hash(&temp, SHARED_MAX_MATERIALIZED_BYTES)?;
        if should_inject(FaultPoint::Verification) || temp_hash.digest != expected {
            return Err(SharedApplyFailureKind::VerificationFailed);
        }
        if should_inject(FaultPoint::Rename) {
            return Err(SharedApplyFailureKind::WriteFailed);
        }
        match &displaces {
            Some(displaced) => {
                publish_displacing(&temp, destination, displaced, temp_hash.pin, &mut keep_temp)?
            }
            None if no_replace => rename_no_replace(&temp, destination)
                .map_err(|_| SharedApplyFailureKind::DestinationChanged)?,
            // A replacement or restore without a pinned file to displace (no
            // file identity on this platform) is never published unchecked.
            None => return Err(SharedApplyFailureKind::AtomicReplaceUnsupported),
        }
        sync_directory(parent);
        after_publish(destination);
        // From here on the destination has changed: a failed check is
        // reported as such, never as "nothing written".
        match stable_hash(destination, SHARED_MAX_MATERIALIZED_BYTES) {
            Ok(final_hash) if final_hash.digest == expected => Ok(()),
            _ => Err(SharedApplyFailureKind::PublishedUnverified),
        }
    })();
    if let Err(kind) = write_result {
        if !keep_temp {
            let _ = fs::remove_file(&temp);
        }
        return Err((kind, Some(temp)));
    }
    Ok(temp)
}

#[cfg(test)]
pub(super) type PublishHook = Box<dyn FnMut(&Path)>;

#[cfg(test)]
thread_local! {
    /// Runs between observation and publication, to force a race.
    static BEFORE_PUBLISH: std::cell::RefCell<Option<PublishHook>> =
        std::cell::RefCell::new(None);
    /// Runs after a refused exchange, before it is reversed.
    static BEFORE_REVERSE: std::cell::RefCell<Option<PublishHook>> =
        std::cell::RefCell::new(None);
    /// Runs right after publication, before the final check.
    static AFTER_PUBLISH: std::cell::RefCell<Option<PublishHook>> =
        std::cell::RefCell::new(None);
    /// Runs right before Undo moves an installed file out of its name.
    static BEFORE_REMOVE: std::cell::RefCell<Option<PublishHook>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(super) fn set_after_publish(hook: Option<PublishHook>) {
    AFTER_PUBLISH.with(|slot| *slot.borrow_mut() = hook);
}

fn after_publish(_destination: &Path) {
    #[cfg(test)]
    AFTER_PUBLISH.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(_destination);
        }
    });
}

#[cfg(test)]
fn set_before_publish(hook: Option<PublishHook>) {
    BEFORE_PUBLISH.with(|slot| *slot.borrow_mut() = hook);
}

#[cfg(test)]
fn set_before_remove(hook: Option<PublishHook>) {
    BEFORE_REMOVE.with(|slot| *slot.borrow_mut() = hook);
}

/// Runs between Undo's last observation of an installed file and the moment
/// that file is moved out of its name, to force a race.
fn before_remove(_destination: &Path) {
    #[cfg(test)]
    BEFORE_REMOVE.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(_destination);
        }
    });
}

#[cfg(test)]
fn set_before_reverse(hook: Option<PublishHook>) {
    BEFORE_REVERSE.with(|slot| *slot.borrow_mut() = hook);
}

fn before_publish(_destination: &Path) {
    #[cfg(test)]
    BEFORE_PUBLISH.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(_destination);
        }
    });
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn before_reverse(_destination: &Path) {
    #[cfg(test)]
    BEFORE_REVERSE.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(_destination);
        }
    });
}

/// Publishes `temp` at `destination` only if what it displaces is still the
/// exact observed file: same inode, size, mtime, permission bits, owner and
/// link count, and the same bytes. On Linux the two names are exchanged
/// atomically and the displaced file is then hashed through an `O_NOFOLLOW`
/// descriptor, so no change between observation and publication can slip
/// through; on a mismatch the exchange is reversed, putting the other file
/// back untouched, and the write is refused. A filesystem without atomic
/// exchange refuses rather than falling back to an unchecked rename. A
/// verified displaced file is removed (its bytes are in the transaction's
/// backup or were EmuWiz's own install).
/// `ours`: the pin of our own temporary file, so after a reversed exchange
/// only our file is ever left for deletion.
#[cfg(target_os = "linux")]
fn publish_displacing(
    temp: &Path,
    destination: &Path,
    displaced: &Displaced,
    ours: Option<FilePin>,
    keep_temp: &mut bool,
) -> Result<(), SharedApplyFailureKind> {
    before_publish(destination);
    let exchanged = if should_inject(FaultPoint::ExchangeUnsupported)
        || should_inject_after(FaultPoint::ExchangeUnsupported)
    {
        Err(std::io::Error::from_raw_os_error(libc::EINVAL))
    } else {
        rename_exchange(temp, destination)
    };
    if let Err(error) = exchanged {
        return Err(
            if matches!(
                error.raw_os_error(),
                Some(libc::EINVAL | libc::EOPNOTSUPP | libc::ENOSYS)
            ) {
                SharedApplyFailureKind::AtomicReplaceUnsupported
            } else {
                SharedApplyFailureKind::DestinationChanged
            },
        );
    }
    // `temp` now names the displaced file and `destination` our new one.
    let observed = stable_hash(
        temp,
        SHARED_MAX_SOURCE_BYTES.max(SHARED_MAX_MATERIALIZED_BYTES),
    );
    if observed.is_ok_and(|hash| hash.pin == Some(displaced.pin) && hash.digest == displaced.digest)
    {
        // Ours is published; a leftover displaced copy is only litter.
        let _ = fs::remove_file(temp);
        return Ok(());
    }
    before_reverse(destination);
    if should_inject(FaultPoint::ReverseExchange) || rename_exchange(temp, destination).is_err() {
        // Ours stays published; the other file is kept under `temp`.
        *keep_temp = true;
        return Err(SharedApplyFailureKind::DisplacedFileKept);
    }
    // Something else may have been renamed onto the destination meanwhile and
    // come back under `temp`: delete it only if it is still our own file.
    let returned = stable_hash(
        temp,
        SHARED_MAX_SOURCE_BYTES.max(SHARED_MAX_MATERIALIZED_BYTES),
    )
    .ok()
    .and_then(|hash| hash.pin);
    if ours.is_none() || returned != ours {
        *keep_temp = true;
        return Err(SharedApplyFailureKind::DisplacedFileKept);
    }
    Err(SharedApplyFailureKind::DestinationChanged)
}

/// Only Linux offers the atomic exchange the guarantee above needs. Elsewhere
/// a replacement or restore is refused outright: there is deliberately no
/// re-check-then-rename fallback.
#[cfg(not(target_os = "linux"))]
fn publish_displacing(
    _temp: &Path,
    destination: &Path,
    _displaced: &Displaced,
    _ours: Option<FilePin>,
    _keep_temp: &mut bool,
) -> Result<(), SharedApplyFailureKind> {
    before_publish(destination);
    Err(SharedApplyFailureKind::AtomicReplaceUnsupported)
}

#[cfg(target_os = "linux")]
fn rename_exchange(left: &Path, right: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let left = std::ffi::CString::new(left.as_os_str().as_bytes())?;
    let right = std::ffi::CString::new(right.as_os_str().as_bytes())?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn atomic_managed_write(path: &Path, bytes: &[u8]) -> Result<(), SharedApplyFailureKind> {
    let parent = path
        .parent()
        .ok_or(SharedApplyFailureKind::ManagedRootUnsafe)?;
    prepare_managed_root(parent)?;
    if fs::symlink_metadata(path).is_ok() {
        return Err(SharedApplyFailureKind::DuplicateOperationId);
    }
    let temp = parent.join(format!(
        ".archivefs-managed-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
    file.write_all(bytes)
        .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
    file.sync_all()
        .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
    if let Err(error) = rename_no_replace(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
            SharedApplyFailureKind::DuplicateOperationId
        } else {
            SharedApplyFailureKind::WriteFailed
        });
    }
    sync_directory(parent);
    Ok(())
}

fn atomic_managed_replace(path: &Path, bytes: &[u8]) -> Result<(), SharedApplyFailureKind> {
    let parent = path
        .parent()
        .ok_or(SharedApplyFailureKind::ManagedRootUnsafe)?;
    prepare_managed_root(parent)?;
    let temp = parent.join(format!(
        ".archivefs-managed-replace-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        file.write_all(bytes)
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        file.sync_all()
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        fs::rename(&temp, path).map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        sync_directory(parent);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn pending_journal_path(
    history_root: &Path,
    operation_id: &str,
) -> Result<PathBuf, SharedApplyFailureKind> {
    Ok(history_root.join(format!("{}.pending.json", safe_identifier(operation_id)?)))
}

fn planned_backup_path(
    backup_root: &Path,
    operation_id: &str,
    entry: &SharedPlanEntry,
) -> Option<SharedTransactionPath> {
    if !entry.backup_required {
        return None;
    }
    let relative = entry.destination_relative_path.to_path_buf().ok()?;
    let destination = entry.destination_root.to_path_buf().ok()?.join(relative);
    let operation = safe_identifier(operation_id).ok()?;
    Some(SharedTransactionPath::from_path(
        &backup_root.join(operation).join(format!(
            "{}.bak",
            digest_text(&destination.to_string_lossy())
        )),
    ))
}

fn durable_journal_from_plan(
    plan: &SharedTransactionPlan,
    options: &SharedApplyOptions,
) -> SharedDurableJournal {
    SharedDurableJournal {
        schema_version: SHARED_DURABLE_JOURNAL_SCHEMA_VERSION,
        transaction_state: SharedRecoveryState::Planned,
        operation_id: options.operation_id.clone(),
        operation_type: "shared_mod_apply".into(),
        plan_id: plan.plan_id.clone(),
        timestamp_unix_seconds: options.timestamp_unix_seconds,
        context: plan.context.clone(),
        approved_source_root: plan.approved_source_root.clone(),
        destination_root: plan.destination_root.clone(),
        backup_root: SharedTransactionPath::from_path(&options.backup_root),
        entries: plan
            .entries
            .iter()
            .map(|entry| SharedDurableEntry {
                plan_entry: entry.clone(),
                state: SharedEntryRecoveryState::Planned,
                observed_destination_digest: entry.destination_pre_digest.clone(),
                backup_path: planned_backup_path(
                    &options.backup_root,
                    &options.operation_id,
                    entry,
                ),
                backup_digest: entry.destination_pre_digest.clone(),
                destination_pre_mode: None,
                resulting_destination_digest: None,
                temporary_path: None,
                destination_existed_before_apply: Some(matches!(
                    entry.destination_pre_state,
                    PreviewDestinationState::RegularFileIdentical
                        | PreviewDestinationState::RegularFileDifferent
                )),
            })
            .collect(),
        created_root_directories: Vec::new(),
        rollback_operation_id: None,
        rollback_of_operation_id: None,
    }
}

fn durable_journal_from_completed(
    journal: &SharedApplyJournal,
    rollback_operation_id: &str,
    backup_root: &Path,
) -> SharedDurableJournal {
    SharedDurableJournal {
        schema_version: SHARED_DURABLE_JOURNAL_SCHEMA_VERSION,
        transaction_state: SharedRecoveryState::RollingBack,
        operation_id: journal.operation_id.clone(),
        operation_type: "shared_mod_rollback".into(),
        plan_id: journal.plan_id.clone(),
        timestamp_unix_seconds: journal.timestamp_unix_seconds,
        context: journal.context.clone(),
        approved_source_root: journal.approved_source_root.clone(),
        destination_root: journal.destination_root.clone(),
        backup_root: SharedTransactionPath::from_path(backup_root),
        entries: journal
            .entries
            .iter()
            .map(|entry| SharedDurableEntry {
                plan_entry: entry.plan_entry.clone(),
                state: SharedEntryRecoveryState::RollingBack,
                observed_destination_digest: entry.observed_destination_digest.clone(),
                backup_path: entry.backup_path.clone(),
                backup_digest: entry.backup_digest.clone(),
                destination_pre_mode: entry.destination_pre_mode,
                resulting_destination_digest: entry.final_destination_digest.clone(),
                temporary_path: entry.temporary_path.clone(),
                destination_existed_before_apply: entry.destination_existed_before_apply,
            })
            .collect(),
        created_root_directories: journal.created_root_directories.clone(),
        rollback_operation_id: Some(rollback_operation_id.to_owned()),
        rollback_of_operation_id: Some(journal.operation_id.clone()),
    }
}

fn update_durable_entry(
    durable: &mut SharedDurableJournal,
    index: usize,
    entry: &SharedApplyEntry,
) {
    let Some(target) = durable.entries.get_mut(index) else {
        return;
    };
    target.observed_destination_digest = entry.observed_destination_digest.clone();
    target.backup_path = entry.backup_path.clone();
    target.backup_digest = entry.backup_digest.clone();
    target.destination_pre_mode = entry.destination_pre_mode;
    target.resulting_destination_digest = entry.final_destination_digest.clone();
    target.temporary_path = entry.temporary_path.clone();
    target.destination_existed_before_apply = entry.destination_existed_before_apply;
    target.state = match entry.outcome {
        SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
            if entry.verification_succeeded =>
        {
            SharedEntryRecoveryState::Applied
        }
        SharedApplyOutcome::BackupFailed => SharedEntryRecoveryState::Applying,
        SharedApplyOutcome::WriteFailed | SharedApplyOutcome::VerificationFailed => {
            SharedEntryRecoveryState::NeedsReview
        }
        _ => SharedEntryRecoveryState::Applying,
    };
}

fn write_pending_journal(
    journal: &SharedDurableJournal,
    history_root: &Path,
) -> Result<(), SharedApplyFailure> {
    if should_inject(FaultPoint::DurableJournalWrite) {
        return Err(failure(
            SharedApplyFailureKind::JournalFailed,
            None,
            "injected durable journal write failure",
        ));
    }
    let bytes = serde_json::to_vec_pretty(journal).map_err(|error| {
        failure(
            SharedApplyFailureKind::JournalFailed,
            None,
            &error.to_string(),
        )
    })?;
    if bytes.len() as u64 > SHARED_MAX_JOURNAL_BYTES {
        return Err(failure(
            SharedApplyFailureKind::ResourceLimitReached,
            None,
            "durable journal size limit reached",
        ));
    }
    let path = pending_journal_path(history_root, &journal.operation_id)
        .map_err(|kind| failure(kind, None, "operation ID is not safe for a pending journal"))?;
    atomic_managed_replace(&path, &bytes)
        .map_err(|kind| failure(kind, Some(&path), "pending journal could not be written"))
}

pub(super) fn read_pending_journal(
    path: &Path,
) -> Result<SharedDurableJournal, SharedApplyFailure> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        failure(
            SharedApplyFailureKind::InvalidJournal,
            Some(path),
            &error.to_string(),
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > SHARED_MAX_JOURNAL_BYTES
    {
        return Err(failure(
            SharedApplyFailureKind::InvalidJournal,
            Some(path),
            "pending journal is not a bounded regular file",
        ));
    }
    let bytes = read_bounded(path, SHARED_MAX_JOURNAL_BYTES)
        .map_err(|kind| failure(kind, Some(path), "pending journal could not be read"))?;
    let journal: SharedDurableJournal = serde_json::from_slice(&bytes).map_err(|error| {
        failure(
            SharedApplyFailureKind::InvalidJournal,
            Some(path),
            &error.to_string(),
        )
    })?;
    if journal.schema_version != SHARED_DURABLE_JOURNAL_SCHEMA_VERSION {
        return Err(failure(
            SharedApplyFailureKind::UnsupportedJournal,
            Some(path),
            "durable journal schema version is unsupported",
        ));
    }
    Ok(journal)
}

/// Read-only startup discovery for durable shared-mod journals.  It performs
/// no cleanup, rollback, resume, locking, or filesystem mutation.
pub fn discover_pending_operations(history_root: &Path) -> Vec<SharedPendingOperation> {
    let Ok(entries) = fs::read_dir(history_root) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new("json")))
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".pending.json"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let journal = read_pending_journal(&path).ok()?;
            let reconciled_entries = journal
                .entries
                .iter()
                .map(|entry| reconcile_durable_entry(entry, journal.transaction_state))
                .collect();
            if matches!(
                journal.transaction_state,
                SharedRecoveryState::Applied | SharedRecoveryState::RolledBack
            ) {
                return None;
            }
            Some(SharedPendingOperation {
                journal_path: SharedTransactionPath::from_path(&path),
                journal,
                reconciled_entries,
            })
        })
        .collect()
}

/// Build an explicit, read-only recovery decision.  It never resumes or
/// rolls back; a caller must separately request an operation after checking
/// the displayed identities and exact transaction envelope.
pub fn plan_pending_recovery(operation: &SharedPendingOperation) -> SharedPendingRecoveryPlan {
    let safe_entries = operation.reconciled_entries.iter().all(|state| {
        matches!(
            state,
            SharedEntryRecoveryState::Applied | SharedEntryRecoveryState::Planned
        )
    });
    let rollback_state = matches!(
        operation.journal.transaction_state,
        SharedRecoveryState::Applying
            | SharedRecoveryState::ApplyFailed
            | SharedRecoveryState::NeedsReview
    );
    let can_safe_rollback = rollback_state && safe_entries;
    SharedPendingRecoveryPlan {
        operation_id: operation.journal.operation_id.clone(),
        state: operation.journal.transaction_state,
        can_safe_rollback,
        can_resume: false,
        reason: if can_safe_rollback {
            "all observed entries are either proven applied or proven unchanged; explicit rollback may be planned"
                .into()
        } else {
            "identity mismatch, missing backup, rollback in progress, or ambiguous filesystem state"
                .into()
        },
    }
}

/// Settles every apply that was interrupted before its terminal journal was
/// written (a crash, a kill, a power cut between the first write and the
/// receipt), so that what it changed is visible in History and can be undone.
///
/// For each `<operation>.pending.json` of a `shared_mod_apply` still in
/// `Planned`/`Applying`/`ApplyFailed`:
///
/// 1. The destination root's lock is tried without waiting. A root still
///    locked belongs to an apply that is running right now; it is left
///    alone (`StillRunning`).
/// 2. Every entry is reconciled against the destination exactly as
///    [`discover_pending_operations`] does: proven applied (destination holds
///    the planned bytes, and the backup still holds the replaced bytes),
///    proven untouched, or neither.
/// 3. A terminal journal is written once from that evidence. Proven-applied
///    entries become ordinary `InstalledNew`/`ReplacedExisting` entries with
///    a `JournalFailedAfterSuccessfulWrite` stage, so Undo works as for any
///    other journal. Untouched entries are recorded as not written. An entry
///    in any other state is recorded as `PublishedUnverified`, which blocks
///    Undo of the whole operation until someone has looked at the file: a
///    destination in an unexplained state is never silently deleted or
///    overwritten.
/// 4. The pending record is retired only after the terminal journal is
///    durable. A write failure keeps the pending record (`NeedsReview`).
///
/// Nothing under the destination root is ever modified here; the only
/// writes are the terminal journal and the pending record in `history_root`.
/// Per-entry created-directory lists are not recoverable from a pending
/// record, so Undo of a settled operation removes its files but may leave
/// an empty directory the apply created.
pub fn recover_interrupted_shared_applies(
    history_root: &Path,
    backup_root: &Path,
) -> Vec<SharedInterruptedApplyRecovery> {
    let mut recoveries = Vec::new();
    for pending in discover_pending_operations(history_root) {
        if pending.journal.operation_type != "shared_mod_apply"
            || !matches!(
                pending.journal.transaction_state,
                SharedRecoveryState::Planned
                    | SharedRecoveryState::Applying
                    | SharedRecoveryState::ApplyFailed
            )
        {
            continue;
        }
        let operation_id = pending.journal.operation_id.clone();
        let mut recovery = SharedInterruptedApplyRecovery {
            operation_id: operation_id.clone(),
            pending_journal_path: pending.journal_path.clone(),
            outcome: SharedInterruptedApplyOutcome::NeedsReview,
            entry_states: pending.reconciled_entries.clone(),
            journal_path: None,
            detail: String::new(),
        };
        let Ok(pending_path) = pending.journal_path.to_path_buf() else {
            recovery.detail = "pending journal path cannot be reconstructed".into();
            recoveries.push(recovery);
            continue;
        };
        let Ok(root) = pending.journal.destination_root.to_path_buf() else {
            recovery.detail = "destination root cannot be reconstructed".into();
            recoveries.push(recovery);
            continue;
        };
        let lock = match RootLock::acquire(&root, Duration::ZERO) {
            Ok(lock) => lock,
            Err(SharedApplyFailureKind::LockTimeout) => {
                recovery.outcome = SharedInterruptedApplyOutcome::StillRunning;
                recovery.detail = "destination root is locked by a running apply".into();
                recoveries.push(recovery);
                continue;
            }
            Err(kind) => {
                recovery.detail = format!("destination root is unavailable or unsafe ({kind:?})");
                recoveries.push(recovery);
                continue;
            }
        };
        let Ok(identifier) = safe_identifier(&operation_id) else {
            recovery.detail = "operation ID is not safe for a journal filename".into();
            recoveries.push(recovery);
            continue;
        };
        let terminal_path = history_root.join(format!("{identifier}.json"));
        if fs::symlink_metadata(&terminal_path).is_ok() {
            // The receipt exists; only its pending twin was left behind.
            let _ = fs::remove_file(&pending_path);
            recovery.outcome = SharedInterruptedApplyOutcome::AlreadyRecorded;
            recovery.journal_path = Some(SharedTransactionPath::from_path(&terminal_path));
            recovery.detail =
                "terminal journal already existed; stale pending record retired".into();
            recoveries.push(recovery);
            drop(lock);
            continue;
        }
        // Re-reconcile under the lock: the discovery pass ran unlocked.
        let states: Vec<SharedEntryRecoveryState> = pending
            .journal
            .entries
            .iter()
            .map(|entry| reconcile_durable_entry(entry, pending.journal.transaction_state))
            .collect();
        recovery.entry_states = states.clone();
        let journal = terminal_journal_from_interrupted(&pending.journal, &states, backup_root);
        match write_journal_once(&journal, history_root) {
            Ok(path) => {
                // The receipt is the durable record from here on; the
                // pending record is simply retired (never rewritten under
                // its canonical name, which could differ from a hand-renamed
                // discovered path and be "retired" again every launch).
                let _ = fs::remove_file(&pending_path);
                let applied = states
                    .iter()
                    .filter(|state| **state == SharedEntryRecoveryState::Applied)
                    .count();
                let unsettled = states
                    .iter()
                    .filter(|state| {
                        !matches!(
                            state,
                            SharedEntryRecoveryState::Applied | SharedEntryRecoveryState::Planned
                        )
                    })
                    .count();
                recovery.outcome = SharedInterruptedApplyOutcome::SettledForUndo;
                recovery.journal_path = Some(SharedTransactionPath::from_path(&path));
                let strays: Vec<String> = journal
                    .entries
                    .iter()
                    .filter(|entry| entry.outcome == SharedApplyOutcome::VerificationFailed)
                    .filter_map(|entry| {
                        entry.failures.iter().find_map(|failure| {
                            failure.path.as_ref().and_then(|path| {
                                stray_publication_file_path(Path::new(&path.display))
                            })
                        })
                    })
                    .map(|path| path.display().to_string())
                    .collect();
                recovery.detail = format!(
                    "interrupted install recorded: {applied} file(s) reached the destination, {} were never written{}{}",
                    states.len() - applied - unsettled,
                    if unsettled > 0 {
                        format!(", {unsettled} need(s) review before Undo")
                    } else {
                        String::new()
                    },
                    if strays.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " (temporary file(s) from the interrupted write still present: {})",
                            strays.join(", ")
                        )
                    }
                );
            }
            Err(error) => {
                recovery.detail = format!(
                    "terminal journal could not be written ({:?}: {}); pending record kept",
                    error.kind, error.detail
                );
            }
        }
        drop(lock);
        recoveries.push(recovery);
    }
    recoveries
}

/// The terminal journal an interrupted apply would have written, rebuilt
/// from its durable record plus what the destination proves now. Only
/// evidence is recorded: an entry is `InstalledNew`/`ReplacedExisting` when
/// the destination holds the planned bytes (and, for a replacement, the
/// backup holds the replaced bytes); untouched entries are recorded as not
/// written; anything else is recorded as published-but-unverified so Undo
/// stays blocked until a person has looked.
fn terminal_journal_from_interrupted(
    durable: &SharedDurableJournal,
    states: &[SharedEntryRecoveryState],
    backup_root: &Path,
) -> SharedApplyJournal {
    let entries: Vec<SharedApplyEntry> = durable
        .entries
        .iter()
        .zip(states)
        .map(|(entry, state)| {
            let plan = &entry.plan_entry;
            let destination = plan
                .destination_root
                .to_path_buf()
                .ok()
                .zip(plan.destination_relative_path.to_path_buf().ok())
                .map(|(root, relative)| root.join(relative));
            let replace = plan.proposed_action == PreviewProposedAction::Replace;
            let mut result = SharedApplyEntry {
                plan_entry: plan.clone(),
                destination_existed_before_apply: entry.destination_existed_before_apply,
                destination_parent_existed_before_apply: None,
                observed_source_digest: Some(plan.source_digest.clone()),
                observed_destination_digest: entry.observed_destination_digest.clone(),
                backup_path: entry.backup_path.clone(),
                backup_digest: entry.backup_digest.clone(),
                destination_pre_mode: entry.destination_pre_mode,
                temporary_path: entry.temporary_path.clone(),
                final_destination_digest: None,
                created_directories: Vec::new(),
                replacement_approved: replace,
                verification_succeeded: false,
                outcome: SharedApplyOutcome::WriteFailed,
                publication: SharedPublicationState::NotPublished,
                stages: vec![if replace {
                    SharedTransactionStage::ReplaceExisting
                } else {
                    SharedTransactionStage::InstallNew
                }],
                warnings: vec![
                    "EmuWiz was interrupted before this install's receipt was written; this record was rebuilt from what the files prove.".into(),
                ],
                failures: Vec::new(),
            };
            match state {
                SharedEntryRecoveryState::Applied => {
                    // A replacement without a backup on disk would have
                    // reconciled as UnsafeToResume, never Applied.
                    if replace && entry.backup_path.is_none() {
                        result.publication = SharedPublicationState::PublishedThenFailed;
                        return fail_result(
                            result,
                            SharedApplyOutcome::VerificationFailed,
                            SharedApplyFailureKind::PublishedUnverified,
                            destination.as_deref(),
                            "the file holds EmuWiz's bytes but no backup of the replaced file is recorded; review it by hand before any Undo",
                        );
                    }
                    result.final_destination_digest = Some(
                        entry
                            .resulting_destination_digest
                            .clone()
                            .unwrap_or_else(|| plan.source_digest.clone()),
                    );
                    result.verification_succeeded = true;
                    result.publication = SharedPublicationState::Published;
                    result.outcome = match plan.proposed_action {
                        PreviewProposedAction::Replace => SharedApplyOutcome::ReplacedExisting,
                        PreviewProposedAction::Skip => SharedApplyOutcome::AlreadyInstalled,
                        _ => SharedApplyOutcome::InstalledNew,
                    };
                    if replace {
                        result.stages.push(SharedTransactionStage::BackupCreated);
                    }
                    result.stages.push(SharedTransactionStage::Success);
                    result
                        .stages
                        .push(SharedTransactionStage::JournalFailedAfterSuccessfulWrite);
                    result
                }
                SharedEntryRecoveryState::Planned => fail_result(
                    result,
                    SharedApplyOutcome::WriteFailed,
                    SharedApplyFailureKind::JournalFailed,
                    destination.as_deref(),
                    "EmuWiz was interrupted before this file was written; the destination is unchanged",
                ),
                _ => {
                    result.publication = SharedPublicationState::PublishedThenFailed;
                    let stray = destination
                        .as_deref()
                        .and_then(stray_publication_file_path);
                    let detail = match stray {
                        Some(stray) => format!(
                            "EmuWiz was interrupted while writing in this folder: a temporary file from that write is still beside this file at {}. Check which of the two holds what you want, remove the temporary file by hand, then run Undo from History if needed. Until then Undo stays blocked and nothing is changed.",
                            stray.display()
                        ),
                        None => "EmuWiz was interrupted and this file is in a state the record cannot explain (neither the planned bytes nor the previous ones); review it by hand before any Undo".to_string(),
                    };
                    fail_result(
                        result,
                        SharedApplyOutcome::VerificationFailed,
                        SharedApplyFailureKind::PublishedUnverified,
                        destination.as_deref(),
                        &detail,
                    )
                }
            }
        })
        .collect();
    let _ = backup_root;
    let any_write = entries.iter().any(|entry| {
        matches!(
            entry.outcome,
            SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
        )
    });
    SharedApplyJournal {
        schema_version: SHARED_APPLY_SCHEMA_VERSION,
        operation_id: durable.operation_id.clone(),
        plan_id: durable.plan_id.clone(),
        timestamp_unix_seconds: durable.timestamp_unix_seconds,
        context: durable.context.clone(),
        approved_source_root: durable.approved_source_root.clone(),
        destination_root: durable.destination_root.clone(),
        created_root_directories: durable.created_root_directories.clone(),
        dry_run: false,
        entries,
        // Never Success: the operation did not complete its own receipt.
        status: if any_write {
            SharedApplyStatus::PartialFailure
        } else {
            SharedApplyStatus::Failed
        },
        rollback_operation_id: None,
    }
}

/// A regular file named like this module's temporary publication files (or
/// an Undo quarantine file) in the destination's folder. They are always
/// removed on the normal paths, so one that remains marks an interrupted or
/// refused publication or removal.
fn stray_publication_file_beside(destination: &Path) -> bool {
    stray_publication_file_path(destination).is_some()
}

/// The first such stray file's path, for a message that tells the person
/// what to look at.
fn stray_publication_file_path(destination: &Path) -> Option<PathBuf> {
    let parent = destination.parent()?;
    let mut strays: Vec<PathBuf> = fs::read_dir(parent)
        .ok()?
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            (name.starts_with(".archivefs-apply-") || name.starts_with(UNDO_QUARANTINE_PREFIX))
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .map(|entry| entry.path())
        .collect();
    strays.sort();
    strays.into_iter().next()
}

fn reconcile_durable_entry(
    entry: &SharedDurableEntry,
    transaction_state: SharedRecoveryState,
) -> SharedEntryRecoveryState {
    let Ok(root) = entry.plan_entry.destination_root.to_path_buf() else {
        return SharedEntryRecoveryState::UnsafeToResume;
    };
    let Ok(relative) = entry.plan_entry.destination_relative_path.to_path_buf() else {
        return SharedEntryRecoveryState::UnsafeToResume;
    };
    let destination = root.join(relative);
    let destination_digest = stable_hash(&destination, SHARED_MAX_SOURCE_BYTES)
        .ok()
        .map(|hash| hash.digest);
    let expected_new = entry.plan_entry.source_digest.as_str();
    let expected_old = entry.plan_entry.destination_pre_digest.as_deref();
    let backup_valid = if entry.plan_entry.backup_required {
        entry
            .backup_path
            .as_ref()
            .and_then(|path| path.to_path_buf().ok())
            .and_then(|path| stable_hash(&path, SHARED_MAX_BACKUP_BYTES).ok())
            .is_some_and(|hash| Some(hash.digest.as_str()) == expected_old)
    } else {
        true
    };
    // An interrupted exchange can leave the displaced file under a hidden
    // publication name beside the destination; until someone looks at it the
    // entry is not settled, whatever the destination itself holds.
    if stray_publication_file_beside(&destination) {
        return SharedEntryRecoveryState::NeedsReview;
    }
    match transaction_state {
        SharedRecoveryState::RollingBack | SharedRecoveryState::RollbackFailed => {
            if (entry.destination_existed_before_apply == Some(false)
                && destination_digest.is_none())
                || (backup_valid && entry.backup_digest.as_deref() == destination_digest.as_deref())
                || (expected_old.is_none() && destination_digest.is_none())
            {
                SharedEntryRecoveryState::RolledBack
            } else if destination_digest.as_deref() == Some(expected_new) {
                SharedEntryRecoveryState::RollingBack
            } else {
                SharedEntryRecoveryState::NeedsReview
            }
        }
        _ => {
            if destination_digest.as_deref() == Some(expected_new) {
                if backup_valid {
                    SharedEntryRecoveryState::Applied
                } else {
                    SharedEntryRecoveryState::UnsafeToResume
                }
            } else if destination_digest.as_deref() == expected_old {
                SharedEntryRecoveryState::Planned
            } else if destination_digest.is_none() && expected_old.is_none() {
                if matches!(
                    entry.state,
                    SharedEntryRecoveryState::Planned | SharedEntryRecoveryState::Applying
                ) {
                    SharedEntryRecoveryState::Planned
                } else {
                    SharedEntryRecoveryState::UnsafeToResume
                }
            } else {
                SharedEntryRecoveryState::NeedsReview
            }
        }
    }
}

fn write_journal_once(
    journal: &SharedApplyJournal,
    history_root: &Path,
) -> Result<PathBuf, SharedApplyFailure> {
    if should_inject(FaultPoint::JournalWrite) {
        return Err(failure(
            SharedApplyFailureKind::JournalFailed,
            None,
            "injected journal write failure",
        ));
    }
    let identifier = safe_identifier(&journal.operation_id).map_err(|kind| {
        failure(
            kind,
            None,
            "operation ID is not safe for a journal filename",
        )
    })?;
    let bytes = serde_json::to_vec_pretty(journal).map_err(|error| {
        failure(
            SharedApplyFailureKind::JournalFailed,
            None,
            &error.to_string(),
        )
    })?;
    if bytes.len() as u64 > SHARED_MAX_JOURNAL_BYTES {
        return Err(failure(
            SharedApplyFailureKind::ResourceLimitReached,
            None,
            "journal size limit reached",
        ));
    }
    let path = history_root.join(format!("{identifier}.json"));
    atomic_managed_write(&path, &bytes)
        .map_err(|kind| failure(kind, Some(&path), "journal could not be written atomically"))?;
    Ok(path)
}

fn read_journal(path: &Path) -> Result<SharedApplyJournal, SharedApplyFailure> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        failure(
            SharedApplyFailureKind::InvalidJournal,
            Some(path),
            &error.to_string(),
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > SHARED_MAX_JOURNAL_BYTES
    {
        return Err(failure(
            SharedApplyFailureKind::InvalidJournal,
            Some(path),
            "journal is not a bounded regular file",
        ));
    }
    let bytes = read_bounded(path, SHARED_MAX_JOURNAL_BYTES)
        .map_err(|kind| failure(kind, Some(path), "journal could not be read safely"))?;
    let journal: SharedApplyJournal = serde_json::from_slice(&bytes).map_err(|error| {
        failure(
            SharedApplyFailureKind::InvalidJournal,
            Some(path),
            &error.to_string(),
        )
    })?;
    if journal.schema_version != SHARED_APPLY_SCHEMA_VERSION {
        return Err(failure(
            SharedApplyFailureKind::UnsupportedJournal,
            Some(path),
            "journal schema version is unsupported",
        ));
    }
    Ok(journal)
}

fn rollback_entry_preview(
    entry: &SharedApplyEntry,
    root: &Path,
    backup_root: &Path,
    operation_id: &str,
    root_matches: bool,
    already_rolled_back: bool,
    recorded_rolled_back: bool,
) -> SharedRollbackEntry {
    let relative = entry.plan_entry.destination_relative_path.to_path_buf();
    let destination = relative
        .as_ref()
        .map(|relative| root.join(relative))
        .map_err(|kind| *kind);
    let mut result = SharedRollbackEntry {
        destination: destination
            .as_deref()
            .ok()
            .map(SharedTransactionPath::from_path),
        backup: entry.backup_path.clone(),
        expected_installed_digest: entry.final_destination_digest.clone(),
        observed_destination_digest: None,
        observed_backup_digest: None,
        outcome: SharedRollbackOutcome::Available,
        failure: None,
        permissions: None,
    };
    if !root_matches {
        result.outcome = SharedRollbackOutcome::RootMismatch;
        return result;
    }
    if already_rolled_back {
        result.outcome = SharedRollbackOutcome::AlreadyRolledBack;
        return result;
    }
    // An apply that published EmuWiz's file but kept the changed file it
    // displaced is not "nothing to undo": block the whole Undo until someone
    // has looked at the kept file.
    if let Some(kept) = entry
        .failures
        .iter()
        .find(|f| f.kind == SharedApplyFailureKind::DisplacedFileKept)
    {
        result.outcome = SharedRollbackOutcome::DestinationChanged;
        result.failure = Some(SharedApplyFailure {
            kind: SharedApplyFailureKind::DisplacedFileKept,
            path: kept.path.clone(),
            detail: format!(
                "{} Undo is blocked until that is resolved by hand.",
                kept.detail
            ),
        });
        return result;
    }
    // EmuWiz's file was published and then changed by something else: the
    // destination holds neither EmuWiz's file nor (for a replacement) the
    // user's original, which only the backup has. Not "nothing to undo".
    if let Some(changed) = entry
        .failures
        .iter()
        .find(|f| f.kind == SharedApplyFailureKind::PublishedUnverified)
    {
        let detail = match entry.backup_path.as_ref() {
            Some(backup) => format!(
                "EmuWiz's file was replaced by something else straight after it was written; the original is kept in EmuWiz's backup at {}. Undo is blocked until that is resolved by hand.",
                backup.display
            ),
            None => "A file EmuWiz installed was changed by something else straight after it was written. Undo is blocked until that is resolved by hand.".into(),
        };
        result.outcome = SharedRollbackOutcome::DestinationChanged;
        result.failure = Some(SharedApplyFailure {
            kind: SharedApplyFailureKind::PublishedUnverified,
            path: changed.path.clone(),
            detail,
        });
        return result;
    }
    if !matches!(
        entry.outcome,
        SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
    ) {
        // The journal says this file was not installed. If EmuWiz still
        // holds a receipt for it, and the file does not itself show that
        // nothing of the install is there, going on would undo the rest of
        // the install and leave this file behind as if it had never been
        // part of it.
        if let (Ok(destination), Ok(relative)) = (destination.as_ref(), relative.as_ref())
            && receipt_contradicts_not_installed(
                backup_root,
                operation_id,
                root,
                relative,
                destination,
                &entry.plan_entry,
            )
        {
            result.outcome = SharedRollbackOutcome::DestinationChanged;
            result.failure = Some(failure(
                SharedApplyFailureKind::RollbackBlocked,
                Some(destination),
                "EmuWiz's own record says it put a file here, and this history entry says it \
                 did not. Undo is blocked until that is resolved by hand.",
            ));
            return result;
        }
        result.outcome = SharedRollbackOutcome::NoChangeRequired;
        return result;
    }
    // A new file is only ever the outcome of a planned install, and a
    // replaced file of a planned replacement. The plan is what the
    // operation receipt holds the journal to; the outcome is not, so it is
    // held to the plan here.
    let as_planned = match entry.outcome {
        SharedApplyOutcome::InstalledNew => {
            entry.plan_entry.proposed_action == PreviewProposedAction::Install
        }
        _ => entry.plan_entry.proposed_action == PreviewProposedAction::Replace,
    };
    if !as_planned {
        result.outcome = SharedRollbackOutcome::JournalMalformed;
        result.failure = Some(failure(
            SharedApplyFailureKind::InvalidJournal,
            destination.as_deref().ok(),
            "this history entry says something was done to this file that the install \
             never planned",
        ));
        return result;
    }
    if entry
        .destination_existed_before_apply
        .is_some_and(|existed| {
            existed != matches!(entry.outcome, SharedApplyOutcome::ReplacedExisting)
        })
    {
        result.outcome = SharedRollbackOutcome::JournalMalformed;
        result.failure = Some(failure(
            SharedApplyFailureKind::InvalidJournal,
            destination.as_deref().ok(),
            "journal outcome contradicts explicit pre-apply destination existence",
        ));
        return result;
    }
    let (Ok(destination), Ok(relative)) = (destination, relative) else {
        result.outcome = SharedRollbackOutcome::DestinationUnsafe;
        return result;
    };
    // The file must be reached from the root by plain folders. A link
    // below the root could lead anywhere.
    if destination
        .strip_prefix(root)
        .is_ok_and(|below| reject_links_below(root, below).is_err())
    {
        result.outcome = SharedRollbackOutcome::DestinationUnsafe;
        return result;
    }
    match stable_hash(&destination, SHARED_MAX_SOURCE_BYTES) {
        Ok(hash) => {
            result.observed_destination_digest = Some(hash.digest.clone());
            if Some(hash.digest.as_str()) != entry.final_destination_digest.as_deref() {
                // Already back to the backed-up bytes (a partly finished
                // Undo restored it): nothing left to do for this file, so a
                // retry can finish the rest. "Back" is what this operation's
                // own backup holds, never what the journal says it holds: a
                // journal could otherwise have any file passed over by
                // naming its present bytes as the backup's.
                if entry.outcome == SharedApplyOutcome::ReplacedExisting
                    && own_backup_holds(backup_root, operation_id, &destination, &hash.digest)
                {
                    result.outcome = SharedRollbackOutcome::NoChangeRequired;
                    return result;
                }
                result.outcome = SharedRollbackOutcome::DestinationChanged;
                return result;
            }
        }
        Err(SharedApplyFailureKind::SourceMissing) => {
            // A new file an interrupted Undo had already removed (its
            // pending record says so): nothing left to do for this file,
            // so a retry can finish the rest.
            if entry.outcome == SharedApplyOutcome::InstalledNew
                && entry.destination_existed_before_apply == Some(false)
                && recorded_rolled_back
            {
                result.outcome = SharedRollbackOutcome::NoChangeRequired;
                return result;
            }
            result.outcome = SharedRollbackOutcome::DestinationMissing;
            return result;
        }
        Err(_) => {
            result.outcome = SharedRollbackOutcome::DestinationUnsafe;
            return result;
        }
    }
    // The file holds the bytes the record says were installed. That alone
    // does not make it this install's file: a file that was already there
    // can hold the same bytes, and a record can be written to say so. What
    // shows it is evidence the record cannot supply: what EmuWiz itself
    // put in the backup store, under this operation, when it installed.
    // For a replaced file that is the backup, checked below. For a file
    // installed where there was none it is the install receipt.
    if entry.outcome == SharedApplyOutcome::InstalledNew
        && !install_receipt_matches(
            backup_root,
            operation_id,
            root,
            &relative,
            entry.final_destination_digest.as_deref(),
        )
    {
        result.outcome = SharedRollbackOutcome::BackupMissing;
        result.failure = Some(failure(
            SharedApplyFailureKind::BackupMissing,
            Some(&destination),
            "EmuWiz has no record of its own, apart from this history entry, that it \
             installed this file. Installs made before EmuWiz kept such records do not have \
             one. The file has been kept as it is, and so has the history entry.",
        ));
        return result;
    }
    if entry.outcome == SharedApplyOutcome::ReplacedExisting {
        result.permissions = Some(SharedRollbackPermissions::for_recorded_mode(
            entry.destination_pre_mode,
        ));
        let Some(backup) = entry.backup_path.as_ref() else {
            result.outcome = SharedRollbackOutcome::BackupMissing;
            return result;
        };
        let Ok(backup) = backup.to_path_buf() else {
            result.outcome = SharedRollbackOutcome::BackupMissing;
            return result;
        };
        // The backup must be the one EmuWiz keeps for this file under this
        // operation: its place in the backup store follows from the
        // operation and the file's path, and the journal may only agree
        // with it. It is strictly inside the backup folder the caller
        // named and is reached without a link.
        let is_this_operations_backup =
            operation_evidence_path(backup_root, operation_id, &destination, BACKUP_SUFFIX)
                .is_ok_and(|expected| expected == backup);
        if !is_this_operations_backup
            || !strictly_inside(&backup, backup_root)
                .is_some_and(|below| reject_links_below(backup_root, below).is_ok())
        {
            result.outcome = SharedRollbackOutcome::BackupChanged;
            return result;
        }
        // The file holds what the journal says was installed. Whether that
        // is what EmuWiz installed is EmuWiz's own word, where it kept one:
        // otherwise a journal could have the original put back over newer
        // content merely by naming that content as the install's.
        if let Err(reason) = replacement_receipt_agrees(
            backup_root,
            operation_id,
            root,
            &relative,
            entry.final_destination_digest.as_deref(),
        ) {
            result.outcome = SharedRollbackOutcome::JournalMalformed;
            result.failure = Some(failure(
                SharedApplyFailureKind::InvalidJournal,
                Some(&destination),
                reason,
            ));
            return result;
        }
        match stable_hash(&backup, SHARED_MAX_SOURCE_BYTES) {
            Ok(hash) => {
                result.observed_backup_digest = Some(hash.digest.clone());
                if Some(hash.digest.as_str()) != entry.backup_digest.as_deref() {
                    result.outcome = SharedRollbackOutcome::BackupChanged;
                }
            }
            Err(SharedApplyFailureKind::SourceMissing) => {
                result.outcome = SharedRollbackOutcome::BackupMissing
            }
            Err(_) => result.outcome = SharedRollbackOutcome::BackupChanged,
        }
    }
    result
}

/// Name prefix of the private file an Undo moves an installed file into
/// before deleting it. Never shared between operations.
const UNDO_QUARANTINE_PREFIX: &str = ".archivefs-undo-";

/// Removes the file an apply installed at `destination`, and only that
/// file. The file is first moved under a private name in the same folder
/// (`RENAME_NOREPLACE`, so nothing is ever clobbered), then hashed there,
/// where nothing writing by path can reach it any more. Only a file still
/// holding `expected_digest` (EmuWiz's own bytes) is deleted. Anything else
/// is put straight back under its own name; if its name was taken in the
/// meantime it is kept under the quarantine name, which is returned so the
/// caller can say where the file is. The check-then-delete window of a
/// plain `remove_file` (Night Watch S04-05) does not exist here.
fn remove_and_verify_new_destination(
    destination: &Path,
    expected_digest: &str,
) -> Result<(), (SharedApplyFailureKind, Option<PathBuf>)> {
    reject_symlink_components(destination).map_err(|kind| (kind, None))?;
    let parent = destination
        .parent()
        .ok_or((SharedApplyFailureKind::InvalidPlan, None))?;
    let quarantine = parent.join(format!(
        "{UNDO_QUARANTINE_PREFIX}{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    before_remove(destination);
    if let Err(error) = rename_no_replace(destination, &quarantine) {
        return Err((
            if error.kind() == std::io::ErrorKind::NotFound {
                SharedApplyFailureKind::SourceMissing
            } else {
                SharedApplyFailureKind::WriteFailed
            },
            None,
        ));
    }
    let still_ours = stable_hash(
        &quarantine,
        SHARED_MAX_SOURCE_BYTES.max(SHARED_MAX_MATERIALIZED_BYTES),
    )
    .is_ok_and(|hash| hash.digest == expected_digest);
    if !still_ours {
        // Not EmuWiz's file any more: put it back exactly as it is.
        return Err(match rename_no_replace(&quarantine, destination) {
            Ok(()) => (SharedApplyFailureKind::DestinationChanged, None),
            Err(_) => (SharedApplyFailureKind::DisplacedFileKept, Some(quarantine)),
        });
    }
    if let Err(_error) = fs::remove_file(&quarantine) {
        // Deleting our own quarantined copy failed; it is still ours, so put
        // it back rather than leave it under a hidden name.
        return Err(match rename_no_replace(&quarantine, destination) {
            Ok(()) => (SharedApplyFailureKind::WriteFailed, None),
            Err(_) => (SharedApplyFailureKind::DisplacedFileKept, Some(quarantine)),
        });
    }
    if should_inject(FaultPoint::RollbackRemovalVerification) {
        fs::write(destination, b"")
            .map_err(|_| (SharedApplyFailureKind::VerificationFailed, None))?;
    }
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            sync_directory(parent);
            Ok(())
        }
        _ => Err((SharedApplyFailureKind::VerificationFailed, None)),
    }
}

/// Removes the folders an entry created for its file, deepest first, once
/// the file is gone. Each is reached from the destination root through
/// already-open folders, never by following a link and never by a path
/// that leaves the root, and is removed with `rmdir`, which succeeds only
/// on a folder that holds nothing. A journal names these folders, so each
/// name is checked again here: anything that is not a plain path strictly
/// inside the root is skipped.
#[cfg(unix)]
fn cleanup_created_directories(entry: &SharedApplyEntry, root: &Path) {
    if entry.destination_parent_existed_before_apply == Some(true) {
        return;
    }
    let Ok(root_fd) = fd_relative::open_dir_no_follow(root) else {
        return;
    };
    for encoded in entry.created_directories.iter().rev() {
        let Ok(path) = encoded.to_path_buf() else {
            continue;
        };
        let Some(below) = strictly_inside(&path, root) else {
            continue;
        };
        let names: Vec<&OsStr> = below
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        let Some((last, parents)) = names.split_last() else {
            continue;
        };
        // Open each parent in turn, relative to the one before it. A
        // parent that is missing, or is a link, ends the walk.
        let mut opened: Vec<std::os::fd::OwnedFd> = Vec::with_capacity(parents.len());
        let reached = parents.iter().all(|name| {
            let parent = opened
                .last()
                .map_or_else(|| fd_relative::as_raw(&root_fd), fd_relative::as_raw);
            match fd_relative::openat_dir_no_follow(parent, name) {
                Ok(fd) => {
                    opened.push(fd);
                    true
                }
                Err(_) => false,
            }
        });
        if !reached {
            continue;
        }
        let parent = opened
            .last()
            .map_or_else(|| fd_relative::as_raw(&root_fd), fd_relative::as_raw);
        let _ = fd_relative::unlinkat_rmdir(parent, last);
    }
}

#[cfg(not(unix))]
fn cleanup_created_directories(_entry: &SharedApplyEntry, _root: &Path) {}

fn prepare_managed_root(root: &Path) -> Result<(), SharedApplyFailureKind> {
    strict_absolute_root(root).map_err(|_| SharedApplyFailureKind::ManagedRootUnsafe)?;
    if root.exists() {
        validate_destination_root(root).map_err(|_| SharedApplyFailureKind::ManagedRootUnsafe)?;
        return Ok(());
    }
    let parent = root
        .parent()
        .ok_or(SharedApplyFailureKind::ManagedRootUnsafe)?;
    reject_symlink_components(parent)?;
    fs::create_dir(root).map_err(|_| SharedApplyFailureKind::ManagedRootUnsafe)
}

fn create_one_parent(root: &Path, parent: &Path) -> Result<(), SharedApplyFailureKind> {
    if should_inject(FaultPoint::ParentCreationRace) {
        return Err(SharedApplyFailureKind::ParentCreationFailed);
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| SharedApplyFailureKind::ParentCreationFailed)?;
    if relative.components().count() != 1 {
        return Err(SharedApplyFailureKind::ParentCreationFailed);
    }
    validate_destination_root(root).map_err(|_| SharedApplyFailureKind::RootChanged)?;
    fs::create_dir(parent).map_err(|_| SharedApplyFailureKind::ParentCreationFailed)?;
    let metadata =
        fs::symlink_metadata(parent).map_err(|_| SharedApplyFailureKind::ParentCreationFailed)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SharedApplyFailureKind::ParentCreationFailed);
    }
    Ok(())
}

/// Creates every missing directory level between `root` and `parent`,
/// returning the levels it actually created (creation order, outermost
/// first) so rollback can remove exactly those. Per-component
/// traversal/absolute-path checks are delegated to `destination_safety`; an
/// existing symlink or non-directory anywhere in the chain fails closed.
fn create_local_mod_parents(
    root: &Path,
    parent: &Path,
) -> Result<Vec<PathBuf>, SharedApplyFailureKind> {
    if should_inject(FaultPoint::ParentCreationRace) {
        return Err(SharedApplyFailureKind::ParentCreationFailed);
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| SharedApplyFailureKind::ParentCreationFailed)?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(SharedApplyFailureKind::ParentCreationFailed);
        };
        validate_single_component(name, root)
            .map_err(|_| SharedApplyFailureKind::ParentCreationFailed)?;
    }
    validate_destination_root(root).map_err(|_| SharedApplyFailureKind::RootChanged)?;
    let mut current = root.to_path_buf();
    let mut created = Vec::new();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(SharedApplyFailureKind::ParentCreationFailed);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .map_err(|_| SharedApplyFailureKind::ParentCreationFailed)?;
                created.push(current.clone());
            }
            Err(_) => return Err(SharedApplyFailureKind::ParentCreationFailed),
        }
    }
    Ok(created)
}

/// One directory level [`bootstrap_missing_destination_root`] itself
/// created and opened, in memory. `path` is display/journal-only from this
/// point on - every filesystem decision this module makes about a
/// bootstrap-created directory goes through `identity` (an `fstat`-derived
/// `(device, inode)` pair) and an open descriptor, never a re-resolved
/// pathname. See the `# Safety` section on [`bootstrap_missing_destination_root`].
#[derive(Debug, Clone)]
struct CreatedRootDirectory {
    path: PathBuf,
    identity: SharedDirectoryIdentity,
}

/// Minimal, narrowly-scoped `unsafe` wrappers around the POSIX fd-relative
/// primitives (`openat`/`mkdirat`/`unlinkat`/`fstat`) this module's
/// descriptor-anchored root bootstrap/cleanup is built on. Nothing outside
/// this inner module ever touches `libc` directly for this feature.
#[cfg(unix)]
mod fd_relative {
    use std::ffi::{CString, OsStr};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn component_cstring(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
    }

    /// Opens `path` as a directory, refusing to follow a symlink at its
    /// final component (`O_NOFOLLOW`). The *only* plain-pathname open in
    /// this whole feature - used exclusively for the one trusted starting
    /// ancestor a caller has already re-validated; every directory this
    /// module creates, opens, or removes after this point is resolved
    /// strictly relative to an already-open descriptor (`openat`/`mkdirat`/
    /// `unlinkat`), so nothing above this ancestor - or any sibling of it -
    /// can ever redirect where a later step lands, no matter what gets
    /// swapped into the path string afterward.
    pub(super) fn open_dir_no_follow(path: &Path) -> io::Result<OwnedFd> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
        // SAFETY: `c_path` is a valid, NUL-terminated C string for the
        // duration of this call; `open` either returns a valid owned fd
        // (>= 0, taken over by `OwnedFd`) or a negative value with `errno`
        // set, both handled below.
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_RDONLY,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by `open` as a fresh, valid,
        // uniquely-owned descriptor (checked `>= 0` above).
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Opens `name` (a single path component - never validated as such
    /// here; every caller in this module already only ever passes one) as
    /// a directory directly beneath the already-open `parent` descriptor,
    /// refusing to follow a symlink. Resolution is entirely relative to
    /// `parent`'s own descriptor, never the filesystem root.
    pub(super) fn openat_dir_no_follow(parent: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
        let c_name = component_cstring(name)?;
        // SAFETY: `parent` is a live, caller-owned directory descriptor for
        // the duration of this call; `c_name` is a valid NUL-terminated C
        // string. `openat` either returns a valid owned fd or a negative
        // value with `errno` set.
        let fd = unsafe {
            libc::openat(
                parent,
                c_name.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_RDONLY,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by `openat` as a fresh, valid,
        // uniquely-owned descriptor (checked `>= 0` above).
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Creates directory `name` directly beneath the already-open `parent`
    /// descriptor. There is no path string involved beyond the single
    /// component `name`, so there is nothing above `parent` left to swap.
    pub(super) fn mkdirat_here(parent: RawFd, name: &OsStr) -> io::Result<()> {
        let c_name = component_cstring(name)?;
        // SAFETY: `parent` is a live directory descriptor; `c_name` is a
        // valid NUL-terminated C string. `mkdirat` returns `0` on success
        // or `-1` with `errno` set.
        let result = unsafe { libc::mkdirat(parent, c_name.as_ptr(), 0o777) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Removes directory `name` directly beneath the already-open `parent`
    /// descriptor - `unlinkat` with `AT_REMOVEDIR`, equivalent to `rmdir`.
    /// This is the *only* emptiness check this module ever performs: POSIX
    /// requires `rmdir`/`AT_REMOVEDIR` to both fail closed on a symlink
    /// (`ENOTDIR`, never dereferenced) and fail on a non-empty directory
    /// (`ENOTEMPTY`) atomically with the removal itself - there is
    /// deliberately no separate `read_dir`-then-`remove_dir` anywhere in
    /// this module, which would open a window between checking and acting
    /// that a path-based check alone could never close.
    pub(super) fn unlinkat_rmdir(parent: RawFd, name: &OsStr) -> io::Result<()> {
        let c_name = component_cstring(name)?;
        // SAFETY: `parent` is a live directory descriptor; `c_name` is a
        // valid NUL-terminated C string. `unlinkat` returns `0` on success
        // or `-1` with `errno` set.
        let result = unsafe { libc::unlinkat(parent, c_name.as_ptr(), libc::AT_REMOVEDIR) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The `(device, inode, is_directory)` triple for an already-open
    /// descriptor, read via `fstat` on the descriptor itself - never by
    /// re-resolving a pathname, so nothing that happens to a name
    /// afterward can change what this reports.
    pub(super) fn fstat_identity(fd: RawFd) -> io::Result<(u64, u64, bool)> {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is a live descriptor; `&mut stat` is a valid,
        // properly-sized out-parameter for the duration of this call.
        let result = unsafe { libc::fstat(fd, &mut stat) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        let is_dir = (stat.st_mode & libc::S_IFMT) == libc::S_IFDIR;
        Ok((stat.st_dev as u64, stat.st_ino as u64, is_dir))
    }

    pub(super) fn as_raw(fd: &OwnedFd) -> RawFd {
        fd.as_raw_fd()
    }
}

/// Safely creates `root` and any missing ancestors up to (but never above)
/// the nearest already-existing directory, when `root` itself does not
/// exist yet. A no-op returning an empty list when `root` already exists -
/// existing-root transactions are completely unaffected by this function
/// ever having been called.
///
/// # Why this exists
///
/// [`RootLock::acquire`] (and every other destination write in this module)
/// requires `root` to already exist. A fresh adapter destination - the
/// motivating case is a Dolphin profile's `Load/Textures`, which does not
/// exist until a texture pack is first installed - never has that. This is
/// the one place that gap is closed, narrowly: only the exact missing
/// `root` chain a caller already validated and approved is ever created,
/// never anything wider.
///
/// # Safety
///
/// [`validate_destination_root`] is used only to find a *plausible*
/// starting ancestor and to fail closed early on an obviously unsafe
/// `root` - it is never, by itself, treated as proof that stays true by
/// the time a directory is actually created. The real authority is
/// descriptor-anchored, no-follow traversal (see [`fd_relative`]): the
/// starting ancestor is (re)opened with `O_NOFOLLOW`, and every directory
/// below it is created with `mkdirat` and then immediately reopened with
/// `openat`+`O_NOFOLLOW` *relative to that already-open parent descriptor* -
/// never a fresh top-level pathname lookup. If an ancestor is swapped to
/// a symlink at any point after it was opened, there is no path string left
/// for that swap to redirect: every subsequent `openat`/`mkdirat` resolves
/// strictly against the open descriptor chain already established, not
/// against `/`. A swap of the *exact* newly created entry, in the narrow
/// window between `mkdirat` and the immediately following `openat`+
/// `O_NOFOLLOW`, is the one gap POSIX itself does not offer an atomic
/// primitive to close (there is no `O_CREAT|O_DIRECTORY`); it fails closed
/// at that `openat` (`ENOTDIR`/`ELOOP`) rather than silently proceeding.
///
/// A failure partway through removes whatever prefix of the chain this
/// call already created, deepest first, through the same descriptor chain
/// (see [`remove_verified_root_chain`]) rather than leaving a partial,
/// orphaned chain behind.
///
/// This function itself never decides whether creating a missing root was
/// *approved* - see `execute_shared_apply`'s own `parent_creation_approved`
/// gate, which is checked before this is ever called.
#[cfg(unix)]
fn bootstrap_missing_destination_root(
    root: &Path,
) -> Result<Vec<CreatedRootDirectory>, SharedApplyFailureKind> {
    let validated_root =
        validate_destination_root(root).map_err(|_| SharedApplyFailureKind::DestinationUnsafe)?;
    if validated_root.state() == DestinationRootState::ExistingDirectory {
        return Ok(Vec::new());
    }
    // The normalized, absolute form `validate_destination_root` itself
    // already proved has no `..`/empty components - never the raw
    // caller-supplied `root` from here on.
    let root = validated_root.path().to_path_buf();

    // Find a *plausible* starting ancestor - see this function's own
    // `# Safety` section for why this walk is only ever a starting point,
    // never the actual safety guarantee.
    let mut existing_ancestor: Option<PathBuf> = None;
    for ancestor in root.ancestors().skip(1) {
        match validate_destination_root(ancestor) {
            Ok(validated) if validated.state() == DestinationRootState::ExistingDirectory => {
                existing_ancestor = Some(ancestor.to_path_buf());
                break;
            }
            Ok(_) => continue,
            Err(_) => return Err(SharedApplyFailureKind::DestinationUnsafe),
        }
    }
    let Some(existing_ancestor) = existing_ancestor else {
        return Err(SharedApplyFailureKind::DestinationUnsafe);
    };

    let missing_suffix = root
        .strip_prefix(&existing_ancestor)
        .map_err(|_| SharedApplyFailureKind::DestinationUnsafe)?;
    let mut missing_names = Vec::new();
    for component in missing_suffix.components() {
        match component {
            Component::Normal(name) => missing_names.push(name.to_os_string()),
            // A validated, normalized path can only ever have `Normal`
            // components below an existing prefix; anything else here
            // means the two walks above somehow disagreed - fail closed
            // rather than guess.
            _ => return Err(SharedApplyFailureKind::DestinationUnsafe),
        }
    }
    if missing_names.is_empty() {
        // `root` validated `Absent` above but now has an existing ancestor
        // equal to itself - it was recreated between the two checks.
        return Err(SharedApplyFailureKind::RootChanged);
    }
    if missing_names.len() > SHARED_MAX_CREATED_DIRECTORIES {
        return Err(SharedApplyFailureKind::ResourceLimitReached);
    }

    // The one and only plain-pathname open in this whole function - see
    // `fd_relative::open_dir_no_follow`'s own doc comment.
    let anchor_fd = fd_relative::open_dir_no_follow(&existing_ancestor)
        .map_err(|_| SharedApplyFailureKind::RootChanged)?;

    let mut open_chain = vec![anchor_fd];
    let mut created: Vec<CreatedRootDirectory> = Vec::new();
    let mut path_so_far = existing_ancestor;
    for name in missing_names {
        path_so_far.push(&name);
        let parent_fd = fd_relative::as_raw(open_chain.last().expect("anchor always present"));

        if should_inject(FaultPoint::ParentCreationRace)
            || fd_relative::mkdirat_here(parent_fd, &name).is_err()
        {
            remove_verified_root_chain(&created);
            return Err(SharedApplyFailureKind::ParentCreationFailed);
        }
        let child_fd = match fd_relative::openat_dir_no_follow(parent_fd, &name) {
            Ok(fd) => fd,
            Err(_) => {
                // The narrow post-`mkdirat` window this function's own
                // `# Safety` section documents - fail closed, and clean up
                // only what was verified before this point.
                remove_verified_root_chain(&created);
                return Err(SharedApplyFailureKind::ParentCreationFailed);
            }
        };
        let identity = match fd_relative::fstat_identity(fd_relative::as_raw(&child_fd)) {
            Ok((device, inode, true)) => SharedDirectoryIdentity { device, inode },
            Ok((_, _, false)) | Err(_) => {
                remove_verified_root_chain(&created);
                return Err(SharedApplyFailureKind::ParentCreationFailed);
            }
        };
        created.push(CreatedRootDirectory {
            path: path_so_far.clone(),
            identity,
        });
        open_chain.push(child_fd);
    }
    Ok(created)
}

#[cfg(not(unix))]
fn bootstrap_missing_destination_root(
    _root: &Path,
) -> Result<Vec<CreatedRootDirectory>, SharedApplyFailureKind> {
    // The descriptor-relative primitives this feature depends on
    // (`openat`/`mkdirat`/`unlinkat`/`O_NOFOLLOW`) are POSIX-only, exactly
    // like `RootLock::acquire`'s own non-unix fallback - fails closed
    // rather than falling back to a weaker, path-based strategy.
    Err(SharedApplyFailureKind::LockUnsupported)
}

/// Descriptor-anchored, identity-verified removal of a bootstrap-created
/// destination-root chain, deepest first. Shared by every cleanup call
/// site in this module - a same-call abort partway through
/// [`bootstrap_missing_destination_root`], a `RootLock` failure
/// immediately after a successful bootstrap, an all-failed apply cleaning
/// up its own just-created empty root, and real rollback reading a
/// validated chain back from a persisted journal (see
/// [`validate_created_root_chain`]) - the exact same safety discipline
/// every time.
///
/// For each entry, outermost first: opens it relative to the previous
/// level's own already-open descriptor (`openat`+`O_NOFOLLOW` - the first
/// entry's parent, the "anchor", is opened once via
/// [`fd_relative::open_dir_no_follow`], the one plain-pathname open in this
/// call), reads its `(device, inode)` identity via `fstat` on that
/// descriptor, and compares it against the identity `entry` itself already
/// carries. The walk stops - without opening anything further - at the
/// first level that fails to open or whose identity disagrees: a directory
/// that no longer matches is never assumed to be "close enough," and
/// nothing deeper than an already-stopped level is ever reachable to be
/// removed by mistake.
///
/// Only entries that matched are then removed, deepest first, each via
/// `unlinkat`+`AT_REMOVEDIR` against the parent descriptor already opened
/// above - the one atomic operation that both proves emptiness and
/// performs the removal (see [`fd_relative::unlinkat_rmdir`]'s own doc
/// comment). Stops at the first removal failure (non-empty, or anything
/// else) and never continues to a shallower level after that.
///
/// Returns how many entries (counted from the deepest/last) were actually
/// removed, so a caller updating a journal can `truncate` to exactly what
/// remains true on disk rather than continuing to claim ownership of
/// something already gone.
#[cfg(unix)]
fn remove_verified_root_chain(chain: &[CreatedRootDirectory]) -> usize {
    remove_deepest_of_verified_root_chain(chain, chain.len())
}

/// [`remove_verified_root_chain`], removing at most the `deepest` deepest
/// entries. When that is fewer than the whole chain, nothing is removed
/// unless every entry verified: a chain that is wrong about one folder is
/// not trusted about another.
#[cfg(unix)]
fn remove_deepest_of_verified_root_chain(chain: &[CreatedRootDirectory], deepest: usize) -> usize {
    let Some(first) = chain.first() else {
        return 0;
    };
    let Some(anchor_path) = first.path.parent() else {
        return 0;
    };
    let Ok(anchor_fd) = fd_relative::open_dir_no_follow(anchor_path) else {
        return 0;
    };

    let mut open_chain = vec![anchor_fd];
    let mut matched = 0usize;
    for entry in chain {
        let Some(name) = entry.path.file_name() else {
            break;
        };
        let parent_fd = fd_relative::as_raw(open_chain.last().expect("anchor always present"));
        let Ok(child_fd) = fd_relative::openat_dir_no_follow(parent_fd, name) else {
            break;
        };
        let Ok((device, inode, is_dir)) =
            fd_relative::fstat_identity(fd_relative::as_raw(&child_fd))
        else {
            break;
        };
        if !is_dir || device != entry.identity.device || inode != entry.identity.inode {
            break;
        }
        open_chain.push(child_fd);
        matched += 1;
    }

    let keep = chain.len().saturating_sub(deepest);
    if keep > 0 && matched != chain.len() {
        return 0;
    }
    let mut removed = 0usize;
    for index in (keep..matched).rev() {
        let Some(name) = chain[index].path.file_name() else {
            break;
        };
        let parent_fd = fd_relative::as_raw(&open_chain[index]);
        if fd_relative::unlinkat_rmdir(parent_fd, name).is_err() {
            break;
        }
        removed += 1;
    }
    removed
}

#[cfg(not(unix))]
fn remove_verified_root_chain(_chain: &[CreatedRootDirectory]) -> usize {
    0
}

#[cfg(not(unix))]
fn remove_deepest_of_verified_root_chain(
    _chain: &[CreatedRootDirectory],
    _deepest: usize,
) -> usize {
    0
}

/// Validates `journal.created_root_directories` as untrusted, persisted
/// input before anything in it is ever opened, let alone removed. A
/// tampered or corrupted claim is rejected as a whole - never trimmed to
/// "the parts that look fine" - by returning an empty `Vec`:
///
/// - bounded by [`SHARED_MAX_CREATED_DIRECTORIES`], the same limit
///   creation itself enforces;
/// - `journal.destination_root` must equal `root` (defense in depth
///   alongside the `root_matches` check already performed before
///   [`execute_shared_rollback`] is ever reachable);
/// - every path must be absolute with only `Normal` components (no `..`,
///   no empty component);
/// - entries must form one exact, contiguous parent/child chain - each
///   entry after the first must be a direct child of the previous one, so
///   there is no gap, no repeat, and no out-of-order entry. Structure alone
///   does **not** keep a pre-existing ancestor out of the chain: a journal
///   that prepends ancestors of the destination root still forms a
///   contiguous chain and passes this function, and with each ancestor's
///   real `(device, inode)` its claim cannot be told from a true one. For
///   that reason the chain is validated as a whole here, but only its last
///   entry, the destination root itself, is ever removed (see
///   [`cleanup_transaction_created_root_directories`]). Folders above the
///   root are never removed by Undo, whatever a journal says;
/// - the *last* entry must equal `root` itself - a chain that does not
///   actually terminate at this rollback's destination is rejected;
/// - every entry must carry an `identity` - one without it is unprovable
///   ownership, not proof of anything (see [`SharedCreatedRootDirectory`]'s
///   own doc comment).
fn validate_created_root_chain(
    journal: &SharedApplyJournal,
    root: &Path,
) -> Vec<CreatedRootDirectory> {
    let claimed = &journal.created_root_directories;
    if claimed.is_empty() || claimed.len() > SHARED_MAX_CREATED_DIRECTORIES {
        return Vec::new();
    }
    if journal.destination_root.to_path_buf().ok().as_deref() != Some(root) {
        return Vec::new();
    }

    let mut resolved: Vec<CreatedRootDirectory> = Vec::with_capacity(claimed.len());
    let mut previous: Option<PathBuf> = None;
    for (index, entry) in claimed.iter().enumerate() {
        let Ok(path) = entry.path.to_path_buf() else {
            return Vec::new();
        };
        if !path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
        {
            return Vec::new();
        }
        if let Some(previous_path) = &previous
            && path.parent() != Some(previous_path.as_path())
        {
            return Vec::new();
        }
        if index + 1 == claimed.len() && path != root {
            return Vec::new();
        }
        let Some(identity) = entry.identity else {
            return Vec::new();
        };
        previous = Some(path.clone());
        resolved.push(CreatedRootDirectory { path, identity });
    }
    resolved
}

/// Rollback-time removal of a transaction's own bootstrap-created
/// destination-root directory chain. Validates the persisted, untrusted
/// journal chain first (see [`validate_created_root_chain`]) and, only for
/// whatever survives that validation, removes it through the same
/// descriptor-anchored, identity-verified path every other cleanup in this
/// module uses (see [`remove_verified_root_chain`]).
///
/// A no-op for a journal with no `created_root_directories` at all -
/// including every journal written before this field existed (see its own
/// `#[serde(default)]`) - so rollback of an old journal is byte-for-byte
/// unchanged from before this function existed: an absent field carries no
/// root-cleanup authority, and there is no path by which it is
/// reinterpreted into any.
fn cleanup_transaction_created_root_directories(journal: &SharedApplyJournal, root: &Path) {
    let chain = validate_created_root_chain(journal, root);
    // Only the destination root itself, which the caller named, is ever
    // removed on a journal's word, and only when it is still the folder
    // the journal recorded and holds nothing. Folders above it are outside
    // what this Undo was opened for: a journal can claim them, with their
    // real identity, and nothing here can tell that claim from a true one.
    // They are left in place, empty or not. The whole chain is still
    // walked and checked: if any entry in it is not the folder the journal
    // says it is, the journal is wrong about its folders and the root is
    // left too.
    remove_deepest_of_verified_root_chain(&chain, 1);
}

/// File-name ending of a backup of a replaced file, in the backup store.
const BACKUP_SUFFIX: &str = ".bak";
/// File-name ending of the receipt for a file installed where there was
/// none, in the backup store. See [`write_receipt`].
const INSTALL_RECEIPT_SUFFIX: &str = ".new";
/// Name of the receipt, in an operation's folder in the backup store, that
/// says which install the folder belongs to. See [`write_operation_receipt`].
const OPERATION_RECEIPT_NAME: &str = "operation.plan";
/// How [`operation_identity_document`] is laid out. The receipt of every
/// install is read for as long as that install can be undone, so a change
/// to the layout, or to how any type in it is written out, needs a new
/// number here and the old reading kept beside the new.
const OPERATION_RECEIPT_FORMAT: u32 = 1;
/// File-name ending of the receipt for a file installed over an existing
/// one, in the backup store beside the backup of the file it replaced.
/// The two kinds of receipt are different files on purpose: a receipt for
/// a replacement can never stand in for the receipt of a new file, so it
/// can never be used to have a file removed. See [`write_receipt`].
const REPLACEMENT_RECEIPT_SUFFIX: &str = ".replacement";

/// Where the backup store keeps one operation's backup of the file it
/// replaced at `destination`. The place follows from the operation and the
/// file's path as text, exactly as it has since backups were first made,
/// so the backups of installs made by earlier versions are found. A
/// journal names its backup too; it may only agree with this.
fn operation_evidence_path(
    backup_root: &Path,
    operation_id: &str,
    destination: &Path,
    suffix: &str,
) -> Result<PathBuf, SharedApplyFailureKind> {
    Ok(backup_root
        .join(safe_identifier(operation_id)?)
        .join(format!(
            "{}{suffix}",
            digest_text(&destination.to_string_lossy())
        )))
}

/// Where the backup store keeps one operation's receipt for the file it
/// installed at `relative` below the destination root `root`.
///
/// The name is the digest of the root and of the path below it, each by
/// its exact bytes and kept apart by a byte no path can hold. So a receipt
/// belongs to one file of one install made for one destination root. A
/// journal that names the same file through another root (a wider one,
/// say, with the rest moved into the path) names a receipt that is not
/// there, and so does one that differs only in bytes that are not text.
///
/// This matters because not every caller has a destination root of its
/// own to open Undo for: History opens it for the root the journal names.
/// The root a journal names is therefore tied to the install by the
/// receipt, not by the caller.
fn operation_receipt_path(
    backup_root: &Path,
    operation_id: &str,
    root: &Path,
    relative: &Path,
    suffix: &str,
) -> Result<PathBuf, SharedApplyFailureKind> {
    #[cfg(unix)]
    let named = {
        use std::os::unix::ffi::OsStrExt;
        let mut named = root.as_os_str().as_bytes().to_vec();
        named.push(0);
        named.extend_from_slice(relative.as_os_str().as_bytes());
        named
    };
    #[cfg(not(unix))]
    let named = format!("{}\0{}", root.to_string_lossy(), relative.to_string_lossy()).into_bytes();
    Ok(backup_root
        .join(safe_identifier(operation_id)?)
        .join(format!("{}{suffix}", digest_bytes(&named))))
}

/// The folder in the backup store that holds one operation's evidence,
/// created if this is the operation's first. Anything else at that name (a
/// link, a file) is refused.
fn operation_evidence_folder(
    backup_root: &Path,
    operation_id: &str,
) -> Result<PathBuf, SharedApplyFailureKind> {
    prepare_managed_root(backup_root)?;
    let operation = backup_root.join(safe_identifier(operation_id)?);
    match fs::create_dir(&operation) {
        Ok(()) => Ok(operation),
        Err(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists
                && fs::symlink_metadata(&operation).is_ok_and(|metadata| metadata.is_dir()) =>
        {
            Ok(operation)
        }
        Err(_) => Err(SharedApplyFailureKind::BackupFailed),
    }
}

/// Records, in the backup store and under this operation, what EmuWiz is
/// about to install at `destination`: the digest of the bytes about to be
/// written. `suffix` says which kind of install it is, a new file or a
/// replacement. The receipt is written just before the file is published
/// (for a replacement, after its backup is made), so an interrupted
/// install can leave a receipt with no file. A crash of EmuWiz cannot
/// leave a file it installed without its receipt; after a power cut that
/// rests on the filesystem having kept the receipt, and if it did not,
/// the file is treated as not EmuWiz's, which is the safe direction. A
/// backup may stand without a receipt for its file: the operation's own
/// receipt already says the install kept receipts, so that backup is not
/// taken for an older install's.
///
/// A journal is a file that anything able to write the history folder can
/// write. A receipt is in the backup store, where a journal cannot put it:
/// it is something Undo can check that a journal cannot supply.
/// - For a new file it is the only thing that shows the file is this
///   install's. No receipt, no removal.
/// - For a replaced file the backup already shows that EmuWiz replaced the
///   file and what the original was. The receipt adds what was installed
///   over it, so that a journal cannot have the original put back over
///   something else merely by saying that is what was installed.
fn write_receipt(
    suffix: &str,
    root: &Path,
    relative: &Path,
    installed_digest: &str,
    backup_root: &Path,
    operation_id: &str,
) -> Result<PathBuf, SharedApplyFailureKind> {
    operation_evidence_folder(backup_root, operation_id)?;
    let receipt = operation_receipt_path(backup_root, operation_id, root, relative, suffix)?;
    atomic_managed_write(&receipt, installed_digest.as_bytes())?;
    Ok(receipt)
}

/// Removes this operation's receipt for one file, once the engine itself
/// has established that what the receipt describes is not there: the
/// install never published it, or published it, failed its check and took
/// it back, or Undo has removed it or put the original back. A receipt
/// must not outlive what it vouches for, or a later file with the same
/// bytes at the same place could be claimed with it. Where the outcome is
/// uncertain the receipt is left beside the file it may describe.
///
/// A receipt that could not be removed is still held to the file: Undo
/// acts on it only while the file holds exactly the bytes it names.
fn withdraw_receipt(
    suffix: &str,
    root: &Path,
    relative: &Path,
    backup_root: &Path,
    operation_id: &str,
) {
    let Ok(receipt) = operation_receipt_path(backup_root, operation_id, root, relative, suffix)
    else {
        return;
    };
    // Only ever a plain file strictly inside the backup folder the caller
    // named, reached without a link.
    if strictly_inside(&receipt, backup_root)
        .is_some_and(|below| reject_links_below(backup_root, below).is_ok())
    {
        let _ = fs::remove_file(receipt);
    }
}

/// What ties a journal to one install: the operation, its destination
/// root, and its whole plan (every file, in order, with what was to be
/// done to it, where it came from and which game it was for). All of it is
/// in the journal, and all of it is known before the install touches
/// anything.
fn operation_identity_document(
    operation_id: &str,
    context: &SharedApplyContext,
    approved_source_root: &SharedTransactionPath,
    destination_root: &SharedTransactionPath,
    plan_id: &str,
    entries: &[&SharedPlanEntry],
) -> Option<Vec<u8>> {
    serde_json::to_vec(&(
        OPERATION_RECEIPT_FORMAT,
        operation_id,
        context,
        approved_source_root,
        destination_root,
        plan_id,
        entries,
    ))
    .ok()
}

/// What an operation receipt holds for an install described by `document`.
fn operation_receipt_content(document: &[u8]) -> String {
    format!("{OPERATION_RECEIPT_FORMAT}:{}", digest_bytes(document))
}

/// Records in the backup store, before the install touches anything, which
/// install this operation's folder belongs to. If it cannot be written the
/// install does not start.
///
/// A file's receipt shows that EmuWiz installed that file. It does not
/// show which journal may speak for it: without this, a journal could
/// carry another install's operation and files, or leave some of its own
/// out, and Undo would act on whatever it listed. This receipt is written
/// by the install itself, from the plan it is about to carry out, and a
/// journal cannot put it there.
fn write_operation_receipt(
    journal: &SharedApplyJournal,
    plan: &SharedTransactionPlan,
    backup_root: &Path,
) -> Result<(), SharedApplyFailureKind> {
    let entries: Vec<&SharedPlanEntry> = plan.entries.iter().collect();
    let document = operation_identity_document(
        &journal.operation_id,
        &journal.context,
        &journal.approved_source_root,
        &journal.destination_root,
        &journal.plan_id,
        &entries,
    )
    .ok_or(SharedApplyFailureKind::BackupFailed)?;
    let folder = operation_evidence_folder(backup_root, &journal.operation_id)?;
    atomic_managed_write(
        &folder.join(OPERATION_RECEIPT_NAME),
        operation_receipt_content(&document).as_bytes(),
    )
    .map(|_| ())
}

/// What is wrong with `journal` as the record of the install the backup
/// store knows under its operation, if anything.
///
/// - A history entry is filed under its own operation's name. One filed
///   under another name is a copy of some other install's record.
/// - Where the install left an operation receipt, the journal must
///   describe exactly that install: the same operation, destination root
///   and plan. A journal that carries another install's files, adds to its
///   own, leaves some out, or describes itself as something else, does
///   not.
/// - An install made before these receipts existed left none, and nothing
///   else of the kind either ([`operation_keeps_receipts`]). Its journal
///   is believed as far as the per-file rules allow, as before.
fn operation_receipt_violation(
    journal: &SharedApplyJournal,
    journal_path: &Path,
    backup_root: &Path,
) -> Option<&'static str> {
    const UNUSABLE: &str = "EmuWiz's own record of which install this is cannot be read";
    let Ok(operation) = safe_identifier(&journal.operation_id) else {
        return Some("this history entry does not name its operation plainly");
    };
    if journal_path.file_name() != Some(OsStr::new(&format!("{operation}.json"))) {
        return Some("this history entry is filed under another install's name");
    }
    let below = Path::new(&operation).join(OPERATION_RECEIPT_NAME);
    if reject_links_below(backup_root, &below).is_err() {
        return Some(UNUSABLE);
    }
    let receipt = backup_root.join(below);
    match fs::symlink_metadata(&receipt) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if operation_keeps_receipts(backup_root, &journal.operation_id) {
                Some(
                    "EmuWiz kept receipts for this install, but not the one that says which \
                     install it is",
                )
            } else {
                None
            }
        }
        Ok(metadata) if metadata.is_file() => {
            let entries: Vec<&SharedPlanEntry> = journal
                .entries
                .iter()
                .map(|entry| &entry.plan_entry)
                .collect();
            let expected = operation_identity_document(
                &journal.operation_id,
                &journal.context,
                &journal.approved_source_root,
                &journal.destination_root,
                &journal.plan_id,
                &entries,
            )
            .map(|document| operation_receipt_content(&document));
            match read_bounded(&receipt, 256) {
                Ok(bytes) if expected.is_some_and(|expected| bytes == expected.as_bytes()) => None,
                Ok(_) => Some(
                    "this history entry is not the record of the install EmuWiz kept under \
                     this name: its files, its folder or its description have been changed",
                ),
                Err(_) => Some(UNUSABLE),
            }
        }
        _ => Some(UNUSABLE),
    }
}

/// For an entry a journal says was not installed: whether EmuWiz's own
/// receipt says otherwise and nothing shows the receipt to be spent.
///
/// A receipt is written just before a file is published and withdrawn
/// when the engine knows the file is not there. One that is still held
/// means EmuWiz may have put a file here. It is spent only if the file
/// itself shows that nothing of the install is there: the file is absent,
/// or, where the plan was to replace a file, it is the very original the
/// plan recorded. A file that is there in any other state, or cannot be
/// read, is one the journal may be leaving out, and that is refused.
fn receipt_contradicts_not_installed(
    backup_root: &Path,
    operation_id: &str,
    root: &Path,
    relative: &Path,
    destination: &Path,
    plan: &SharedPlanEntry,
) -> bool {
    let held = [INSTALL_RECEIPT_SUFFIX, REPLACEMENT_RECEIPT_SUFFIX]
        .into_iter()
        .any(|suffix| {
            operation_receipt_path(backup_root, operation_id, root, relative, suffix).is_ok_and(
                |receipt| {
                    !matches!(
                        fs::symlink_metadata(&receipt),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    )
                },
            )
        });
    if !held {
        return false;
    }
    match stable_hash(destination, SHARED_MAX_SOURCE_BYTES) {
        Err(SharedApplyFailureKind::SourceMissing) => false,
        Ok(hash) => {
            !(plan.proposed_action == PreviewProposedAction::Replace
                && plan.destination_pre_digest.as_deref() == Some(hash.digest.as_str()))
        }
        Err(_) => true,
    }
}

/// Whether this operation's own backup of `destination` holds exactly
/// `digest`: the file is back to its original. Read from the backup store,
/// where the operation put it, and not from what a journal says its backup
/// holds.
fn own_backup_holds(
    backup_root: &Path,
    operation_id: &str,
    destination: &Path,
    digest: &str,
) -> bool {
    let Ok(backup) = operation_evidence_path(backup_root, operation_id, destination, BACKUP_SUFFIX)
    else {
        return false;
    };
    strictly_inside(&backup, backup_root)
        .is_some_and(|below| reject_links_below(backup_root, below).is_ok())
        && stable_hash(&backup, SHARED_MAX_SOURCE_BYTES).is_ok_and(|hash| hash.digest == digest)
}

/// Whether the backup store holds this operation's receipt for
/// `destination`, saying it installed exactly `installed_digest` there.
/// The receipt is looked for where EmuWiz would have put it, inside the
/// backup folder the caller named, and is never reached through a link.
fn install_receipt_matches(
    backup_root: &Path,
    operation_id: &str,
    root: &Path,
    relative: &Path,
    installed_digest: Option<&str>,
) -> bool {
    let Some(installed_digest) = installed_digest else {
        return false;
    };
    let Ok(receipt) = operation_receipt_path(
        backup_root,
        operation_id,
        root,
        relative,
        INSTALL_RECEIPT_SUFFIX,
    ) else {
        return false;
    };
    let Some(below) = strictly_inside(&receipt, backup_root) else {
        return false;
    };
    if reject_links_below(backup_root, below).is_err() {
        return false;
    }
    // A receipt is one digest: anything longer is not a receipt.
    read_bounded(&receipt, 256).is_ok_and(|bytes| bytes == installed_digest.as_bytes())
}

/// Whether this operation's folder in the backup store holds a receipt of
/// any kind. An EmuWiz that keeps receipts writes the operation's own
/// before it touches anything, and never removes it; so a folder with a
/// backup and no receipt at all is from an EmuWiz that did not keep them,
/// and so is an operation with no folder. Anything else that cannot be
/// read is answered the strict way.
fn operation_keeps_receipts(backup_root: &Path, operation_id: &str) -> bool {
    let Ok(operation) = safe_identifier(operation_id) else {
        return true;
    };
    if reject_links_below(backup_root, Path::new(&operation)).is_err() {
        return true;
    }
    let entries = match fs::read_dir(backup_root.join(&operation)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return true;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == OPERATION_RECEIPT_NAME
            || name.ends_with(REPLACEMENT_RECEIPT_SUFFIX)
            || name.ends_with(INSTALL_RECEIPT_SUFFIX)
        {
            return true;
        }
    }
    false
}

/// Whether the journal's word for what was installed over the file at
/// `relative` below `root` may be believed: it must agree with this
/// operation's own receipt for that replacement.
///
/// Installs made before EmuWiz kept receipts have backups and no receipt
/// of any kind. For them there is nothing to hold the journal to, and Undo
/// goes on the journal's word, as it always did: refusing would make
/// every backup made by an earlier EmuWiz unusable. Whether an operation
/// is one of those is decided by what its folder in the backup store
/// holds ([`operation_keeps_receipts`]), never by how a journal names a
/// file: a journal can neither remove a receipt nor, by naming the file
/// some other way, make a newer install look like an older one. For an
/// install that kept receipts, a replaced file with no receipt is one
/// EmuWiz did not replace, or has already put back.
fn replacement_receipt_agrees(
    backup_root: &Path,
    operation_id: &str,
    root: &Path,
    relative: &Path,
    installed_digest: Option<&str>,
) -> Result<(), &'static str> {
    const UNUSABLE: &str = "EmuWiz's own record of what it installed over this file cannot be \
                            read, so the file was kept as it is";
    let Ok(receipt) = operation_receipt_path(
        backup_root,
        operation_id,
        root,
        relative,
        REPLACEMENT_RECEIPT_SUFFIX,
    ) else {
        return Err(UNUSABLE);
    };
    let Some(below) = strictly_inside(&receipt, backup_root) else {
        return Err(UNUSABLE);
    };
    if reject_links_below(backup_root, below).is_err() {
        return Err(UNUSABLE);
    }
    match fs::symlink_metadata(&receipt) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if operation_keeps_receipts(backup_root, operation_id) {
                Err(
                    "EmuWiz kept its own record of what this install put over files, and has \
                     none for this file as this history entry names it, so the file was kept \
                     as it is",
                )
            } else {
                // A backup from before these receipts existed.
                Ok(())
            }
        }
        Ok(metadata) if metadata.is_file() => {
            // A receipt is one digest: anything longer is not a receipt.
            match read_bounded(&receipt, 256) {
                Ok(bytes) if installed_digest.is_some_and(|digest| bytes == digest.as_bytes()) => {
                    Ok(())
                }
                Ok(_) => Err(
                    "this history entry does not agree with EmuWiz's own record of what it \
                     installed over this file, so the file was kept as it is",
                ),
                Err(_) => Err(UNUSABLE),
            }
        }
        _ => Err(UNUSABLE),
    }
}

/// Whether `path` is one or more plain names: no root, no `.`, no `..`.
/// The only shape a path taken from a journal may have before it is joined
/// to a folder the caller named.
fn is_plain_relative(path: &Path) -> bool {
    let mut names = 0usize;
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return false;
        }
        names += 1;
    }
    names > 0
}

/// The part of `path` below `root`, when `path` is absolute and lies
/// strictly inside `root` by plain names alone. The comparison is by
/// components, which is exact because the remainder may hold no `..`.
/// Links are not looked at here; they are dealt with where a path is used.
fn strictly_inside<'a>(path: &'a Path, root: &Path) -> Option<&'a Path> {
    if !path.is_absolute() {
        return None;
    }
    let below = path.strip_prefix(root).ok()?;
    is_plain_relative(below).then_some(below)
}

/// Refuses a path below `root` that is a symbolic link or is reached
/// through one. `root` is the caller's and is not examined.
fn reject_links_below(root: &Path, below: &Path) -> Result<(), SharedApplyFailureKind> {
    let mut current = root.to_path_buf();
    for component in below.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SharedApplyFailureKind::SourceSymlink);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(SharedApplyFailureKind::SourceChanged),
        }
    }
    Ok(())
}

/// What in `journal` reaches outside the destination root this Undo was
/// opened for, if anything.
///
/// A journal is a file that anything able to write EmuWiz's history folder
/// can rewrite. It records what an install did; it does not decide where
/// Undo may act. Undo acts only inside the root it was opened for. Every
/// entry must therefore name that same root, a plain relative path below
/// it for its file, and as folders it created only the folders on the way
/// from the root to that file. One entry that does not is enough to refuse
/// the whole journal: a record that is wrong about where it acted is not
/// trusted about anything else. The journal itself is left exactly as it
/// is.
///
/// Some callers have a root of their own to open Undo for. Others open it
/// for the root the journal names, so these rules alone tie no root to an
/// install: the receipts do ([`operation_receipt_violation`],
/// [`operation_receipt_path`]).
fn journal_boundary_violation(journal: &SharedApplyJournal, root: &Path) -> Option<&'static str> {
    let plain_absolute = root.is_absolute()
        && root
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)));
    if !plain_absolute {
        return Some("the destination folder is not a plain absolute path");
    }
    for entry in &journal.entries {
        match entry.plan_entry.destination_root.to_path_buf() {
            Ok(entry_root) if entry_root == root => {}
            _ => {
                return Some(
                    "this record names a destination folder other than the one Undo was \
                     opened for",
                );
            }
        }
        let destination = match entry.plan_entry.destination_relative_path.to_path_buf() {
            Ok(relative) if is_plain_relative(&relative) => root.join(relative),
            _ => {
                return Some(
                    "this record names a file by a path that is not a plain path inside \
                     the destination folder",
                );
            }
        };
        // An install only ever creates the folders on the way to its own
        // file. A folder anywhere else, however empty, is not one of them.
        if entry.created_directories.len() > SHARED_MAX_CREATED_DIRECTORIES {
            return Some("this record claims more created folders than an install makes");
        }
        for created in &entry.created_directories {
            match created.to_path_buf() {
                Ok(path)
                    if strictly_inside(&path, root).is_some()
                        && path != destination
                        && destination.starts_with(&path) => {}
                _ => {
                    return Some(
                        "this record claims a created folder that is not inside the \
                         destination folder, on the way to its own file",
                    );
                }
            }
        }
    }
    None
}

fn strict_absolute_root(root: &Path) -> Result<(), SharedApplyFailure> {
    if !root.is_absolute() || root.parent().is_none() {
        return Err(failure(
            SharedApplyFailureKind::DestinationUnsafe,
            Some(root),
            "root must be absolute and cannot be a filesystem root",
        ));
    }
    Ok(())
}

fn exactly_two_components(path: &Path) -> Option<(OsString, OsString)> {
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (components.len() == 2).then(|| (components[0].clone(), components[1].clone()))
}

fn roots_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn reject_symlink_components(path: &Path) -> Result<(), SharedApplyFailureKind> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SharedApplyFailureKind::SourceSymlink);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(SharedApplyFailureKind::SourceChanged),
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, SharedApplyFailureKind> {
    let hash = stable_hash(path, max)?;
    let mut bytes = Vec::with_capacity(hash.bytes as usize);
    let file = File::open(path).map_err(|_| SharedApplyFailureKind::SourceChanged)?;
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SharedApplyFailureKind::SourceChanged)?;
    if bytes.len() as u64 > max || digest_bytes(&bytes) != hash.digest {
        return Err(SharedApplyFailureKind::SourceChanged);
    }
    Ok(bytes)
}

fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.len() == right.len()
            && left.mtime() == right.mtime()
            && left.mtime_nsec() == right.mtime_nsec()
    }
    #[cfg(not(unix))]
    {
        left.len() == right.len() && left.modified().ok() == right.modified().ok()
    }
}

fn set_permissions(file: &File, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
        Ok(())
    }
}

fn sync_directory(path: &Path) {
    let _ = File::open(path).and_then(|file| file.sync_all());
}

#[cfg(target_os = "linux")]
fn rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(destination).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "destination exists",
        ));
    }
    fs::rename(source, destination)
}

#[derive(Debug)]
struct RootLock {
    file: File,
}

impl RootLock {
    fn acquire(root: &Path, timeout: Duration) -> Result<Self, SharedApplyFailureKind> {
        validate_destination_root(root).map_err(|_| SharedApplyFailureKind::RootChanged)?;
        let file = File::open(root).map_err(|_| SharedApplyFailureKind::RootChanged)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let deadline = Instant::now() + timeout;
            loop {
                let result =
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    return Ok(Self { file });
                }
                if Instant::now() >= deadline {
                    return Err(SharedApplyFailureKind::LockTimeout);
                }
                std::thread::sleep(LOCK_RETRY);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = file;
            Err(SharedApplyFailureKind::LockUnsupported)
        }
    }
}

impl Drop for RootLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

fn derive_status(entries: &[SharedApplyEntry], dry_run: bool) -> SharedApplyStatus {
    if dry_run {
        return SharedApplyStatus::DryRun;
    }
    let successes = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.outcome,
                SharedApplyOutcome::InstalledNew
                    | SharedApplyOutcome::ReplacedExisting
                    | SharedApplyOutcome::AlreadyInstalled
            )
        })
        .count();
    if successes == entries.len() {
        SharedApplyStatus::Success
    } else if successes > 0 {
        SharedApplyStatus::PartialFailure
    } else {
        SharedApplyStatus::Failed
    }
}

fn failed_entry(
    plan: &SharedPlanEntry,
    kind: SharedApplyFailureKind,
    detail: &str,
) -> SharedApplyEntry {
    fail_result(
        SharedApplyEntry {
            plan_entry: plan.clone(),
            destination_existed_before_apply: None,
            destination_parent_existed_before_apply: None,
            observed_source_digest: None,
            observed_destination_digest: None,
            backup_path: None,
            backup_digest: None,
            destination_pre_mode: None,
            temporary_path: None,
            final_destination_digest: None,
            created_directories: Vec::new(),
            replacement_approved: false,
            verification_succeeded: false,
            outcome: SharedApplyOutcome::SkippedNotEligible,
            publication: SharedPublicationState::NotPublished,
            stages: Vec::new(),
            warnings: Vec::new(),
            failures: Vec::new(),
        },
        SharedApplyOutcome::SkippedNotEligible,
        kind,
        None,
        detail,
    )
}

fn fail_result(
    mut result: SharedApplyEntry,
    outcome: SharedApplyOutcome,
    kind: SharedApplyFailureKind,
    path: Option<&Path>,
    detail: &str,
) -> SharedApplyEntry {
    result.outcome = outcome;
    result.failures.push(failure(kind, path, detail));
    result.stages.push(match outcome {
        SharedApplyOutcome::SourceChanged => SharedTransactionStage::SourceChanged,
        SharedApplyOutcome::DestinationChanged => SharedTransactionStage::DestinationChanged,
        SharedApplyOutcome::BackupFailed => SharedTransactionStage::BackupFailed,
        SharedApplyOutcome::VerificationFailed => SharedTransactionStage::VerificationFailed,
        SharedApplyOutcome::WriteFailed => SharedTransactionStage::WriteFailed,
        SharedApplyOutcome::SkippedReplacementNotApproved => {
            SharedTransactionStage::SkippedReplacementNotApproved
        }
        SharedApplyOutcome::SkippedConflict => SharedTransactionStage::SkippedConflict,
        _ => SharedTransactionStage::SkippedNotEligible,
    });
    result
}

fn failure(kind: SharedApplyFailureKind, path: Option<&Path>, detail: &str) -> SharedApplyFailure {
    SharedApplyFailure {
        kind,
        path: path.map(SharedTransactionPath::from_path),
        detail: detail.to_owned(),
    }
}

pub fn seal_shared_transaction_plan(
    plan: &mut SharedTransactionPlan,
) -> Result<(), SharedApplyFailure> {
    plan.plan_id.clear();
    plan.plan_id = plan_digest(plan)?;
    Ok(())
}

fn plan_digest(plan: &SharedTransactionPlan) -> Result<String, SharedApplyFailure> {
    let mut clone = plan.clone();
    clone.plan_id.clear();
    serde_json::to_vec(&clone)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|error| {
            failure(
                SharedApplyFailureKind::InvalidPlan,
                None,
                &error.to_string(),
            )
        })
}

fn rollback_preview_digest(preview: &SharedRollbackPreview) -> String {
    let mut clone = preview.clone();
    clone.preview_id.clear();
    serde_json::to_vec(&clone)
        .map(|bytes| digest_bytes(&bytes))
        .unwrap_or_default()
}

fn digest_text(text: &str) -> String {
    digest_bytes(text.as_bytes())
}

fn digest_bytes(bytes: &[u8]) -> String {
    hex_bytes(&Sha256::digest(bytes))
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn safe_identifier(value: &str) -> Result<String, SharedApplyFailureKind> {
    if value.is_empty()
        || value.len() > 128
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(SharedApplyFailureKind::InvalidPlan);
    }
    Ok(value.to_owned())
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn generate_shared_operation_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let nonce = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let material = format!("{now}:{}:{nonce}", std::process::id());
    format!("shared-{}", &digest_text(&material)[..32])
}

/// Fixture helpers shared by this module's test modules.
#[cfg(test)]
mod tests_support {
    use super::*;
    use crate::patch_manager::{
        PreviewIdentity, PreviewIdentityKind, PreviewIdentityState, PreviewMatchStrength,
        PreviewSourceItem, SharedPreviewRequest, build_shared_preview,
    };

    pub(super) struct Fixture(pub(super) PathBuf);

    impl Fixture {
        pub(super) fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "archivefs-shared-transaction-{label}-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        pub(super) fn source_root(&self) -> PathBuf {
            self.0.join("source")
        }

        pub(super) fn destination_root(&self) -> PathBuf {
            self.0.join("destination")
        }

        pub(super) fn history_root(&self) -> PathBuf {
            self.0.join("history")
        }

        pub(super) fn backup_root(&self) -> PathBuf {
            self.0.join("backups")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(super) fn preview(
        fixture: &Fixture,
        source_bytes: &[u8],
        existing: Option<&[u8]>,
    ) -> SharedPreviewReport {
        fs::create_dir(fixture.source_root()).unwrap();
        fs::create_dir(fixture.destination_root()).unwrap();
        let source = fixture.source_root().join("game.cht");
        fs::write(&source, source_bytes).unwrap();
        if let Some(bytes) = existing {
            let parent = fixture.destination_root().join("Nintendo - NES");
            fs::create_dir(&parent).unwrap();
            fs::write(parent.join("game.cht"), bytes).unwrap();
        }
        build_shared_preview(&SharedPreviewRequest {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: fixture.0.join("selected.zip"),
            platform: Some("NES".into()),
            identity: PreviewIdentity {
                kind: PreviewIdentityKind::RetroArchCatalogueMatch,
                state: PreviewIdentityState::Verified,
                value: Some("archive-1".into()),
                archive_path: fixture.0.join("selected.zip"),
                revision: None,
            },
            destination_root: fixture.destination_root(),
            source_items: vec![PreviewSourceItem {
                adapter: PreviewAdapter::RetroArch,
                source_path: source,
                expected_source_digest: Some(digest_bytes(source_bytes)),
                destination_relative_paths: vec![PathBuf::from("Nintendo - NES/game.cht")],
                match_strength: PreviewMatchStrength::VerifiedExact,
            }],
        })
        .unwrap()
    }

    pub(super) fn make_plan(
        fixture: &Fixture,
        report: &SharedPreviewReport,
    ) -> SharedTransactionPlan {
        build_shared_transaction_plan(
            report,
            "retroarch-native",
            "trusted-catalogue",
            &fixture.source_root(),
        )
        .unwrap()
    }

    pub(super) fn options(
        fixture: &Fixture,
        plan: &SharedTransactionPlan,
        operation: &str,
        dry_run: bool,
        general: bool,
        replacement: bool,
    ) -> SharedApplyOptions {
        SharedApplyOptions {
            dry_run,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: general,
                replacement_approved: replacement,
            }),
            operation_id: operation.into(),
            timestamp_unix_seconds: 1_700_000_000,
            current_context: plan.context.clone(),
            history_root: fixture.history_root(),
            backup_root: fixture.backup_root(),
        }
    }

    /// `count` new files `Nintendo - NES/game-N.cht` with bytes `new-N`.
    pub(super) fn multi_preview(fixture: &Fixture, count: usize) -> SharedPreviewReport {
        fs::create_dir(fixture.source_root()).unwrap();
        fs::create_dir(fixture.destination_root()).unwrap();
        let source_items = (0..count)
            .map(|index| {
                let source = fixture.source_root().join(format!("source-{index}.cht"));
                fs::write(&source, format!("new-{index}")).unwrap();
                PreviewSourceItem {
                    adapter: PreviewAdapter::RetroArch,
                    source_path: source,
                    expected_source_digest: None,
                    destination_relative_paths: vec![PathBuf::from(format!(
                        "Nintendo - NES/game-{index}.cht"
                    ))],
                    match_strength: PreviewMatchStrength::VerifiedExact,
                }
            })
            .collect();
        build_shared_preview(&SharedPreviewRequest {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: fixture.0.join("selected.zip"),
            platform: Some("NES".into()),
            identity: PreviewIdentity {
                kind: PreviewIdentityKind::RetroArchCatalogueMatch,
                state: PreviewIdentityState::Verified,
                value: Some("archive-1".into()),
                archive_path: fixture.0.join("selected.zip"),
                revision: None,
            },
            destination_root: fixture.destination_root(),
            source_items,
        })
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::patch_manager::{
        PreviewIdentity, PreviewIdentityKind, PreviewIdentityState, PreviewMatchStrength,
        PreviewSourceItem, SharedPreviewRequest, build_shared_preview,
    };

    #[test]
    fn interrupted_apply_is_discoverable_and_reconciles_without_auto_resume() {
        let fixture = Fixture::new("durable-interrupted-apply");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::TerminalJournal));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "durable-interrupted", false, true, false),
        );
        inject_fault(None);
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(result.journal_path.is_none());
        assert!(
            fixture
                .destination_root()
                .join("Nintendo - NES/game.cht")
                .exists()
        );
        let pending = discover_pending_operations(&fixture.history_root());
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].journal.transaction_state,
            SharedRecoveryState::Applying
        );
        assert_eq!(
            pending[0].reconciled_entries,
            vec![SharedEntryRecoveryState::Applied]
        );
        assert!(plan_pending_recovery(&pending[0]).can_safe_rollback);
        assert!(!plan_pending_recovery(&pending[0]).can_resume);
        assert!(
            discover_shared_apply_history(&fixture.history_root())
                .journals
                .is_empty()
        );
    }

    #[test]
    fn multi_file_interruption_after_second_entry_has_all_mutations_discoverable() {
        let fixture = Fixture::new("durable-multi-file");
        fs::create_dir(fixture.source_root()).unwrap();
        fs::create_dir(fixture.destination_root()).unwrap();
        let source_items = (0..3)
            .map(|index| {
                let source = fixture.source_root().join(format!("source-{index}.cht"));
                fs::write(&source, format!("new-{index}")).unwrap();
                PreviewSourceItem {
                    adapter: PreviewAdapter::RetroArch,
                    source_path: source,
                    expected_source_digest: None,
                    destination_relative_paths: vec![PathBuf::from(format!(
                        "Nintendo - NES/game-{index}.cht"
                    ))],
                    match_strength: PreviewMatchStrength::VerifiedExact,
                }
            })
            .collect();
        let report = build_shared_preview(&SharedPreviewRequest {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: fixture.0.join("selected.zip"),
            platform: Some("NES".into()),
            identity: PreviewIdentity {
                kind: PreviewIdentityKind::RetroArchCatalogueMatch,
                state: PreviewIdentityState::Verified,
                value: Some("archive-1".into()),
                archive_path: fixture.0.join("selected.zip"),
                revision: None,
            },
            destination_root: fixture.destination_root(),
            source_items,
        })
        .unwrap();
        let plan = make_plan(&fixture, &report);
        inject_fault_after(FaultPoint::ApplyCheckpoint, 1);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "durable-multi", false, true, false),
        );
        inject_fault(None);
        assert_eq!(
            result.journal_failure.unwrap().kind,
            SharedApplyFailureKind::JournalFailed
        );
        assert!(
            fixture
                .destination_root()
                .join("Nintendo - NES/game-0.cht")
                .exists()
        );
        assert!(
            fixture
                .destination_root()
                .join("Nintendo - NES/game-1.cht")
                .exists()
        );
        assert!(
            !fixture
                .destination_root()
                .join("Nintendo - NES/game-2.cht")
                .exists()
        );
        let pending = discover_pending_operations(&fixture.history_root());
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].reconciled_entries,
            vec![
                SharedEntryRecoveryState::Applied,
                SharedEntryRecoveryState::Applied,
                SharedEntryRecoveryState::Planned
            ]
        );
    }

    #[test]
    fn interrupted_rollback_is_discoverable_and_not_a_completed_receipt() {
        let fixture = Fixture::new("durable-interrupted-rollback");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "durable-rollback", false, true, true),
        );
        let journal_path = applied.journal_path.unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        inject_fault(Some(FaultPoint::RollbackAfterMutation));
        let result = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "durable-rollback-reverse".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        inject_fault(None);
        assert_eq!(result.status, SharedApplyStatus::PartialFailure);
        let pending = discover_pending_operations(&fixture.history_root());
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].journal.transaction_state,
            SharedRecoveryState::RollingBack
        );
        assert!(pending[0].journal.rollback_of_operation_id.is_some());
        assert!(
            fixture
                .history_root()
                .join("durable-rollback.json")
                .exists()
        );
    }

    #[test]
    fn legacy_final_receipt_is_not_reported_as_pending() {
        let fixture = Fixture::new("durable-legacy");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "legacy-receipt", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(discover_pending_operations(&fixture.history_root()).is_empty());
        assert_eq!(
            discover_shared_apply_history(&fixture.history_root())
                .journals
                .len(),
            1
        );
    }

    #[test]
    fn dry_run_and_missing_confirmation_write_nothing() {
        let fixture = Fixture::new("dry-run");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "dry-run", false, false, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::DryRun);
        assert!(
            !fixture
                .destination_root()
                .join("Nintendo - NES/game.cht")
                .exists()
        );
        assert!(!fixture.history_root().exists());
        assert!(!fixture.backup_root().exists());
    }

    #[test]
    fn install_new_is_atomic_journaled_and_rollback_is_bound_and_idempotent() {
        let fixture = Fixture::new("install");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "install-one", false, true, false),
        );
        let destination = fixture.destination_root().join("Nintendo - NES/game.cht");
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert_eq!(
            result.journal.entries[0].destination_existed_before_apply,
            Some(false)
        );
        assert_eq!(
            result.journal.entries[0].destination_parent_existed_before_apply,
            Some(false)
        );
        let journal_path = result.journal_path.unwrap();
        assert!(journal_path.exists());
        assert_eq!(
            discover_shared_apply_history(&fixture.history_root())
                .journals
                .len(),
            1
        );

        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(rollback.available);
        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "rollback-one".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        assert!(!destination.exists());
        assert!(!fixture.destination_root().join("Nintendo - NES").exists());
        let repeated = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(!repeated.available);
        assert_eq!(
            repeated.entries[0].outcome,
            SharedRollbackOutcome::AlreadyRolledBack
        );
    }

    #[test]
    fn rollback_reports_verification_failure_when_a_removed_new_file_reappears() {
        let fixture = Fixture::new("rollback-remove-verification");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "remove-verification", false, true, false),
        );
        let journal_path = result.journal_path.unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        inject_fault(Some(FaultPoint::RollbackRemovalVerification));
        let failed = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "remove-verification-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        inject_fault(None);
        assert_eq!(failed.status, SharedApplyStatus::Failed);
        assert_eq!(
            failed.preview.entries[0].outcome,
            SharedRollbackOutcome::Failed
        );
        assert_eq!(
            failed.preview.entries[0]
                .failure
                .as_ref()
                .map(|failure| failure.kind),
            Some(SharedApplyFailureKind::VerificationFailed)
        );
        assert!(failed.journal_path.is_none());
        assert!(
            fixture
                .destination_root()
                .join("Nintendo - NES/game.cht")
                .is_file()
        );
    }

    #[test]
    fn replacement_requires_permission_creates_verified_backup_and_restores_it() {
        let fixture = Fixture::new("replace");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        let denied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "replace-denied", false, true, false),
        );
        assert_eq!(
            denied.journal.entries[0].outcome,
            SharedApplyOutcome::SkippedReplacementNotApproved
        );
        assert_eq!(
            fs::read(fixture.destination_root().join("Nintendo - NES/game.cht")).unwrap(),
            b"old"
        );
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "replace-one", false, true, true),
        );
        let entry = &result.journal.entries[0];
        assert_eq!(entry.outcome, SharedApplyOutcome::ReplacedExisting);
        assert_eq!(entry.destination_existed_before_apply, Some(true));
        assert_eq!(entry.destination_parent_existed_before_apply, Some(true));
        let backup = entry.backup_path.as_ref().unwrap().to_path_buf().unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"old");
        let journal = result.journal_path.unwrap();
        let rollback = preview_shared_rollback(
            &journal,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(rollback.available);
        let outcome = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "restore-one".into(),
                timestamp_unix_seconds: 1_700_000_002,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(outcome.status, SharedApplyStatus::Success);
        assert_eq!(
            fs::read(fixture.destination_root().join("Nintendo - NES/game.cht")).unwrap(),
            b"old"
        );
        assert!(backup.exists(), "backup retention is deliberate");
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    fn confirm_rollback(
        fixture: &Fixture,
        preview: &SharedRollbackPreview,
        id: &str,
    ) -> SharedRollbackResult {
        execute_shared_rollback(
            preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: id.into(),
                timestamp_unix_seconds: 1_700_000_002,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        )
    }

    /// Job #22: replacing a user's file keeps its permission bits, and Undo
    /// puts back both its bytes and its bits.
    #[cfg(unix)]
    #[test]
    fn replacement_keeps_and_rollback_restores_existing_permission_bits() {
        for mode in [0o600, 0o644, 0o664] {
            let fixture = Fixture::new(&format!("mode-{mode:o}"));
            let report = preview(&fixture, b"new", Some(b"old"));
            let live = fixture.destination_root().join("Nintendo - NES/game.cht");
            chmod(&live, mode);
            let plan = make_plan(&fixture, &report);
            let result = execute_shared_apply(
                &plan,
                &options(
                    &fixture,
                    &plan,
                    &format!("mode-apply-{mode:o}"),
                    false,
                    true,
                    true,
                ),
            );
            let entry = &result.journal.entries[0];
            assert_eq!(
                entry.outcome,
                SharedApplyOutcome::ReplacedExisting,
                "{mode:o}"
            );
            assert_eq!(entry.destination_pre_mode, Some(mode));
            assert_eq!(fs::read(&live).unwrap(), b"new");
            assert_eq!(mode_of(&live), mode, "Apply changed {mode:o}");
            let rollback = preview_shared_rollback(
                result.journal_path.as_ref().unwrap(),
                &fixture.destination_root(),
                &fixture.backup_root(),
            );
            assert!(rollback.available);
            assert_eq!(
                rollback.entries[0].permissions,
                Some(SharedRollbackPermissions::Original { mode })
            );
            // A later user chmod of the installed file is not the original.
            chmod(&live, 0o640);
            let outcome = confirm_rollback(&fixture, &rollback, &format!("mode-undo-{mode:o}"));
            assert_eq!(outcome.status, SharedApplyStatus::Success);
            assert_eq!(fs::read(&live).unwrap(), b"old");
            assert_eq!(mode_of(&live), mode, "Undo did not restore {mode:o}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn newly_created_files_keep_the_owner_only_creation_policy() {
        let fixture = Fixture::new("mode-new");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "mode-new", false, true, false),
        );
        let entry = &result.journal.entries[0];
        assert_eq!(entry.outcome, SharedApplyOutcome::InstalledNew);
        assert_eq!(entry.destination_pre_mode, None);
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        assert_eq!(mode_of(&live), OWNER_ONLY_MODE);
        let rollback = preview_shared_rollback(
            result.journal_path.as_ref().unwrap(),
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert_eq!(rollback.entries[0].permissions, None);
    }

    /// A journal from before modes were recorded, or a mode EmuWiz will not
    /// reproduce, is restored owner-only and the preview says so.
    #[cfg(unix)]
    #[test]
    fn unknown_or_special_original_modes_are_reported_not_invented() {
        for (label, recorded, expected) in [
            ("legacy", None, SharedRollbackPermissions::NotRecorded),
            (
                "setgid",
                Some(0o2664),
                SharedRollbackPermissions::NotRestorable { mode: 0o2664 },
            ),
            (
                "unreadable",
                Some(0o200),
                SharedRollbackPermissions::NotRestorable { mode: 0o200 },
            ),
        ] {
            let fixture = Fixture::new(&format!("mode-{label}"));
            let report = preview(&fixture, b"new", Some(b"old"));
            let live = fixture.destination_root().join("Nintendo - NES/game.cht");
            chmod(&live, 0o664);
            let plan = make_plan(&fixture, &report);
            let result = execute_shared_apply(
                &plan,
                &options(
                    &fixture,
                    &plan,
                    &format!("mode-apply-{label}"),
                    false,
                    true,
                    true,
                ),
            );
            let journal_path = result.journal_path.clone().unwrap();
            // Rewrite the immutable journal to simulate the recorded evidence.
            let mut journal = read_journal(&journal_path).unwrap();
            journal.entries[0].destination_pre_mode = recorded;
            let text = serde_json::to_string_pretty(&journal).unwrap();
            if recorded.is_none() {
                assert!(
                    !text.contains("destination_pre_mode"),
                    "older journal shape"
                );
            }
            chmod(&journal_path, 0o600);
            fs::write(&journal_path, text).unwrap();
            let rollback = preview_shared_rollback(
                &journal_path,
                &fixture.destination_root(),
                &fixture.backup_root(),
            );
            assert!(rollback.available, "{label}");
            assert_eq!(rollback.entries[0].permissions, Some(expected), "{label}");
            assert!(expected.summary().contains("0600"), "{label}");
            let outcome = confirm_rollback(&fixture, &rollback, &format!("mode-undo-{label}"));
            assert_eq!(outcome.status, SharedApplyStatus::Success, "{label}");
            assert_eq!(fs::read(&live).unwrap(), b"old");
            assert_eq!(mode_of(&live), OWNER_ONLY_MODE, "{label}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn existing_special_mode_is_not_carried_into_a_replacement() {
        let fixture = Fixture::new("mode-special-apply");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o4755);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "mode-special", false, true, true),
        );
        let entry = &result.journal.entries[0];
        assert_eq!(entry.outcome, SharedApplyOutcome::ReplacedExisting);
        assert_eq!(entry.destination_pre_mode, Some(0o4755));
        assert_eq!(mode_of(&live), OWNER_ONLY_MODE);
        assert!(
            entry
                .warnings
                .iter()
                .any(|warning| warning.contains("4755")),
            "{:?}",
            entry.warnings
        );
    }

    /// A restore whose permission change fails leaves the installed file
    /// (bytes and bits) alone and is reported as a failure; a retry then
    /// restores both.
    #[cfg(unix)]
    #[test]
    fn failed_permission_restore_is_reported_and_retry_restores_bits() {
        let fixture = Fixture::new("mode-restore-fault");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o644);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "mode-fault", false, true, true),
        );
        let journal_path = result.journal_path.clone().unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        inject_fault(Some(FaultPoint::Permissions));
        let failed = confirm_rollback(&fixture, &rollback, "mode-fault-undo");
        inject_fault(None);
        assert_ne!(failed.status, SharedApplyStatus::Success);
        assert_eq!(
            failed.preview.entries[0].outcome,
            SharedRollbackOutcome::Failed
        );
        assert_eq!(fs::read(&live).unwrap(), b"new");
        assert_eq!(mode_of(&live), 0o644);
        let retry = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(retry.available);
        let outcome = confirm_rollback(&fixture, &retry, "mode-fault-retry");
        assert_eq!(outcome.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(mode_of(&live), 0o644);
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_replacement_keeps_mode_evidence_in_the_pending_journal() {
        let fixture = Fixture::new("mode-interrupted");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::TerminalJournal));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "mode-interrupted", false, true, true),
        );
        inject_fault(None);
        assert!(result.journal_path.is_none());
        assert_eq!(mode_of(&live), 0o664);
        let pending = discover_pending_operations(&fixture.history_root());
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].journal.entries[0].destination_pre_mode,
            Some(0o664)
        );
        assert!(!plan_pending_recovery(&pending[0]).can_resume);
    }

    /// Undo never follows a destination that became a symlink, so no
    /// permission bits are copied onto the link's target.
    #[cfg(unix)]
    #[test]
    fn rollback_through_a_swapped_symlink_is_refused_and_target_bits_untouched() {
        let fixture = Fixture::new("mode-symlink");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "mode-symlink", false, true, true),
        );
        let journal_path = result.journal_path.clone().unwrap();
        let outside = fixture.0.join("outside.cht");
        fs::write(&outside, b"new").unwrap();
        chmod(&outside, 0o600);
        fs::remove_file(&live).unwrap();
        std::os::unix::fs::symlink(&outside, &live).unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(!rollback.available);
        assert_eq!(
            rollback.entries[0].outcome,
            SharedRollbackOutcome::DestinationUnsafe
        );
        let refused = confirm_rollback(&fixture, &rollback, "mode-symlink-undo");
        assert_eq!(refused.status, SharedApplyStatus::Failed);
        assert_eq!(fs::read(&outside).unwrap(), b"new");
        assert_eq!(mode_of(&outside), 0o600);
        assert!(
            fs::symlink_metadata(&live)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    fn stray_temporaries(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".archivefs-apply-"))
            .collect()
    }

    #[cfg(unix)]
    fn ino_of(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(path).unwrap().ino()
    }

    /// Job #22: a permission change between observing the file (whose mode is
    /// recorded) and publishing the replacement is refused; the user's change
    /// stays and no stale mode is applied.
    #[cfg(unix)]
    #[test]
    fn apply_refuses_a_permission_change_between_observation_and_publication() {
        let fixture = Fixture::new("race-apply-chmod");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        set_before_publish(Some(Box::new(|destination: &Path| {
            chmod(destination, 0o640)
        })));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-chmod", false, true, true),
        );
        set_before_publish(None);
        let entry = &result.journal.entries[0];
        assert_eq!(entry.outcome, SharedApplyOutcome::DestinationChanged);
        assert!(
            entry
                .failures
                .iter()
                .any(|f| f.kind == SharedApplyFailureKind::DestinationChanged),
            "{:?}",
            entry.failures
        );
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(
            mode_of(&live),
            0o640,
            "the user's change is kept, not the stale 0664"
        );
        assert!(stray_temporaries(live.parent().unwrap()).is_empty());
    }

    /// The same file content under a different inode is a different file.
    #[cfg(unix)]
    #[test]
    fn apply_refuses_a_same_content_swap_between_observation_and_publication() {
        let fixture = Fixture::new("race-apply-swap");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        let swapped = std::rc::Rc::new(std::cell::Cell::new(0_u64));
        let seen = swapped.clone();
        set_before_publish(Some(Box::new(move |destination: &Path| {
            let other = destination.with_file_name("other.cht");
            fs::write(&other, b"old").unwrap();
            chmod(&other, 0o664);
            fs::rename(&other, destination).unwrap();
            seen.set(ino_of(destination));
        })));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-swap", false, true, true),
        );
        set_before_publish(None);
        let entry = &result.journal.entries[0];
        assert_ne!(entry.outcome, SharedApplyOutcome::ReplacedExisting);
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(
            ino_of(&live),
            swapped.get(),
            "the swapped-in file is put back untouched"
        );
        assert!(stray_temporaries(live.parent().unwrap()).is_empty());
    }

    /// Undo pins the installed file: a permission change before publication
    /// refuses the restore and leaves it as found; a fresh retry succeeds.
    #[cfg(unix)]
    #[test]
    fn rollback_refuses_a_change_to_the_installed_file_before_publication() {
        let fixture = Fixture::new("race-undo");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-undo", false, true, true),
        );
        let journal_path = result.journal_path.clone().unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(rollback.available);
        set_before_publish(Some(Box::new(|destination: &Path| {
            chmod(destination, 0o600)
        })));
        let refused = confirm_rollback(&fixture, &rollback, "race-undo-1");
        set_before_publish(None);
        assert_ne!(refused.status, SharedApplyStatus::Success);
        assert_eq!(
            refused.preview.entries[0].outcome,
            SharedRollbackOutcome::Failed
        );
        assert_eq!(
            refused.preview.entries[0].failure.as_ref().map(|f| f.kind),
            Some(SharedApplyFailureKind::DestinationChanged)
        );
        let pending = discover_pending_operations(&fixture.history_root());
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].journal.entries[0].state,
            SharedEntryRecoveryState::NeedsReview,
            "the refusal is recorded durably"
        );
        assert_eq!(fs::read(&live).unwrap(), b"new");
        assert_eq!(mode_of(&live), 0o600);
        assert!(stray_temporaries(live.parent().unwrap()).is_empty());
        let retry = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(retry.available);
        let outcome = confirm_rollback(&fixture, &retry, "race-undo-2");
        assert_eq!(outcome.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(mode_of(&live), 0o664);
    }

    #[cfg(unix)]
    #[test]
    fn automatic_restore_refuses_a_changed_installed_file() {
        let fixture = Fixture::new("race-verify-restore");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-verify", false, true, true),
        );
        let entry = &result.journal.entries[0];
        set_before_publish(Some(Box::new(|destination: &Path| {
            chmod(destination, 0o644)
        })));
        let restored = restore_after_failed_verification(
            &entry.plan_entry,
            entry,
            &live,
            entry.final_destination_digest.as_deref().unwrap(),
        );
        set_before_publish(None);
        assert_eq!(restored, Err(SharedApplyFailureKind::DestinationChanged));
        assert_eq!(fs::read(&live).unwrap(), b"new");
        assert_eq!(mode_of(&live), 0o644);
        assert!(stray_temporaries(live.parent().unwrap()).is_empty());
    }

    /// A filesystem that can't exchange names atomically refuses Apply and
    /// Undo of a replacement with that reason, never "destination changed",
    /// and changes nothing.
    #[cfg(unix)]
    #[test]
    fn filesystems_without_atomic_exchange_refuse_with_an_accurate_reason() {
        let fixture = Fixture::new("race-no-exchange");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::ExchangeUnsupported));
        let refused = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "no-exchange", false, true, true),
        );
        inject_fault(None);
        let entry = &refused.journal.entries[0];
        assert!(
            entry.failures.iter().any(|f| f.kind
                == SharedApplyFailureKind::AtomicReplaceUnsupported
                && f.detail.contains("cannot replace files atomically")),
            "{:?}",
            entry.failures
        );
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(mode_of(&live), 0o664);
        assert!(stray_temporaries(live.parent().unwrap()).is_empty());
        // Undo of a replacement on such a filesystem is refused the same way.
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "no-exchange-2", false, true, true),
        );
        let journal_path = applied.journal_path.clone().unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        inject_fault(Some(FaultPoint::ExchangeUnsupported));
        let undo = confirm_rollback(&fixture, &rollback, "no-exchange-undo");
        inject_fault(None);
        assert_eq!(
            undo.preview.entries[0].failure.as_ref().map(|f| f.kind),
            Some(SharedApplyFailureKind::AtomicReplaceUnsupported)
        );
        // Never reported as done; the journal stays and names the backup for
        // a manual restore.
        assert_eq!(undo.status, SharedApplyStatus::Failed);
        assert!(undo.journal_path.is_none(), "no rollback marker is written");
        let detail = &undo.preview.entries[0].failure.as_ref().unwrap().detail;
        let backup = applied.journal.entries[0]
            .backup_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        assert!(detail.contains("did not restore this file"), "{detail}");
        assert!(
            detail.contains("Nothing was changed: every file was kept as it is."),
            "{detail}"
        );
        assert!(detail.contains(&backup.display().to_string()), "{detail}");
        assert_eq!(fs::read(&backup).unwrap(), b"old");
        assert_eq!(fs::read(&live).unwrap(), b"new");
        assert!(stray_temporaries(live.parent().unwrap()).is_empty());
        assert!(journal_path.is_file());
        let again = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(again.available, "the change can still be undone later");
    }

    /// If the changed file can't be put back, it is kept (never deleted) and
    /// its location is named.
    #[cfg(unix)]
    #[test]
    fn a_displaced_file_that_cannot_be_put_back_is_kept_and_named() {
        let fixture = Fixture::new("race-keep");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        set_before_publish(Some(Box::new(|destination: &Path| {
            chmod(destination, 0o640)
        })));
        inject_fault(Some(FaultPoint::ReverseExchange));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-keep", false, true, true),
        );
        inject_fault(None);
        set_before_publish(None);
        let entry = &result.journal.entries[0];
        assert_ne!(entry.outcome, SharedApplyOutcome::ReplacedExisting);
        assert_eq!(entry.publication, SharedPublicationState::Uncertain);
        assert_eq!(
            shared_destination_outcome(&result, None),
            SharedDestinationOutcome::Uncertain
        );
        let failure = entry
            .failures
            .iter()
            .find(|f| f.kind == SharedApplyFailureKind::DisplacedFileKept)
            .expect("kept-file failure");
        let kept = entry
            .temporary_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        assert!(
            failure.detail.contains(&kept.display().to_string()),
            "{}",
            failure.detail
        );
        assert_eq!(fs::read(&kept).unwrap(), b"old");
        assert_eq!(mode_of(&kept), 0o640);
        assert_eq!(fs::read(&live).unwrap(), b"new");
        // Recovery would not call this settled while the kept file exists.
        assert!(stray_publication_file_beside(&live));
        // Nor can a later Undo pretend this entry needs nothing.
        let rollback = preview_shared_rollback(
            result.journal_path.as_ref().unwrap(),
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(!rollback.available);
        assert_eq!(
            rollback.entries[0].failure.as_ref().map(|f| f.kind),
            Some(SharedApplyFailureKind::DisplacedFileKept)
        );
        assert!(
            rollback.entries[0]
                .failure
                .as_ref()
                .unwrap()
                .detail
                .contains(&kept.display().to_string())
        );
    }

    /// A file renamed onto the destination while a refused exchange is being
    /// reversed comes back under the temporary name: it is kept, never
    /// deleted as if it were EmuWiz's own.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_file_arriving_during_the_reverse_exchange_is_kept() {
        let fixture = Fixture::new("race-arrival");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        set_before_publish(Some(Box::new(|destination: &Path| {
            chmod(destination, 0o640)
        })));
        set_before_reverse(Some(Box::new(|destination: &Path| {
            let arrival = destination.with_file_name("arrival.cht");
            fs::write(&arrival, b"arrival").unwrap();
            fs::rename(&arrival, destination).unwrap();
        })));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-arrival", false, true, true),
        );
        set_before_reverse(None);
        set_before_publish(None);
        let entry = &result.journal.entries[0];
        assert!(
            entry
                .failures
                .iter()
                .any(|f| f.kind == SharedApplyFailureKind::DisplacedFileKept),
            "{:?}",
            entry.failures
        );
        let kept = entry
            .temporary_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        assert_eq!(fs::read(&kept).unwrap(), b"arrival", "never deleted");
        // The kept file is the arrival, not EmuWiz's: the text says so.
        let detail = &entry
            .failures
            .iter()
            .find(|f| f.kind == SharedApplyFailureKind::DisplacedFileKept)
            .unwrap()
            .detail;
        assert!(detail.contains("a file EmuWiz did not write"), "{detail}");
        assert!(
            !detail.contains("EmuWiz's file is now in place"),
            "{detail}"
        );
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(mode_of(&live), 0o640);
    }

    /// Runs against whatever filesystem holds the temporary directory, with
    /// no fault injection: where the kernel offers RENAME_EXCHANGE the
    /// replacement keeps the file's bits; where it refuses (for example
    /// exFAT, ntfs3, many FUSE mounts - run with TMPDIR there) the outcome is
    /// AtomicReplaceUnsupported and the live file is untouched.
    #[cfg(unix)]
    #[test]
    fn replacement_on_the_real_filesystem_keeps_bits_or_refuses_untouched() {
        let fixture = Fixture::new("real-fs");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o644);
        let before = mode_of(&live);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "real-fs", false, true, true),
        );
        let entry = &result.journal.entries[0];
        if entry.outcome == SharedApplyOutcome::ReplacedExisting {
            assert_eq!(fs::read(&live).unwrap(), b"new");
            assert_eq!(mode_of(&live), before);
        } else {
            assert!(
                entry
                    .failures
                    .iter()
                    .any(|f| f.kind == SharedApplyFailureKind::AtomicReplaceUnsupported),
                "{:?}",
                entry.failures
            );
            assert_eq!(fs::read(&live).unwrap(), b"old");
            assert_eq!(mode_of(&live), before);
            assert!(stray_temporaries(live.parent().unwrap()).is_empty());
        }
    }

    /// A replacement or restore with no pinned file to displace - what a
    /// platform without file identity produces - is refused, never published
    /// with a plain rename.
    #[cfg(unix)]
    #[test]
    fn a_replacement_without_a_pinned_file_is_refused_not_renamed() {
        let fixture = Fixture::new("no-pin");
        fs::create_dir_all(fixture.destination_root()).unwrap();
        let live = fixture.destination_root().join("game.cht");
        fs::write(&live, b"old").unwrap();
        chmod(&live, 0o664);
        let expected = digest_bytes(b"new");
        let refused = atomic_write_bytes(b"new", &live, &expected, false, 0o664, None);
        assert_eq!(
            refused.map(|_| ()).map_err(|(kind, _)| kind),
            Err(SharedApplyFailureKind::AtomicReplaceUnsupported)
        );
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(mode_of(&live), 0o664);
        assert!(stray_temporaries(live.parent().unwrap()).is_empty());
    }

    /// Same size and the old mtime but different bytes is a changed file.
    #[cfg(unix)]
    #[test]
    fn a_content_rewrite_that_keeps_size_and_mtime_is_refused() {
        let fixture = Fixture::new("race-rewrite");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        set_before_publish(Some(Box::new(|destination: &Path| {
            let modified = fs::metadata(destination).unwrap().modified().unwrap();
            let file = OpenOptions::new().write(true).open(destination).unwrap();
            (&file).write_all(b"OLD").unwrap();
            file.set_modified(modified).unwrap();
        })));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-rewrite", false, true, true),
        );
        set_before_publish(None);
        assert_eq!(
            result.journal.entries[0].outcome,
            SharedApplyOutcome::DestinationChanged
        );
        assert_eq!(
            fs::read(&live).unwrap(),
            b"OLD",
            "the user's bytes are kept"
        );
        assert!(stray_temporaries(live.parent().unwrap()).is_empty());
    }

    /// A crash between exchange and verification leaves a hidden publication
    /// file; recovery reports the entry for review instead of settled.
    #[cfg(unix)]
    #[test]
    fn recovery_flags_a_stray_publication_file_for_review() {
        let fixture = Fixture::new("race-recovery");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::TerminalJournal));
        execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-recovery", false, true, true),
        );
        inject_fault(None);
        let settled = discover_pending_operations(&fixture.history_root());
        assert_eq!(
            settled[0].reconciled_entries,
            vec![SharedEntryRecoveryState::Applied]
        );
        fs::write(live.with_file_name(".archivefs-apply-999-0"), b"displaced").unwrap();
        let pending = discover_pending_operations(&fixture.history_root());
        assert_eq!(
            pending[0].reconciled_entries,
            vec![SharedEntryRecoveryState::NeedsReview]
        );
        assert!(!plan_pending_recovery(&pending[0]).can_safe_rollback);
    }

    /// `(relative path, existing bytes if any, new bytes)`.
    #[cfg(unix)]
    type MixedItem<'a> = (&'a str, Option<&'a [u8]>, &'a [u8]);

    /// A transaction mixing new and replaced files, in the given journal order.
    #[cfg(unix)]
    fn mixed_plan(fixture: &Fixture, items: &[MixedItem]) -> SharedTransactionPlan {
        fs::create_dir(fixture.source_root()).unwrap();
        fs::create_dir_all(fixture.destination_root().join("Nintendo - NES")).unwrap();
        let source_items = items
            .iter()
            .enumerate()
            .map(|(index, (relative, existing, new))| {
                let source = fixture.source_root().join(format!("source-{index}"));
                fs::write(&source, new).unwrap();
                if let Some(bytes) = existing {
                    let live = fixture.destination_root().join(relative);
                    fs::write(&live, bytes).unwrap();
                    chmod(&live, 0o664);
                }
                PreviewSourceItem {
                    adapter: PreviewAdapter::RetroArch,
                    source_path: source,
                    expected_source_digest: None,
                    destination_relative_paths: vec![PathBuf::from(relative)],
                    match_strength: PreviewMatchStrength::VerifiedExact,
                }
            })
            .collect();
        let report = build_shared_preview(&SharedPreviewRequest {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: fixture.0.join("selected.zip"),
            platform: Some("NES".into()),
            identity: PreviewIdentity {
                kind: PreviewIdentityKind::RetroArchCatalogueMatch,
                state: PreviewIdentityState::Verified,
                value: Some("archive-1".into()),
                archive_path: fixture.0.join("selected.zip"),
                revision: None,
            },
            destination_root: fixture.destination_root(),
            source_items,
        })
        .unwrap();
        make_plan(fixture, &report)
    }

    #[cfg(unix)]
    fn entry_for<'a>(
        preview: &'a SharedRollbackPreview,
        relative: &str,
    ) -> &'a SharedRollbackEntry {
        preview
            .entries
            .iter()
            .find(|entry| {
                entry
                    .destination
                    .as_ref()
                    .is_some_and(|path| path.display.ends_with(relative))
            })
            .unwrap()
    }

    /// Job #22 (exFAT): when the replaced file cannot be restored atomically,
    /// Undo stops before removing the new file that belongs with it, even
    /// though that file comes first in the journal; nothing changes, the
    /// result says so, and the journal and backup stay recoverable.
    #[cfg(unix)]
    #[test]
    fn undo_refused_restore_keeps_dependent_new_files_and_changes_nothing() {
        let fixture = Fixture::new("undo-ordering");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/game.cht", Some(b"old"), b"new"),
            ],
        );
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "undo-ordering", false, true, true),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        let journal_path = applied.journal_path.clone().unwrap();
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        let overlay = fixture
            .destination_root()
            .join("Nintendo - NES/aaa-overlay.cfg");
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        inject_fault(Some(FaultPoint::ExchangeUnsupported));
        let refused = confirm_rollback(&fixture, &rollback, "undo-ordering-1");
        inject_fault(None);
        assert_eq!(refused.status, SharedApplyStatus::Failed);
        assert!(refused.journal_path.is_none(), "never marked undone");
        let restore = entry_for(&refused.preview, "game.cht");
        assert_eq!(restore.outcome, SharedRollbackOutcome::Failed);
        let detail = &restore.failure.as_ref().unwrap().detail;
        assert_eq!(
            restore.failure.as_ref().unwrap().kind,
            SharedApplyFailureKind::AtomicReplaceUnsupported
        );
        assert!(detail.contains("did not restore this file"), "{detail}");
        assert!(
            detail.contains("Nothing was changed: every file was kept as it is."),
            "{detail}"
        );
        let kept = entry_for(&refused.preview, "overlay.cfg");
        assert_eq!(kept.outcome, SharedRollbackOutcome::Retained);
        assert!(
            kept.failure
                .as_ref()
                .unwrap()
                .detail
                .starts_with("Kept as it is")
        );
        // Nothing changed on disk; recovery records intact.
        assert_eq!(fs::read(&live).unwrap(), b"new");
        assert_eq!(fs::read(&overlay).unwrap(), b"overlay");
        let backup = applied
            .journal
            .entries
            .iter()
            .find_map(|entry| entry.backup_path.as_ref())
            .unwrap()
            .to_path_buf()
            .unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"old");
        assert!(journal_path.is_file());
        // On a filesystem that can swap, the same Undo still works later.
        let retry = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(retry.available);
        let done = confirm_rollback(&fixture, &retry, "undo-ordering-2");
        assert_eq!(done.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(mode_of(&live), 0o664);
        assert!(!overlay.exists());
    }

    /// A genuinely partial Undo: the restore was published but the installed
    /// file had been changed meanwhile and could not be put back, so it is
    /// kept aside. That is a change, so the text never says "nothing
    /// changed"; the dependent new file is still kept and the status is
    /// partial.
    #[cfg(unix)]
    #[test]
    fn partial_undo_reports_what_changed_and_keeps_dependent_files() {
        let fixture = Fixture::new("undo-partial");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/game.cht", Some(b"old"), b"new"),
            ],
        );
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "undo-partial", false, true, true),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        let journal_path = applied.journal_path.clone().unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        set_before_publish(Some(Box::new(|destination: &Path| {
            chmod(destination, 0o600)
        })));
        inject_fault(Some(FaultPoint::ReverseExchange));
        let partial = confirm_rollback(&fixture, &rollback, "undo-partial-1");
        inject_fault(None);
        set_before_publish(None);
        assert_eq!(partial.status, SharedApplyStatus::PartialFailure);
        assert!(partial.journal_path.is_none(), "never marked undone");
        let restore = entry_for(&partial.preview, "game.cht");
        assert_eq!(restore.outcome, SharedRollbackOutcome::Failed);
        let failure = restore.failure.as_ref().unwrap();
        assert_eq!(failure.kind, SharedApplyFailureKind::DisplacedFileKept);
        assert!(
            failure.detail.contains(
                "Files EmuWiz had already restored stay restored; every other file was kept as it is."
            ),
            "{}",
            failure.detail
        );
        assert!(
            !failure.detail.contains("Nothing was changed"),
            "{}",
            failure.detail
        );
        let dir = fixture.destination_root().join("Nintendo - NES");
        assert_eq!(fs::read(dir.join("game.cht")).unwrap(), b"old");
        assert_eq!(
            entry_for(&partial.preview, "overlay.cfg").outcome,
            SharedRollbackOutcome::Retained
        );
        assert_eq!(fs::read(dir.join("aaa-overlay.cfg")).unwrap(), b"overlay");
        // The user's changed file is kept, never deleted.
        let kept: Vec<_> = stray_temporaries(&dir);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(fs::read(dir.join(&kept[0])).unwrap(), b"new");
    }

    /// Job #22: one transaction replacing two existing files and adding one
    /// keeps a distinct backup for each replaced file (bytes and mode), and
    /// Undo restores both byte for byte and removes the new file.
    #[cfg(unix)]
    #[test]
    fn two_replaced_files_and_one_new_file_back_up_and_undo_exactly() {
        let fixture = Fixture::new("multi-backup");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/a.cht", Some(b"old-a"), b"new-a"),
                ("Nintendo - NES/b.cht", Some(b"old-b"), b"new-b"),
            ],
        );
        let dir = fixture.destination_root().join("Nintendo - NES");
        chmod(&dir.join("b.cht"), 0o644);
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "multi-backup", false, true, true),
        );
        assert_eq!(
            applied.journal.status,
            SharedApplyStatus::Success,
            "{:?}",
            applied.journal.entries
        );
        let backups: Vec<_> = applied
            .journal
            .entries
            .iter()
            .filter_map(|entry| entry.backup_path.as_ref())
            .map(|path| path.to_path_buf().unwrap())
            .collect();
        assert_eq!(backups.len(), 2);
        assert_ne!(backups[0], backups[1], "distinct backup identities");
        let mut contents: Vec<_> = backups.iter().map(|path| fs::read(path).unwrap()).collect();
        contents.sort();
        assert_eq!(contents, vec![b"old-a".to_vec(), b"old-b".to_vec()]);
        assert_eq!(fs::read(dir.join("a.cht")).unwrap(), b"new-a");
        assert_eq!(mode_of(&dir.join("a.cht")), 0o664);
        assert_eq!(mode_of(&dir.join("b.cht")), 0o644);
        let journal_path = applied.journal_path.clone().unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(rollback.available);
        let done = confirm_rollback(&fixture, &rollback, "multi-backup-undo");
        assert_eq!(done.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(dir.join("a.cht")).unwrap(), b"old-a");
        assert_eq!(fs::read(dir.join("b.cht")).unwrap(), b"old-b");
        assert_eq!(mode_of(&dir.join("a.cht")), 0o664);
        assert_eq!(mode_of(&dir.join("b.cht")), 0o644);
        assert!(!dir.join("aaa-overlay.cfg").exists());
        assert!(
            backups.iter().all(|path| path.is_file()),
            "backups are retained"
        );
    }

    /// An interrupted multi-replacement transaction keeps every backup valid
    /// and recovery sees all three entries applied.
    #[cfg(unix)]
    #[test]
    fn interrupted_multi_replacement_keeps_every_backup_recoverable() {
        let fixture = Fixture::new("multi-backup-interrupted");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/a.cht", Some(b"old-a"), b"new-a"),
                ("Nintendo - NES/b.cht", Some(b"old-b"), b"new-b"),
            ],
        );
        inject_fault(Some(FaultPoint::TerminalJournal));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "multi-interrupted", false, true, true),
        );
        inject_fault(None);
        assert!(result.journal_path.is_none());
        let pending = discover_pending_operations(&fixture.history_root());
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].reconciled_entries,
            vec![SharedEntryRecoveryState::Applied; 3]
        );
        let backups: Vec<_> = pending[0]
            .journal
            .entries
            .iter()
            .filter_map(|entry| entry.backup_path.as_ref())
            .collect();
        assert_eq!(backups.len(), 2);
        assert!(plan_pending_recovery(&pending[0]).can_safe_rollback);
    }

    /// A partial Apply among several replacements: the changed file is
    /// refused untouched, the other replacement and the new file apply, and
    /// Undo restores exactly what was changed.
    #[cfg(unix)]
    #[test]
    fn partial_multi_replacement_apply_undoes_exactly_what_changed() {
        let fixture = Fixture::new("multi-backup-partial");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/a.cht", Some(b"old-a"), b"new-a"),
                ("Nintendo - NES/b.cht", Some(b"old-b"), b"new-b"),
            ],
        );
        let dir = fixture.destination_root().join("Nintendo - NES");
        fs::write(dir.join("b.cht"), b"user-b").unwrap();
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "multi-partial", false, true, true),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::PartialFailure);
        assert_eq!(fs::read(dir.join("a.cht")).unwrap(), b"new-a");
        assert_eq!(fs::read(dir.join("b.cht")).unwrap(), b"user-b");
        let journal_path = applied.journal_path.clone().unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(rollback.available);
        let done = confirm_rollback(&fixture, &rollback, "multi-partial-undo");
        assert_eq!(done.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(dir.join("a.cht")).unwrap(), b"old-a");
        assert_eq!(fs::read(dir.join("b.cht")).unwrap(), b"user-b");
        assert!(!dir.join("aaa-overlay.cfg").exists());
    }

    /// With several replacements, a restore refused after another succeeded
    /// keeps the earlier one restored, keeps the rest, and says exactly that.
    #[cfg(unix)]
    #[test]
    fn undo_refused_after_an_earlier_restore_reports_what_stayed_restored() {
        let fixture = Fixture::new("multi-backup-undo-partial");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/a.cht", Some(b"old-a"), b"new-a"),
                ("Nintendo - NES/b.cht", Some(b"old-b"), b"new-b"),
            ],
        );
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "multi-undo-partial", false, true, true),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        let journal_path = applied.journal_path.clone().unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        inject_fault_after(FaultPoint::ExchangeUnsupported, 1);
        let partial = confirm_rollback(&fixture, &rollback, "multi-undo-partial-1");
        inject_fault(None);
        assert_eq!(partial.status, SharedApplyStatus::PartialFailure);
        let dir = fixture.destination_root().join("Nintendo - NES");
        let restored: Vec<_> = ["a.cht", "b.cht"]
            .iter()
            .filter(|name| {
                entry_for(&partial.preview, name).outcome == SharedRollbackOutcome::RestoredBackup
            })
            .collect();
        assert_eq!(restored.len(), 1, "exactly one restore completed");
        let refused_name = if *restored[0] == "a.cht" {
            "b.cht"
        } else {
            "a.cht"
        };
        let refused = entry_for(&partial.preview, refused_name);
        assert_eq!(refused.outcome, SharedRollbackOutcome::Failed);
        let detail = &refused.failure.as_ref().unwrap().detail;
        assert!(
            detail.contains("Files EmuWiz had already restored stay restored; every other file was kept as it is."),
            "{detail}"
        );
        assert!(!detail.contains("Nothing was changed"), "{detail}");
        assert_eq!(
            entry_for(&partial.preview, "aaa-overlay.cfg").outcome,
            SharedRollbackOutcome::Retained
        );
        assert!(dir.join("aaa-overlay.cfg").exists());
        let restored_name = restored[0];
        let old = if *restored_name == "a.cht" {
            b"old-a"
        } else {
            b"old-b"
        };
        assert_eq!(fs::read(dir.join(restored_name)).unwrap(), old);
        // A retry finishes the rest: the restored file counts as done.
        let retry = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(retry.available, "{:?}", retry.entries);
        assert_eq!(
            entry_for(&retry, restored_name).outcome,
            SharedRollbackOutcome::NoChangeRequired
        );
        let done = confirm_rollback(&fixture, &retry, "multi-undo-partial-2");
        assert_eq!(done.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(dir.join("a.cht")).unwrap(), b"old-a");
        assert_eq!(fs::read(dir.join("b.cht")).unwrap(), b"old-b");
        assert!(!dir.join("aaa-overlay.cfg").exists());
    }

    /// An Apply whose replacement was changed straight after it was written
    /// blocks Undo (the original is only in the backup) instead of letting
    /// the other entries roll back and call the change undone.
    #[cfg(unix)]
    #[test]
    fn apply_changed_after_publication_blocks_undo_and_names_the_backup() {
        let fixture = Fixture::new("apply-after-publish");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/game.cht", Some(b"old"), b"new"),
            ],
        );
        set_after_publish(Some(Box::new(|destination: &Path| {
            if destination.ends_with("game.cht") {
                fs::write(destination, b"edited").unwrap();
            }
        })));
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "apply-after-publish", false, true, true),
        );
        set_after_publish(None);
        assert_eq!(applied.journal.status, SharedApplyStatus::PartialFailure);
        let replaced = applied
            .journal
            .entries
            .iter()
            .find(|entry| entry.backup_path.is_some())
            .unwrap();
        assert!(
            replaced
                .failures
                .iter()
                .any(|failure| failure.kind == SharedApplyFailureKind::PublishedUnverified)
        );
        assert_eq!(
            replaced.publication,
            SharedPublicationState::PublishedThenFailed
        );
        assert_eq!(
            shared_destination_outcome(&applied, None),
            SharedDestinationOutcome::PublishedNeedsRecovery
        );
        let rollback = preview_shared_rollback(
            applied.journal_path.as_ref().unwrap(),
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(
            !rollback.available,
            "never rolled back as if nothing remained"
        );
        let blocked = entry_for(&rollback, "game.cht");
        let failure = blocked.failure.as_ref().unwrap();
        assert_eq!(failure.kind, SharedApplyFailureKind::PublishedUnverified);
        let backup = replaced.backup_path.as_ref().unwrap().display.clone();
        assert!(failure.detail.contains(&backup), "{}", failure.detail);
        assert_eq!(fs::read(&backup).unwrap(), b"old");
    }

    /// A restore that published but whose destination changed straight
    /// afterwards is a change: never "Nothing was changed", status partial,
    /// and the dependent new file is kept.
    #[cfg(unix)]
    #[test]
    fn undo_restore_changed_after_publication_is_reported_as_a_change() {
        let fixture = Fixture::new("undo-after-publish");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/game.cht", Some(b"old"), b"new"),
            ],
        );
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "undo-after-publish", false, true, true),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        let journal_path = applied.journal_path.clone().unwrap();
        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        set_after_publish(Some(Box::new(|destination: &Path| {
            fs::write(destination, b"edited").unwrap();
        })));
        let result = confirm_rollback(&fixture, &rollback, "undo-after-publish-1");
        set_after_publish(None);
        assert_eq!(result.status, SharedApplyStatus::PartialFailure);
        let restore = entry_for(&result.preview, "game.cht");
        let failure = restore.failure.as_ref().unwrap();
        assert_eq!(failure.kind, SharedApplyFailureKind::PublishedUnverified);
        assert!(
            failure
                .detail
                .contains("EmuWiz restored this file, but it changed straight afterwards"),
            "{}",
            failure.detail
        );
        assert!(
            !failure.detail.contains("Nothing was changed"),
            "{}",
            failure.detail
        );
        assert_eq!(
            entry_for(&result.preview, "aaa-overlay.cfg").outcome,
            SharedRollbackOutcome::Retained
        );
        let dir = fixture.destination_root().join("Nintendo - NES");
        assert!(dir.join("aaa-overlay.cfg").exists());
        assert_eq!(fs::read(dir.join("game.cht")).unwrap(), b"edited");
    }

    /// The operation's backup folder is reused only when it is a real
    /// directory, and an existing backup file is never overwritten.
    #[cfg(unix)]
    #[test]
    fn backup_folder_link_and_backup_name_collision_are_refused() {
        let fixture = Fixture::new("multi-backup-unsafe");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        let plan = make_plan(&fixture, &report);
        // A link at the operation's backup folder name.
        fs::create_dir_all(fixture.backup_root()).unwrap();
        let elsewhere = fixture.0.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, fixture.backup_root().join("linked-op")).unwrap();
        let refused = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "linked-op", false, true, true),
        );
        assert_ne!(
            refused.journal.entries[0].outcome,
            SharedApplyOutcome::ReplacedExisting
        );
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert!(
            fs::read_dir(&elsewhere).unwrap().next().is_none(),
            "nothing written through the link"
        );
        // A file already at this destination's backup name.
        let folder = fixture.backup_root().join("collide-op");
        fs::create_dir(&folder).unwrap();
        let taken = folder.join(format!("{}.bak", digest_text(&live.to_string_lossy())));
        fs::write(&taken, b"someone else's backup").unwrap();
        let refused = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "collide-op", false, true, true),
        );
        assert_ne!(
            refused.journal.entries[0].outcome,
            SharedApplyOutcome::ReplacedExisting
        );
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(fs::read(&taken).unwrap(), b"someone else's backup");
    }

    /// Apply's own automatic restore after a failed content check uses the
    /// recorded mode too.
    #[cfg(unix)]
    #[test]
    fn restore_after_failed_verification_restores_recorded_bits() {
        let fixture = Fixture::new("mode-verify-restore");
        let report = preview(&fixture, b"new", Some(b"old"));
        let live = fixture.destination_root().join("Nintendo - NES/game.cht");
        chmod(&live, 0o664);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "mode-verify", false, true, true),
        );
        let entry = &result.journal.entries[0];
        restore_after_failed_verification(
            &entry.plan_entry,
            entry,
            &live,
            entry.final_destination_digest.as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(fs::read(&live).unwrap(), b"old");
        assert_eq!(mode_of(&live), 0o664);
    }

    #[test]
    fn stale_plan_source_and_destination_changes_fail_closed() {
        let fixture = Fixture::new("stale");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        let mut stale = options(&fixture, &plan, "stale-context", false, true, true);
        stale.current_context.profile_id = "other-profile".into();
        assert_eq!(
            execute_shared_apply(&plan, &stale).journal.status,
            SharedApplyStatus::Failed
        );
        fs::write(fixture.source_root().join("game.cht"), b"changed").unwrap();
        let source_changed = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "source-changed", false, true, true),
        );
        assert_eq!(
            source_changed.journal.entries[0].outcome,
            SharedApplyOutcome::SourceChanged
        );
        fs::write(fixture.source_root().join("game.cht"), b"new").unwrap();
        fs::write(
            fixture.destination_root().join("Nintendo - NES/game.cht"),
            b"user-change",
        )
        .unwrap();
        let destination_changed = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "destination-changed", false, true, true),
        );
        assert_eq!(
            destination_changed.journal.entries[0].outcome,
            SharedApplyOutcome::DestinationChanged
        );
    }

    #[test]
    fn journal_failure_after_write_is_truthful_partial_success_and_temp_is_clean() {
        let fixture = Fixture::new("journal-failure");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let options = options(&fixture, &plan, "partial", false, true, false);
        inject_fault(Some(FaultPoint::JournalWrite));
        let result = execute_shared_apply(&plan, &options);
        inject_fault(None);
        assert_eq!(result.journal.status, SharedApplyStatus::PartialFailure);
        assert!(result.journal_failure.is_some());
        assert_eq!(
            fs::read(fixture.destination_root().join("Nintendo - NES/game.cht")).unwrap(),
            b"new"
        );
        assert!(
            fs::read_dir(fixture.destination_root().join("Nintendo - NES"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".archivefs-"))
        );
    }

    #[test]
    fn malformed_history_is_bounded_and_rollback_blocks_user_changes_and_bad_backup() {
        let fixture = Fixture::new("history");
        fs::create_dir(fixture.history_root()).unwrap();
        fs::write(fixture.history_root().join("bad.json"), b"{").unwrap();
        let history = discover_shared_apply_history(&fixture.history_root());
        assert!(history.journals.is_empty());
        assert_eq!(history.warnings.len(), 1);

        let fixture = Fixture::new("rollback-blocks");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "replace-block", false, true, true),
        );
        let journal = applied.journal_path.unwrap();
        fs::write(
            fixture.destination_root().join("Nintendo - NES/game.cht"),
            b"user",
        )
        .unwrap();
        let changed = preview_shared_rollback(
            &journal,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(!changed.available);
        assert_eq!(
            changed.entries[0].outcome,
            SharedRollbackOutcome::DestinationChanged
        );
    }

    #[test]
    fn unsupported_adapters_duplicate_paths_limits_and_lock_contention_fail_closed() {
        assert_eq!(
            adapter_write_support(PreviewAdapter::Pcsx2),
            SharedAdapterWriteSupport::ApplyAndRollback
        );
        assert_eq!(
            adapter_write_support(PreviewAdapter::Dolphin),
            SharedAdapterWriteSupport::ApplyAndRollback,
            "Dolphin gained apply/rollback support in the GameCube/Gecko adapter milestone"
        );
        let fixture = Fixture::new("lock");
        fs::create_dir(fixture.destination_root()).unwrap();
        let _first =
            RootLock::acquire(&fixture.destination_root(), Duration::from_millis(20)).unwrap();
        assert_eq!(
            RootLock::acquire(&fixture.destination_root(), Duration::from_millis(20)).unwrap_err(),
            SharedApplyFailureKind::LockTimeout
        );
    }

    /// The destination is replaced, then its durable checkpoint fails. The
    /// engine records the publication, so callers never parse the detail.
    #[test]
    fn checkpoint_failure_after_replace_records_published_then_failed() {
        let fixture = Fixture::new("replaced-checkpoint");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::ReplacedCheckpoint));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "replaced-checkpoint", false, true, true),
        );
        inject_fault(None);
        let entry = &result.journal.entries[0];
        assert_eq!(entry.outcome, SharedApplyOutcome::WriteFailed);
        assert_eq!(
            entry.publication,
            SharedPublicationState::PublishedThenFailed
        );
        assert_eq!(
            fs::read(fixture.destination_root().join("Nintendo - NES/game.cht")).unwrap(),
            b"new",
            "the pack's file is live"
        );
        assert_eq!(
            shared_entry_destination(entry, None),
            SharedEntryDestination::NeedsRecovery
        );
        assert_eq!(
            shared_destination_outcome(&result, None),
            SharedDestinationOutcome::PublishedNeedsRecovery
        );
        assert!(!shared_rollback_can_reverse(entry));
        // Rewording the failure detail cannot change the classification.
        let mut reworded = SharedApplyResult {
            journal: result.journal.clone(),
            journal_path: result.journal_path.clone(),
            journal_failure: None,
        };
        for failure in &mut reworded.journal.entries[0].failures {
            failure.detail = "something else entirely".into();
        }
        assert_eq!(
            shared_destination_outcome(&reworded, None),
            SharedDestinationOutcome::PublishedNeedsRecovery
        );
    }

    /// Published, failed its check, previous file restored: reversed, never
    /// "nothing published".
    #[test]
    fn a_restore_after_a_failed_check_is_reversed_not_nothing_published() {
        let fixture = Fixture::new("restored-after-check");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::Rename));
        let mut result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "restored-after-check", false, true, true),
        );
        inject_fault(None);
        result.journal.entries[0].outcome = SharedApplyOutcome::VerificationFailed;
        result.journal.entries[0].publication = SharedPublicationState::PublishedThenRestored;
        assert_eq!(
            shared_entry_destination(&result.journal.entries[0], None),
            SharedEntryDestination::RestoredAfterFailedCheck
        );
        assert_eq!(
            shared_destination_outcome(&result, None),
            SharedDestinationOutcome::Reversed
        );
    }

    /// A completed install whose rollback fails stays published.
    #[test]
    fn failed_rollback_leaves_a_published_entry_needing_recovery() {
        let fixture = Fixture::new("failed-rollback-publication");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "failed-rollback-pub", false, true, true),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        assert_eq!(
            applied.journal.entries[0].publication,
            SharedPublicationState::Published
        );
        assert_eq!(
            shared_destination_outcome(&applied, None),
            SharedDestinationOutcome::DurableSuccess
        );
        let preview = preview_shared_rollback(
            applied.journal_path.as_ref().unwrap(),
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(preview.available);
        inject_fault(Some(FaultPoint::RollbackRestore));
        let rollback = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "failed-rollback-pub-undo".into(),
                timestamp_unix_seconds: 1,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        inject_fault(None);
        assert_ne!(rollback.status, SharedApplyStatus::Success);
        assert_eq!(
            shared_destination_outcome(&applied, Some(&rollback)),
            SharedDestinationOutcome::PublishedNeedsRecovery
        );
    }

    /// Journals written before publication was recorded: a failed entry is
    /// uncertain, never "nothing written"; refusals before any write and
    /// completed entries keep their meaning.
    #[test]
    fn journals_without_a_publication_record_fail_closed() {
        let fixture = Fixture::new("legacy-publication");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::Rename));
        let mut result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "legacy-publication", false, true, false),
        );
        inject_fault(None);
        let mut json = serde_json::to_value(&result.journal.entries[0]).unwrap();
        json.as_object_mut().unwrap().remove("publication");
        let legacy: SharedApplyEntry = serde_json::from_value(json).unwrap();
        assert_eq!(legacy.publication, SharedPublicationState::NotRecorded);
        result.journal.entries[0] = legacy;
        assert_eq!(
            shared_destination_outcome(&result, None),
            SharedDestinationOutcome::Uncertain
        );
        result.journal.entries[0].outcome = SharedApplyOutcome::SourceChanged;
        assert_eq!(
            shared_destination_outcome(&result, None),
            SharedDestinationOutcome::NothingPublished
        );
    }

    #[test]
    fn injected_apply_failures_preserve_atomicity_and_truthful_state() {
        for fault in [
            FaultPoint::TemporaryWrite,
            FaultPoint::Flush,
            FaultPoint::Rename,
            FaultPoint::Verification,
            FaultPoint::ParentCreationRace,
            FaultPoint::SourceMutation,
            FaultPoint::DestinationMutation,
        ] {
            let fixture = Fixture::new(&format!("fault-{fault:?}"));
            let report = preview(&fixture, b"new", None);
            let plan = make_plan(&fixture, &report);
            inject_fault(Some(fault));
            let result = execute_shared_apply(
                &plan,
                &options(&fixture, &plan, "fault-run", false, true, false),
            );
            inject_fault(None);
            assert_ne!(result.journal.status, SharedApplyStatus::Success);
            assert!(
                !fixture
                    .destination_root()
                    .join("Nintendo - NES/game.cht")
                    .exists()
            );
            // Failures before publication are proven, not inferred.
            assert_eq!(
                result.journal.entries[0].publication,
                SharedPublicationState::NotPublished,
                "{fault:?}"
            );
            assert_eq!(
                shared_destination_outcome(&result, None),
                SharedDestinationOutcome::NothingPublished,
                "{fault:?}"
            );
        }

        let fixture = Fixture::new("backup-fault");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::BackupWrite));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "backup-fault", false, true, true),
        );
        inject_fault(None);
        assert_eq!(
            result.journal.entries[0].outcome,
            SharedApplyOutcome::BackupFailed
        );
        assert_eq!(
            fs::read(fixture.destination_root().join("Nintendo - NES/game.cht")).unwrap(),
            b"old"
        );

        let fixture = Fixture::new("journal-injected");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::JournalWrite));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "journal-fault", false, true, false),
        );
        inject_fault(None);
        assert_eq!(result.journal.status, SharedApplyStatus::PartialFailure);
        assert_eq!(
            result.journal_failure.as_ref().map(|failure| failure.kind),
            Some(SharedApplyFailureKind::JournalFailed)
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_round_trip_and_symlink_source_is_never_plannable() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;
        let path = PathBuf::from(OsString::from_vec(vec![b'/', b't', b'm', b'/', 0xff]));
        assert_eq!(
            SharedTransactionPath::from_path(&path)
                .to_path_buf()
                .unwrap(),
            path
        );

        let fixture = Fixture::new("symlink");
        fs::create_dir(fixture.source_root()).unwrap();
        fs::create_dir(fixture.destination_root()).unwrap();
        fs::write(fixture.0.join("outside"), b"new").unwrap();
        symlink(
            fixture.0.join("outside"),
            fixture.source_root().join("game.cht"),
        )
        .unwrap();
        let report = build_shared_preview(&SharedPreviewRequest {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: fixture.0.join("selected.zip"),
            platform: Some("NES".into()),
            identity: PreviewIdentity {
                kind: PreviewIdentityKind::RetroArchCatalogueMatch,
                state: PreviewIdentityState::Verified,
                value: Some("archive-1".into()),
                archive_path: fixture.0.join("selected.zip"),
                revision: None,
            },
            destination_root: fixture.destination_root(),
            source_items: vec![PreviewSourceItem {
                adapter: PreviewAdapter::RetroArch,
                source_path: fixture.source_root().join("game.cht"),
                expected_source_digest: None,
                destination_relative_paths: vec![PathBuf::from("Nintendo - NES/game.cht")],
                match_strength: PreviewMatchStrength::VerifiedExact,
            }],
        })
        .unwrap();
        assert!(
            build_shared_transaction_plan(&report, "profile", "trusted", &fixture.source_root())
                .is_err()
        );
    }

    // -----------------------------------------------------------------
    // Missing destination-root bootstrap (descriptor-anchored)
    // -----------------------------------------------------------------
    //
    // Every test below drives the real entry points
    // (`execute_shared_apply`/`execute_shared_rollback`) rather than
    // calling `bootstrap_missing_destination_root`/`remove_verified_root_chain`
    // directly, except where a test is specifically about rejecting a
    // hand-crafted, tampered journal - the one place going through
    // `execute_shared_rollback` and going around it via `read_journal` on a
    // hand-edited file both exercise the real, persisted-input code path.

    /// Like [`preview`], but never pre-creates `destination_root` (or
    /// anything below it) - callers decide exactly how much of the chain
    /// exists before building the preview/plan, so these tests can exercise
    /// a completely missing root, a partially missing chain, or (by
    /// pre-creating everything themselves) an already-existing root.
    fn preview_at_root(
        fixture: &Fixture,
        destination_root: PathBuf,
        source_bytes: &[u8],
    ) -> SharedPreviewReport {
        fs::create_dir(fixture.source_root()).unwrap();
        let source = fixture.source_root().join("game.cht");
        fs::write(&source, source_bytes).unwrap();
        build_shared_preview(&SharedPreviewRequest {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: fixture.0.join("selected.zip"),
            platform: Some("NES".into()),
            identity: PreviewIdentity {
                kind: PreviewIdentityKind::RetroArchCatalogueMatch,
                state: PreviewIdentityState::Verified,
                value: Some("archive-1".into()),
                archive_path: fixture.0.join("selected.zip"),
                revision: None,
            },
            destination_root,
            source_items: vec![PreviewSourceItem {
                adapter: PreviewAdapter::RetroArch,
                source_path: source,
                expected_source_digest: Some(digest_bytes(source_bytes)),
                destination_relative_paths: vec![PathBuf::from("Nintendo - NES/game.cht")],
                match_strength: PreviewMatchStrength::VerifiedExact,
            }],
        })
        .unwrap()
    }

    fn created_root_paths(journal: &SharedApplyJournal) -> Vec<PathBuf> {
        journal
            .created_root_directories
            .iter()
            .map(|entry| entry.path.to_path_buf().unwrap())
            .collect()
    }

    // --- existing-root transaction unchanged ------------------------------

    #[test]
    fn existing_destination_root_bootstraps_nothing() {
        let fixture = Fixture::new("bootstrap-existing-root");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "existing-root", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(result.journal.created_root_directories.is_empty());
        assert!(
            fixture
                .destination_root()
                .join("Nintendo - NES/game.cht")
                .is_file()
        );
    }

    // --- full missing-root create/apply/journal/rollback -------------------

    #[test]
    fn completely_missing_destination_root_is_safely_created_applied_and_rolled_back() {
        let fixture = Fixture::new("bootstrap-fully-missing");
        // Mirrors the motivating Dolphin case: `<profile>/Load/Textures`,
        // none of which exists yet.
        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        assert!(!root.exists());
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "fully-missing", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(root.is_dir());
        assert!(root.join("Nintendo - NES/game.cht").is_file());
        assert_eq!(
            fs::read(root.join("Nintendo - NES/game.cht")).unwrap(),
            b"new"
        );

        // Recorded outermost-first, each with a real identity, and
        // persisted to disk (not only held in memory).
        assert_eq!(
            created_root_paths(&result.journal),
            vec![
                fixture.0.join("dolphin"),
                fixture.0.join("dolphin").join("Load"),
                root.clone(),
            ]
        );
        assert!(
            result
                .journal
                .created_root_directories
                .iter()
                .all(|entry| entry.identity.is_some())
        );
        let journal_path = result.journal_path.unwrap();
        let persisted = read_journal(&journal_path).unwrap();
        assert_eq!(persisted.created_root_directories.len(), 3);

        let rollback = preview_shared_rollback(&journal_path, &root, &fixture.backup_root());
        assert!(rollback.available);
        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "rollback-removes-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        // The destination root, which this Undo was opened for, is removed.
        // The folders the install created above it are outside what Undo
        // may remove on a journal's word, and are left, empty.
        assert!(!root.exists());
        let load = fixture.0.join("dolphin").join("Load");
        assert!(load.is_dir());
        assert_eq!(fs::read_dir(&load).unwrap().count(), 0);
        assert!(fixture.0.join("dolphin").is_dir());
    }

    #[test]
    fn partially_missing_root_chain_creates_only_the_missing_levels() {
        let fixture = Fixture::new("bootstrap-partial");
        let dolphin = fixture.0.join("dolphin");
        fs::create_dir(&dolphin).unwrap();
        let root = dolphin.join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "partial-missing", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(root.is_dir());
        assert_eq!(
            created_root_paths(&result.journal),
            vec![dolphin.join("Load"), root.clone()]
        );
    }

    #[test]
    fn rollback_leaves_a_pre_existing_root_untouched() {
        let fixture = Fixture::new("bootstrap-rollback-preserves-preexisting");
        let dolphin = fixture.0.join("dolphin");
        fs::create_dir(&dolphin).unwrap();
        let root = dolphin.join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "rollback-preserves", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        let journal_path = result.journal_path.unwrap();
        let rollback = preview_shared_rollback(&journal_path, &root, &fixture.backup_root());
        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "rollback-preserves-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        assert!(!root.exists(), "the bootstrap-created leaf must be removed");
        assert!(
            dolphin.join("Load").is_dir(),
            "a folder above the destination root is left in place, even one the install made"
        );
        assert!(
            dolphin.is_dir(),
            "a directory that existed before the transaction must never be removed by rollback"
        );
    }

    #[test]
    fn rollback_stops_at_a_non_empty_transaction_created_directory() {
        let fixture = Fixture::new("bootstrap-rollback-stop-nonempty");
        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(
                &fixture,
                &plan,
                "rollback-stop-nonempty",
                false,
                true,
                false,
            ),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        let journal_path = result.journal_path.unwrap();
        let rollback = preview_shared_rollback(&journal_path, &root, &fixture.backup_root());

        // The installed file's own removal reports failure and leaves the
        // file (and so its "Nintendo - NES" parent directory) genuinely
        // present - the bootstrap-created `Textures` root is therefore
        // non-empty by the time root cleanup would run.
        inject_fault(Some(FaultPoint::RollbackRemovalVerification));
        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "rollback-stop-nonempty-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        inject_fault(None);
        assert_eq!(rolled_back.status, SharedApplyStatus::Failed);
        assert!(
            root.join("Nintendo - NES/game.cht").is_file(),
            "the file itself was never actually removed"
        );
        assert!(root.is_dir(), "a non-empty root must never be removed");
        assert!(
            fixture.0.join("dolphin").join("Load").is_dir(),
            "a shallower level must never be removed once a deeper one stopped cleanup"
        );
    }

    // --- created root replaced by a user-created directory before rollback -

    #[test]
    fn a_created_root_replaced_by_a_user_created_directory_is_not_deleted() {
        let fixture = Fixture::new("bootstrap-user-replaced-directory");
        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "user-replaced-dir", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        let journal_path = result.journal_path.unwrap();
        let rollback = preview_shared_rollback(&journal_path, &root, &fixture.backup_root());

        // The user wipes and recreates `Textures` themselves (their own
        // new, empty directory - same path, different inode) before
        // rollback ever runs. This also removes the installed file, so the
        // per-file rollback step itself cannot succeed either - the point
        // of this test is specifically that root cleanup, which runs
        // regardless of the per-file outcome, still refuses to touch the
        // replacement.
        fs::remove_dir_all(&root).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("user-file.txt"), b"do not touch").unwrap();

        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "user-replaced-dir-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        let _ = rolled_back;
        assert!(
            root.is_dir(),
            "the user's replacement directory must never be removed"
        );
        assert!(
            root.join("user-file.txt").is_file(),
            "its contents must be completely untouched"
        );
    }

    // --- created root replaced by a file -----------------------------------

    #[test]
    fn a_created_root_replaced_by_a_file_is_blocked() {
        let fixture = Fixture::new("bootstrap-user-replaced-file");
        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "user-replaced-file", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        let journal_path = result.journal_path.unwrap();
        let rollback = preview_shared_rollback(&journal_path, &root, &fixture.backup_root());

        fs::remove_dir_all(&root).unwrap();
        fs::write(&root, b"now a plain file").unwrap();

        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "user-replaced-file-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        let _ = rolled_back;
        assert!(root.is_file(), "the replacement file must never be removed");
        assert_eq!(fs::read(&root).unwrap(), b"now a plain file");
    }

    // --- journal that also claims the pre-existing ancestor ----------------

    /// Applies into `<fixture>/dolphin/Load/Textures`, where only `dolphin`
    /// existed beforehand, then rewrites the persisted journal so that it
    /// also claims `dolphin` as created by the transaction. `claim` is given
    /// `dolphin` and the identity of the first directory the transaction
    /// really created, and returns the identity the forged entry carries.
    /// Returns `dolphin`, the destination root and the rollback's status.
    fn roll_back_a_journal_that_also_claims_the_ancestor(
        fixture: &Fixture,
        label: &str,
        claim: impl FnOnce(&Path, SharedDirectoryIdentity) -> SharedDirectoryIdentity,
    ) -> (PathBuf, PathBuf, SharedApplyStatus) {
        let dolphin = fixture.0.join("dolphin");
        if !dolphin.exists() {
            fs::create_dir(&dolphin).unwrap();
        }
        let root = dolphin.join("Load").join("Textures");
        let report = preview_at_root(fixture, root.clone(), b"new");
        let plan = make_plan(fixture, &report);
        let result =
            execute_shared_apply(&plan, &options(fixture, &plan, label, false, true, false));
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        let journal_path = result.journal_path.unwrap();

        // Tamper the persisted journal: prepend the pre-existing `dolphin`
        // ancestor to the claimed chain, as if this transaction had
        // created it too.
        let mut tampered = read_journal(&journal_path).unwrap();
        let created = tampered.created_root_directories[0]
            .identity
            .expect("a created directory always carries its identity");
        tampered.created_root_directories.insert(
            0,
            SharedCreatedRootDirectory {
                path: SharedTransactionPath::from_path(&dolphin),
                identity: Some(claim(&dolphin, created)),
            },
        );
        fs::write(&journal_path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();

        let rollback = preview_shared_rollback(&journal_path, &root, &fixture.backup_root());
        assert!(rollback.available);
        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: format!("{label}-rollback"),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        (dolphin, root, rolled_back.status)
    }

    fn real_identity_of(directory: &Path) -> SharedDirectoryIdentity {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(directory).unwrap();
        SharedDirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    #[test]
    fn a_journal_claiming_the_pre_existing_ancestor_with_a_wrong_identity_removes_nothing() {
        // The forged chain is well formed: prepending an ancestor keeps it
        // contiguous, so it is not structure that stops it. The walk in
        // `remove_verified_root_chain` stops at its first entry, whose
        // identity is not the directory's own, and nothing below an entry
        // that stopped the walk is reached.
        //
        // Each case is wrong in exactly one field, and cannot be right by
        // chance, so that each comparison is pinned on its own. (This test
        // used to claim "the created folder's inode + 1". On a filesystem
        // that reuses freed inode numbers the ancestor can have exactly
        // that inode; the claim is then indistinguishable from a true one
        // and the ancestor is removed, which is the only way found by which
        // this test could fail, as it did now and then in CI.)
        type Claim = fn(&Path, SharedDirectoryIdentity) -> SharedDirectoryIdentity;
        // The inode of the folder the transaction created: on the same
        // device as the ancestor, and a different inode for as long as both
        // exist.
        fn the_created_folders_identity(
            _ancestor: &Path,
            created: SharedDirectoryIdentity,
        ) -> SharedDirectoryIdentity {
            created
        }
        // The ancestor's own inode on another device.
        fn the_ancestors_inode_on_another_device(
            ancestor: &Path,
            _created: SharedDirectoryIdentity,
        ) -> SharedDirectoryIdentity {
            let real = real_identity_of(ancestor);
            SharedDirectoryIdentity {
                device: real.device.wrapping_add(1),
                inode: real.inode,
            }
        }
        let cases: [(&str, Claim); 2] = [
            ("wrong-inode", the_created_folders_identity),
            ("wrong-device", the_ancestors_inode_on_another_device),
        ];
        for (label, claim) in cases {
            let fixture = Fixture::new(&format!("bootstrap-forged-ancestor-{label}"));
            let (dolphin, root, status) =
                roll_back_a_journal_that_also_claims_the_ancestor(&fixture, label, claim);
            assert_eq!(status, SharedApplyStatus::Success, "{label}");
            assert!(
                dolphin.is_dir(),
                "{label}: the pre-existing ancestor must never be removed"
            );
            assert!(
                root.exists(),
                "{label}: a chain that stopped at its first entry must not be partially honoured"
            );
        }
    }

    #[test]
    fn an_ancestor_claimed_with_its_real_identity_is_never_removed() {
        // With the ancestor's exact identity the forged claim cannot be told
        // from a true one. It makes no difference: Undo removes the
        // destination root it was opened for and nothing above it.
        for (label, in_use) in [
            ("forged-ancestor-in-use", true),
            ("forged-ancestor-empty", false),
        ] {
            let fixture = Fixture::new(label);
            let dolphin = fixture.0.join("dolphin");
            fs::create_dir(&dolphin).unwrap();
            if in_use {
                fs::write(dolphin.join("Dolphin.ini"), b"[General]\n").unwrap();
            }
            let (dolphin, root, status) = roll_back_a_journal_that_also_claims_the_ancestor(
                &fixture,
                label,
                |ancestor, _| real_identity_of(ancestor),
            );
            assert_eq!(status, SharedApplyStatus::Success, "{label}");
            assert!(!root.exists(), "{label}: the root itself is the install's");
            assert!(
                dolphin.is_dir(),
                "{label}: the pre-existing ancestor must never be removed"
            );
            assert!(
                dolphin.join("Load").is_dir(),
                "{label}: nothing above the root is removed"
            );
            if in_use {
                assert_eq!(
                    fs::read(dolphin.join("Dolphin.ini")).unwrap(),
                    b"[General]\n"
                );
            }
        }
    }

    // --- forged journals: Undo must not act outside the root it was given ---
    //
    // Everything here happens inside the test fixture's own temporary
    // folder. "Outside the root" means a sibling folder of the destination
    // root inside that fixture.

    const INSTALLED: &str = "Nintendo - NES/game.cht";

    /// Installs one new file honestly (`INSTALLED`, bytes `new`) and returns
    /// the path of the persisted journal.
    fn honest_install(fixture: &Fixture, label: &str) -> PathBuf {
        let report = preview(fixture, b"new", None);
        let plan = make_plan(fixture, &report);
        let result =
            execute_shared_apply(&plan, &options(fixture, &plan, label, false, true, false));
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        result.journal_path.unwrap()
    }

    /// Replaces an existing file (bytes `old`) honestly with `new`, keeping a
    /// backup, and returns the path of the persisted journal.
    fn honest_replacement(fixture: &Fixture, label: &str) -> PathBuf {
        let report = preview(fixture, b"new", Some(b"old"));
        let plan = make_plan(fixture, &report);
        let result =
            execute_shared_apply(&plan, &options(fixture, &plan, label, false, true, true));
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        result.journal_path.unwrap()
    }

    fn rewrite_journal(journal_path: &Path, forge: impl FnOnce(&mut SharedApplyJournal)) {
        let mut journal = read_journal(journal_path).unwrap();
        forge(&mut journal);
        fs::write(journal_path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    }

    /// Previews Undo and, when the preview offers it, runs it with approval.
    /// Returns whether it was offered, and the status when it ran.
    fn try_undo(
        fixture: &Fixture,
        journal_path: &Path,
        label: &str,
    ) -> (bool, Option<SharedApplyStatus>) {
        let root = fixture.destination_root();
        let preview = preview_shared_rollback(journal_path, &root, &fixture.backup_root());
        if !preview.available {
            return (false, None);
        }
        let result = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: format!("{label}-rollback"),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        (true, Some(result.status))
    }

    /// The folder next to the destination root, inside the fixture.
    fn outside(fixture: &Fixture) -> PathBuf {
        let outside = fixture.0.join("outside");
        if !outside.exists() {
            fs::create_dir(&outside).unwrap();
        }
        outside
    }

    /// A file outside the destination root holding the bytes the honest
    /// install wrote, so the digest the journal records is also this file's.
    fn someone_elses_file(fixture: &Fixture, name: &str) -> PathBuf {
        let path = outside(fixture).join(name);
        fs::write(&path, b"new").unwrap();
        path
    }

    fn assert_refused_and_untouched(
        fixture: &Fixture,
        outcome: (bool, Option<SharedApplyStatus>),
        kept: &Path,
        what: &str,
    ) {
        assert!(
            kept.exists(),
            "{what}: Undo removed {} (offered: {}, status: {:?})",
            kept.display(),
            outcome.0,
            outcome.1
        );
        assert!(
            !outcome.0,
            "{what}: Undo was offered for a journal that names something outside its root \
             (status when run: {:?})",
            outcome.1
        );
        // A refused record undoes nothing at all: what the transaction did
        // install is still there.
        assert!(
            fixture.destination_root().join(INSTALLED).exists(),
            "{what}: a refused record must not be partly undone"
        );
    }

    #[test]
    fn a_forged_absolute_path_cannot_make_undo_delete_a_file_outside_the_root() {
        let fixture = Fixture::new("forged-absolute");
        let journal = honest_install(&fixture, "forged-absolute");
        let theirs = someone_elses_file(&fixture, "keep.cht");
        rewrite_journal(&journal, |journal| {
            journal.entries[0].plan_entry.destination_relative_path =
                SharedTransactionPath::from_path(&theirs);
        });
        let outcome = try_undo(&fixture, &journal, "forged-absolute");
        assert_eq!(fs::read(&theirs).unwrap_or_default(), b"new");
        assert_refused_and_untouched(&fixture, outcome, &theirs, "absolute path");
    }

    #[test]
    fn a_forged_parent_reference_cannot_make_undo_delete_a_file_outside_the_root() {
        let fixture = Fixture::new("forged-dotdot");
        let journal = honest_install(&fixture, "forged-dotdot");
        let theirs = someone_elses_file(&fixture, "keep.cht");
        rewrite_journal(&journal, |journal| {
            journal.entries[0].plan_entry.destination_relative_path =
                SharedTransactionPath::from_path(Path::new("../outside/keep.cht"));
        });
        let outcome = try_undo(&fixture, &journal, "forged-dotdot");
        assert_refused_and_untouched(&fixture, outcome, &theirs, "`..` in the file path");
    }

    #[test]
    fn a_forged_entry_root_cannot_make_undo_delete_a_file_under_another_root() {
        let fixture = Fixture::new("forged-entry-root");
        let journal = honest_install(&fixture, "forged-entry-root");
        // The preview looks under the root the caller named, the removal
        // under the root the entry names. A decoy satisfies the first.
        let theirs = someone_elses_file(&fixture, "game.cht");
        let elsewhere = outside(&fixture);
        rewrite_journal(&journal, |journal| {
            journal.entries[0].plan_entry.destination_root =
                SharedTransactionPath::from_path(&elsewhere);
            journal.entries[0].plan_entry.destination_relative_path =
                SharedTransactionPath::from_path(Path::new("game.cht"));
        });
        fs::write(fixture.destination_root().join("game.cht"), b"new").unwrap();
        let outcome = try_undo(&fixture, &journal, "forged-entry-root");
        assert_refused_and_untouched(&fixture, outcome, &theirs, "another root in the entry");
        assert!(fixture.destination_root().join("game.cht").exists());
    }

    #[test]
    fn a_forged_created_folder_cannot_make_undo_remove_a_folder_outside_the_root() {
        let fixture = Fixture::new("forged-created-folder");
        let journal = honest_install(&fixture, "forged-created-folder");
        let theirs = outside(&fixture).join("empty");
        fs::create_dir(&theirs).unwrap();
        let named = fixture
            .destination_root()
            .join("..")
            .join("outside")
            .join("empty");
        rewrite_journal(&journal, |journal| {
            journal.entries[0].destination_parent_existed_before_apply = Some(false);
            journal.entries[0]
                .created_directories
                .push(SharedTransactionPath::from_path(&named));
        });
        let outcome = try_undo(&fixture, &journal, "forged-created-folder");
        assert_refused_and_untouched(&fixture, outcome, &theirs, "`..` in a created folder");
    }

    #[test]
    fn a_created_folder_reached_through_a_link_is_not_removed() {
        let fixture = Fixture::new("forged-created-folder-link");
        let journal = honest_install(&fixture, "forged-created-folder-link");
        let theirs = outside(&fixture).join("empty");
        fs::create_dir(&theirs).unwrap();
        // Lexically inside the root; really somewhere else.
        std::os::unix::fs::symlink(outside(&fixture), fixture.destination_root().join("link"))
            .unwrap();
        let named = fixture.destination_root().join("link").join("empty");
        rewrite_journal(&journal, |journal| {
            journal.entries[0].destination_parent_existed_before_apply = Some(false);
            journal.entries[0]
                .created_directories
                .push(SharedTransactionPath::from_path(&named));
        });
        let outcome = try_undo(&fixture, &journal, "forged-created-folder-link");
        assert!(
            theirs.is_dir(),
            "a folder outside the root was removed through a link (offered: {}, status: {:?})",
            outcome.0,
            outcome.1
        );
        // The record is refused as a whole: the folder is not on the way
        // to the install's own file.
        assert!(!outcome.0);
        // And the clean-up itself, handed the claim directly, does not
        // follow the link.
        let forged = read_journal(&journal).unwrap();
        cleanup_created_directories(&forged.entries[0], &fixture.destination_root());
        assert!(
            theirs.is_dir(),
            "the clean-up followed a link out of the root"
        );
    }

    #[test]
    fn a_forged_file_path_through_a_link_cannot_make_undo_delete_outside_the_root() {
        let fixture = Fixture::new("forged-file-link");
        let journal = honest_install(&fixture, "forged-file-link");
        let theirs = someone_elses_file(&fixture, "keep.cht");
        std::os::unix::fs::symlink(outside(&fixture), fixture.destination_root().join("link"))
            .unwrap();
        rewrite_journal(&journal, |journal| {
            journal.entries[0].plan_entry.destination_relative_path =
                SharedTransactionPath::from_path(Path::new("link/keep.cht"));
        });
        let outcome = try_undo(&fixture, &journal, "forged-file-link");
        assert_eq!(
            fs::read(&theirs).unwrap_or_default(),
            b"new",
            "a file outside the root was deleted through a link (offered: {}, status: {:?})",
            outcome.0,
            outcome.1
        );
        assert!(
            outcome.1 != Some(SharedApplyStatus::Success),
            "Undo through a link reported success"
        );
    }

    #[test]
    fn a_forged_backup_path_cannot_make_undo_write_someone_elses_bytes() {
        let fixture = Fixture::new("forged-backup");
        let journal = honest_replacement(&fixture, "forged-backup");
        let planted = outside(&fixture).join("planted.cht");
        fs::write(&planted, b"not the original").unwrap();
        // Lexically under the backup root; really outside it.
        let named = fixture
            .backup_root()
            .join("..")
            .join("outside")
            .join("planted.cht");
        rewrite_journal(&journal, |journal| {
            journal.entries[0].backup_path = Some(SharedTransactionPath::from_path(&named));
            journal.entries[0].backup_digest = Some(digest_bytes(b"not the original"));
        });
        let outcome = try_undo(&fixture, &journal, "forged-backup");
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"new",
            "Undo wrote bytes from outside the backup root (offered: {}, status: {:?})",
            outcome.0,
            outcome.1
        );
        assert!(
            !outcome.0,
            "Undo was offered with a backup outside the backup root"
        );
    }

    /// Makes the operation `label` look as an earlier EmuWiz left it: its
    /// backups, and no receipt of any kind.
    fn forget_receipts(fixture: &Fixture, label: &str) {
        for entry in fs::read_dir(fixture.backup_root().join(label)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension() != Some(OsStr::new("bak")) {
                fs::remove_file(path).unwrap();
            }
        }
    }

    /// Where the backup store keeps the receipt for the file an honest
    /// install made, under the operation `label`.
    fn receipt_for(fixture: &Fixture, label: &str, relative: &str) -> PathBuf {
        operation_receipt_path(
            &fixture.backup_root(),
            label,
            &fixture.destination_root(),
            Path::new(relative),
            INSTALL_RECEIPT_SUFFIX,
        )
        .unwrap()
    }

    /// Where the backup store keeps the receipt for what an honest install
    /// put over an existing file, under the operation `label`.
    fn replacement_receipt_for(fixture: &Fixture, label: &str, relative: &str) -> PathBuf {
        operation_receipt_path(
            &fixture.backup_root(),
            label,
            &fixture.destination_root(),
            Path::new(relative),
            REPLACEMENT_RECEIPT_SUFFIX,
        )
        .unwrap()
    }

    #[test]
    fn an_honest_install_leaves_its_receipt_in_the_backup_store() {
        let fixture = Fixture::new("receipt");
        honest_install(&fixture, "receipt");
        let receipt = receipt_for(&fixture, "receipt", INSTALLED);
        assert!(receipt.starts_with(fixture.backup_root().join("receipt")));
        assert_eq!(
            fs::read(&receipt).unwrap(),
            digest_bytes(b"new").as_bytes(),
            "the receipt is the digest of what was installed"
        );
        // A replaced file is shown by its backup. Its receipt says what was
        // installed over it, and is not the receipt of a new file.
        let fixture = Fixture::new("receipt-replace");
        let journal = honest_replacement(&fixture, "receipt-replace");
        assert!(!receipt_for(&fixture, "receipt-replace", INSTALLED).exists());
        assert_eq!(
            fs::read(replacement_receipt_for(
                &fixture,
                "receipt-replace",
                INSTALLED
            ))
            .unwrap(),
            digest_bytes(b"new").as_bytes(),
            "the receipt is the digest of what was installed over the file"
        );
        let recorded = read_journal(&journal).unwrap();
        let backup = recorded.entries[0]
            .backup_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        assert_eq!(
            backup,
            operation_evidence_path(
                &fixture.backup_root(),
                "receipt-replace",
                &fixture.destination_root().join(INSTALLED),
                BACKUP_SUFFIX
            )
            .unwrap()
        );
    }

    #[test]
    fn an_install_that_cannot_record_itself_does_not_start() {
        for (label, existing) in [
            ("operation-unwritable-new", None),
            ("operation-unwritable-replace", Some(b"old".as_slice())),
        ] {
            let fixture = Fixture::new(label);
            let report = preview(&fixture, b"new", existing);
            let plan = make_plan(&fixture, &report);
            // Something that is not a folder stands where this operation's
            // evidence would go.
            fs::create_dir_all(fixture.backup_root()).unwrap();
            fs::write(fixture.backup_root().join(label), b"in the way").unwrap();
            let result =
                execute_shared_apply(&plan, &options(&fixture, &plan, label, false, true, true));
            assert_eq!(result.journal.status, SharedApplyStatus::Failed, "{label}");
            assert!(result.journal.entries[0].backup_path.is_none(), "{label}");
            // Nothing recorded, so nothing touched.
            let live = fixture.destination_root().join(INSTALLED);
            match existing {
                None => assert!(!live.exists(), "{label}"),
                Some(bytes) => assert_eq!(fs::read(&live).unwrap(), bytes, "{label}"),
            }
            assert_eq!(
                fs::read(fixture.backup_root().join(label)).unwrap(),
                b"in the way"
            );
        }
    }

    #[test]
    fn a_file_whose_receipt_cannot_be_written_is_left_as_it_was() {
        for (label, existing) in [
            ("receipt-unwritable-new", None),
            ("receipt-unwritable-replace", Some(b"old".as_slice())),
        ] {
            let fixture = Fixture::new(label);
            let report = preview(&fixture, b"new", existing);
            let plan = make_plan(&fixture, &report);
            // Something already stands exactly where this file's receipt
            // would go, in the operation's own folder.
            let receipt = match existing {
                None => receipt_for(&fixture, label, INSTALLED),
                Some(_) => replacement_receipt_for(&fixture, label, INSTALLED),
            };
            fs::create_dir_all(&receipt).unwrap();
            let result =
                execute_shared_apply(&plan, &options(&fixture, &plan, label, false, true, true));
            assert_ne!(result.journal.status, SharedApplyStatus::Success, "{label}");
            assert_eq!(
                result.journal.entries[0].outcome,
                SharedApplyOutcome::BackupFailed,
                "{label}"
            );
            let live = fixture.destination_root().join(INSTALLED);
            match existing {
                None => assert!(!live.exists(), "{label}"),
                Some(bytes) => assert_eq!(fs::read(&live).unwrap(), bytes, "{label}"),
            }
        }
    }

    #[test]
    fn a_failed_publish_withdraws_its_receipt() {
        let fixture = Fixture::new("receipt-withdrawn");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::Rename));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "receipt-withdrawn", false, true, false),
        );
        inject_fault(None);
        assert_ne!(result.journal.status, SharedApplyStatus::Success);
        assert_eq!(
            result.journal.entries[0].publication,
            SharedPublicationState::NotPublished
        );
        assert!(!fixture.destination_root().join(INSTALLED).exists());
        assert!(
            !receipt_for(&fixture, "receipt-withdrawn", INSTALLED).exists(),
            "a receipt was left for a file that was never installed"
        );
    }

    #[test]
    fn an_undone_install_cannot_be_undone_again_by_putting_its_record_back() {
        let fixture = Fixture::new("receipt-replay");
        let journal = honest_install(&fixture, "receipt-replay");
        // The history folder exactly as the install left it.
        let history: Vec<(PathBuf, Vec<u8>)> = fs::read_dir(fixture.history_root())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .map(|path| {
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        assert!(history.iter().any(|(path, _)| *path == journal));

        assert_eq!(
            try_undo(&fixture, &journal, "receipt-replay"),
            (true, Some(SharedApplyStatus::Success))
        );
        assert!(!fixture.destination_root().join(INSTALLED).exists());
        assert!(
            !receipt_for(&fixture, "receipt-replay", INSTALLED).exists(),
            "Undo removed the file and left the receipt that vouched for it"
        );

        // Later, someone's own file with the same bytes at the same place,
        // and the history folder put back to how it was before the Undo.
        let mine = fixture.destination_root().join(INSTALLED);
        fs::create_dir_all(mine.parent().unwrap()).unwrap();
        fs::write(&mine, b"new").unwrap();
        for entry in fs::read_dir(fixture.history_root()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                fs::remove_file(path).unwrap();
            }
        }
        for (path, bytes) in &history {
            fs::write(path, bytes).unwrap();
        }
        // The record is as good as new and the bytes match. What refuses
        // it is that EmuWiz no longer holds a receipt for this file.
        let again = preview_shared_rollback(
            &journal,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert_eq!(
            again.entries[0].outcome,
            SharedRollbackOutcome::BackupMissing
        );
        assert_eq!(
            try_undo(&fixture, &journal, "receipt-replay-2"),
            (false, None)
        );
        assert_eq!(fs::read(&mine).unwrap(), b"new");
    }

    #[test]
    fn a_receipt_is_named_from_its_root_and_its_path_kept_apart() {
        use std::os::unix::ffi::OsStrExt;
        let store = Path::new("/backups");
        let name = |root: &[u8], relative: &[u8], suffix: &str| {
            operation_receipt_path(
                store,
                "op",
                Path::new(OsStr::from_bytes(root)),
                Path::new(OsStr::from_bytes(relative)),
                suffix,
            )
            .unwrap()
        };
        // The same file named through a wider root is another receipt.
        assert_ne!(
            name(b"/games/cheats", b"nes/save.cht", INSTALL_RECEIPT_SUFFIX),
            name(b"/games", b"cheats/nes/save.cht", INSTALL_RECEIPT_SUFFIX)
        );
        assert_ne!(
            name(
                b"/games/cheats",
                b"nes/save.cht",
                REPLACEMENT_RECEIPT_SUFFIX
            ),
            name(
                b"/games",
                b"cheats/nes/save.cht",
                REPLACEMENT_RECEIPT_SUFFIX
            )
        );
        // Two paths that differ only in a byte that is not text read the
        // same as text, and share a backup name. Their receipts are still
        // different files.
        let backup = |path: &[u8]| {
            operation_evidence_path(
                store,
                "op",
                Path::new(OsStr::from_bytes(path)),
                BACKUP_SUFFIX,
            )
            .unwrap()
        };
        assert_eq!(
            backup(b"/games/save\xff.cht"),
            backup(b"/games/save\xfe.cht")
        );
        assert_ne!(
            name(b"/games", b"save\xff.cht", INSTALL_RECEIPT_SUFFIX),
            name(b"/games", b"save\xfe.cht", INSTALL_RECEIPT_SUFFIX)
        );
        // The receipt of a new file and the receipt of a replacement are
        // never the same file, and both sit in the operation's own folder.
        let new = name(b"/games", b"save.cht", INSTALL_RECEIPT_SUFFIX);
        let replacement = name(b"/games", b"save.cht", REPLACEMENT_RECEIPT_SUFFIX);
        assert_ne!(new, replacement);
        assert_eq!(new.parent(), Some(Path::new("/backups/op")));
        assert_eq!(replacement.parent(), Some(Path::new("/backups/op")));
        // The operation is one plain name: it cannot point elsewhere.
        for operation in ["", ".", "..", "a/b", "../op"] {
            assert!(
                operation_receipt_path(
                    store,
                    operation,
                    Path::new("/x"),
                    Path::new("y"),
                    INSTALL_RECEIPT_SUFFIX
                )
                .is_err(),
                "{operation:?}"
            );
        }
    }

    /// An Undo opened for `root`, as a screen would open it with whatever
    /// spelling of the folder it holds.
    fn try_undo_for(
        fixture: &Fixture,
        journal_path: &Path,
        root: &Path,
        label: &str,
    ) -> (bool, Option<SharedApplyStatus>) {
        let preview = preview_shared_rollback(journal_path, root, &fixture.backup_root());
        if !preview.available {
            return (false, None);
        }
        let result = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: format!("{label}-rollback"),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        (true, Some(result.status))
    }

    /// The same folder, spelled three other ways.
    fn other_spellings(root: &Path) -> Vec<(&'static str, PathBuf)> {
        let text = root.to_str().unwrap();
        let (parent, name) = text.rsplit_once('/').unwrap();
        vec![
            ("trailing-slash", PathBuf::from(format!("{text}/"))),
            ("doubled-slash", PathBuf::from(format!("{parent}//{name}"))),
            ("dot-segment", PathBuf::from(format!("{parent}/./{name}"))),
        ]
    }

    #[test]
    fn undo_opened_for_the_same_folder_spelled_differently_finds_the_installs_evidence() {
        let spellings = other_spellings(Path::new("/x/destination"));
        for (index, (spelling, _)) in spellings.iter().enumerate() {
            // A file installed where there was none.
            let label = format!("spelled-new-{spelling}");
            let fixture = Fixture::new(&label);
            let journal = honest_install(&fixture, &label);
            let root = other_spellings(&fixture.destination_root()).remove(index).1;
            assert_ne!(root.as_os_str(), fixture.destination_root().as_os_str());
            assert_eq!(
                try_undo_for(&fixture, &journal, &root, &label),
                (true, Some(SharedApplyStatus::Success)),
                "{spelling}"
            );
            assert!(!fixture.destination_root().join(INSTALLED).exists());

            // A replaced file.
            let label = format!("spelled-replace-{spelling}");
            let fixture = Fixture::new(&label);
            let journal = honest_replacement(&fixture, &label);
            let root = other_spellings(&fixture.destination_root()).remove(index).1;
            assert_eq!(
                try_undo_for(&fixture, &journal, &root, &label),
                (true, Some(SharedApplyStatus::Success)),
                "{spelling}"
            );
            assert_eq!(
                fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
                b"old"
            );

            // A replaced file from an install made before receipts: its
            // backup is found under the name it was given at install.
            let label = format!("spelled-legacy-{spelling}");
            let fixture = Fixture::new(&label);
            let journal = honest_replacement(&fixture, &label);
            forget_receipts(&fixture, &label);
            let root = other_spellings(&fixture.destination_root()).remove(index).1;
            assert_eq!(
                try_undo_for(&fixture, &journal, &root, &label),
                (true, Some(SharedApplyStatus::Success)),
                "{spelling}"
            );
            assert_eq!(
                fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
                b"old"
            );
        }
    }

    #[test]
    fn undo_opened_for_another_folder_is_refused_however_it_leads_to_the_same_place() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("another-folder");
        let journal = honest_install(&fixture, "another-folder");
        let installed = fixture.destination_root().join(INSTALLED);
        // A link to the very same folder is another path.
        let alias = fixture.0.join("alias");
        symlink(fixture.destination_root(), &alias).unwrap();
        // So is the same folder reached by going up and back down.
        let around = fixture.destination_root().join("..").join("destination");
        // And the folder above it, and a folder beside it.
        let beside = fixture.0.join("beside");
        fs::create_dir(&beside).unwrap();
        for (what, root) in [
            ("a link to the folder", alias),
            ("up and back down", around),
            ("the folder above", fixture.0.clone()),
            ("a folder beside it", beside),
        ] {
            let preview = preview_shared_rollback(&journal, &root, &fixture.backup_root());
            assert!(!preview.available, "{what}");
            assert_eq!(
                preview.entries[0].outcome,
                SharedRollbackOutcome::RootMismatch,
                "{what}"
            );
            assert_eq!(fs::read(&installed).unwrap(), b"new", "{what}");
        }
        // The folder itself, as recorded, is still undone.
        assert_eq!(
            try_undo(&fixture, &journal, "another-folder"),
            (true, Some(SharedApplyStatus::Success))
        );
        assert!(!installed.exists());
    }

    /// History opens Undo for the folder the record names. A record that
    /// names a wider folder, with the rest moved into the path, points at
    /// the very same file. It must gain nothing by it.
    #[test]
    fn a_record_that_widens_its_destination_folder_removes_nothing() {
        let fixture = Fixture::new("forged-wider-root");
        let journal = honest_install(&fixture, "forged-wider-root");
        let installed = fixture.destination_root().join(INSTALLED);
        // An empty folder beside the real destination folder.
        let theirs = fixture.0.join("someones-empty-folder");
        fs::create_dir(&theirs).unwrap();
        let wider = fixture.0.clone();
        let moved = Path::new("destination").join(INSTALLED);
        rewrite_journal(&journal, |journal| {
            journal.destination_root = SharedTransactionPath::from_path(&wider);
            let entry = &mut journal.entries[0];
            entry.plan_entry.destination_root = SharedTransactionPath::from_path(&wider);
            entry.plan_entry.destination_relative_path = SharedTransactionPath::from_path(&moved);
            entry.destination_parent_existed_before_apply = Some(false);
            // Only folders on the way to the file, so the path rules pass:
            // the real destination folder and the folder below it.
            entry.created_directories = vec![
                SharedTransactionPath::from_path(&fixture.destination_root()),
                SharedTransactionPath::from_path(
                    &fixture.destination_root().join("Nintendo - NES"),
                ),
            ];
        });
        // Undo is opened for the folder the record names, as History does.
        let preview = preview_shared_rollback(&journal, &wider, &fixture.backup_root());
        assert_eq!(
            preview.entries[0].outcome,
            SharedRollbackOutcome::JournalMalformed,
            "this is no longer the record of the install EmuWiz kept: {:?}",
            preview.entries[0]
        );
        assert!(!preview.available);
        let result = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "forged-wider-root-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(result.status, SharedApplyStatus::Failed);
        assert_eq!(fs::read(&installed).unwrap(), b"new");
        assert!(fixture.destination_root().join("Nintendo - NES").is_dir());
        assert!(theirs.is_dir());
        // The receipt the honest install wrote is still there for an Undo
        // opened for the real folder.
        assert!(receipt_for(&fixture, "forged-wider-root", INSTALLED).exists());

        // Naming the neighbour's folder as one the install created is
        // refused outright: it is not on the way to the file.
        rewrite_journal(&journal, |journal| {
            journal.entries[0]
                .created_directories
                .push(SharedTransactionPath::from_path(&theirs));
        });
        let preview = preview_shared_rollback(&journal, &wider, &fixture.backup_root());
        assert_eq!(
            preview.entries[0].outcome,
            SharedRollbackOutcome::JournalMalformed
        );
        assert!(theirs.is_dir());
        assert_eq!(fs::read(&installed).unwrap(), b"new");
    }

    #[test]
    fn a_wider_destination_folder_does_not_make_a_replacement_look_like_an_older_install() {
        let fixture = Fixture::new("forged-wider-root-replace");
        let journal = honest_replacement(&fixture, "forged-wider-root-replace");
        let live = fixture.destination_root().join(INSTALLED);
        fs::write(&live, b"changed since").unwrap();
        let wider = fixture.0.clone();
        let moved = Path::new("destination").join(INSTALLED);
        rewrite_journal(&journal, |journal| {
            journal.destination_root = SharedTransactionPath::from_path(&wider);
            let entry = &mut journal.entries[0];
            entry.plan_entry.destination_root = SharedTransactionPath::from_path(&wider);
            entry.plan_entry.destination_relative_path = SharedTransactionPath::from_path(&moved);
            entry.final_destination_digest = Some(digest_bytes(b"changed since"));
        });
        // Named this way the file has no receipt. The operation kept
        // receipts all the same, so its backup is not believed on the
        // record's word as an older install's would be.
        let preview = preview_shared_rollback(&journal, &wider, &fixture.backup_root());
        assert_eq!(
            preview.entries[0].outcome,
            SharedRollbackOutcome::JournalMalformed,
            "{:?}",
            preview.entries[0]
        );
        assert!(!preview.available);
        assert_eq!(fs::read(&live).unwrap(), b"changed since");
        // The check on the replacement's own receipt, by itself: an install
        // that kept receipts has none under this name, so the record is not
        // believed; one that kept none would be.
        let claimed = digest_bytes(b"changed since");
        assert!(
            replacement_receipt_agrees(
                &fixture.backup_root(),
                "forged-wider-root-replace",
                &wider,
                &moved,
                Some(&claimed),
            )
            .is_err()
        );
        forget_receipts(&fixture, "forged-wider-root-replace");
        assert!(
            replacement_receipt_agrees(
                &fixture.backup_root(),
                "forged-wider-root-replace",
                &wider,
                &moved,
                Some(&claimed),
            )
            .is_ok()
        );
    }

    #[test]
    fn a_record_cannot_claim_an_empty_folder_that_is_not_on_the_way_to_its_file() {
        let fixture = Fixture::new("forged-unrelated-folder");
        let journal = honest_install(&fixture, "forged-unrelated-folder");
        let theirs = fixture.destination_root().join("Someone's saves");
        fs::create_dir(&theirs).unwrap();
        rewrite_journal(&journal, |journal| {
            journal.entries[0]
                .created_directories
                .push(SharedTransactionPath::from_path(&theirs));
        });
        assert_eq!(try_undo(&fixture, &journal, "unrelated"), (false, None));
        assert!(theirs.is_dir());
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"new"
        );
        // Nor more folders than an install makes.
        rewrite_journal(&journal, |journal| {
            let parent = SharedTransactionPath::from_path(
                &fixture.destination_root().join("Nintendo - NES"),
            );
            journal.entries[0].created_directories =
                vec![parent; SHARED_MAX_CREATED_DIRECTORIES + 1];
        });
        assert_eq!(try_undo(&fixture, &journal, "too-many"), (false, None));
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"new"
        );
    }

    #[test]
    fn a_record_that_breaks_the_path_rules_is_refused_before_any_file_it_names_is_read() {
        let check = |label: &str, reason: &str, forge: &dyn Fn(&mut SharedApplyJournal, &Path)| {
            let fixture = Fixture::new(label);
            let journal = honest_install(&fixture, label);
            let theirs = someone_elses_file(&fixture, "keep.cht");
            rewrite_journal(&journal, |journal| forge(journal, &theirs));
            let preview = preview_shared_rollback(
                &journal,
                &fixture.destination_root(),
                &fixture.backup_root(),
            );
            assert!(!preview.available, "{label}");
            for entry in &preview.entries {
                // Refused by the path rules themselves, not for want of a
                // receipt, and without opening what the record names.
                assert_eq!(
                    entry.outcome,
                    SharedRollbackOutcome::JournalMalformed,
                    "{label}"
                );
                assert!(entry.observed_destination_digest.is_none(), "{label}");
                assert!(entry.destination.is_none(), "{label}");
                let detail = &entry.failure.as_ref().unwrap().detail;
                assert!(detail.contains(reason), "{label}: {detail}");
            }
        };
        check("rules-absolute", "not a plain path", &|journal, theirs| {
            journal.entries[0].plan_entry.destination_relative_path =
                SharedTransactionPath::from_path(theirs);
        });
        check("rules-parent", "not a plain path", &|journal, _| {
            journal.entries[0].plan_entry.destination_relative_path =
                SharedTransactionPath::from_path(Path::new("../outside/keep.cht"));
        });
        check(
            "rules-entry-root",
            "other than the one",
            &|journal, theirs| {
                journal.entries[0].plan_entry.destination_root =
                    SharedTransactionPath::from_path(theirs.parent().unwrap());
            },
        );
    }

    #[test]
    fn a_record_rewritten_after_undo_was_approved_is_not_acted_on() {
        let fixture = Fixture::new("rewritten-after-approval");
        let journal = honest_replacement(&fixture, "rewritten-after-approval");
        let live = fixture.destination_root().join(INSTALLED);
        let approved = preview_shared_rollback(
            &journal,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(approved.available);
        // Between the approval and the Undo the file changes, and the
        // record is rewritten to match it.
        fs::write(&live, b"changed since").unwrap();
        rewrite_journal(&journal, |journal| {
            journal.entries[0].final_destination_digest = Some(digest_bytes(b"changed since"));
        });
        let result = execute_shared_rollback(
            &approved,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: approved.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "rewritten-after-approval-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(result.status, SharedApplyStatus::Failed);
        assert!(result.journal_path.is_none());
        assert_eq!(fs::read(&live).unwrap(), b"changed since");
    }

    #[test]
    fn an_older_install_that_added_and_replaced_files_is_refused_as_a_whole_and_everything_is_kept()
    {
        let fixture = Fixture::new("legacy-mixed");
        let plan = mixed_plan(
            &fixture,
            &[
                ("Nintendo - NES/aaa-overlay.cfg", None, b"overlay"),
                ("Nintendo - NES/game.cht", Some(b"old"), b"new"),
            ],
        );
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "legacy-mixed", false, true, true),
        );
        assert_eq!(applied.journal.status, SharedApplyStatus::Success);
        let journal = applied.journal_path.clone().unwrap();
        // As an earlier EmuWiz left it: the backup, and no receipt of
        // either kind.
        let evidence = fixture.backup_root().join("legacy-mixed");
        let mut backups = 0;
        for entry in fs::read_dir(&evidence).unwrap() {
            let path = entry.unwrap().path();
            if path.extension() == Some(OsStr::new("bak")) {
                backups += 1;
            } else {
                fs::remove_file(path).unwrap();
            }
        }
        assert_eq!(backups, 1);
        let recorded = fs::read(&journal).unwrap();
        assert_eq!(try_undo(&fixture, &journal, "legacy-mixed"), (false, None));
        assert_eq!(
            fs::read(
                fixture
                    .destination_root()
                    .join("Nintendo - NES/aaa-overlay.cfg")
            )
            .unwrap(),
            b"overlay"
        );
        assert_eq!(
            fs::read(fixture.destination_root().join("Nintendo - NES/game.cht")).unwrap(),
            b"new"
        );
        assert_eq!(fs::read(&journal).unwrap(), recorded);
        assert_eq!(
            fs::read_dir(&evidence).unwrap().count(),
            1,
            "the backup of the replaced original is kept"
        );
    }

    /// Another install into the same fixture: its own source files, its
    /// own operation. Returns its record.
    fn install_also(fixture: &Fixture, label: &str, items: &[MixedItem]) -> PathBuf {
        fs::create_dir_all(fixture.source_root()).unwrap();
        fs::create_dir_all(fixture.destination_root().join("Nintendo - NES")).unwrap();
        let source_items = items
            .iter()
            .enumerate()
            .map(|(index, (relative, existing, new))| {
                let source = fixture.source_root().join(format!("{label}-{index}"));
                fs::write(&source, new).unwrap();
                if let Some(bytes) = existing {
                    fs::write(fixture.destination_root().join(relative), bytes).unwrap();
                }
                PreviewSourceItem {
                    adapter: PreviewAdapter::RetroArch,
                    source_path: source,
                    expected_source_digest: None,
                    destination_relative_paths: vec![PathBuf::from(relative)],
                    match_strength: PreviewMatchStrength::VerifiedExact,
                }
            })
            .collect();
        let report = build_shared_preview(&SharedPreviewRequest {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: fixture.0.join(format!("{label}.zip")),
            platform: Some("NES".into()),
            identity: PreviewIdentity {
                kind: PreviewIdentityKind::RetroArchCatalogueMatch,
                state: PreviewIdentityState::Verified,
                value: Some("archive-1".into()),
                archive_path: fixture.0.join(format!("{label}.zip")),
                revision: None,
            },
            destination_root: fixture.destination_root(),
            source_items,
        })
        .unwrap();
        let plan = make_plan(fixture, &report);
        let applied =
            execute_shared_apply(&plan, &options(fixture, &plan, label, false, true, true));
        assert_eq!(
            applied.journal.status,
            SharedApplyStatus::Success,
            "{:?}",
            applied.journal
        );
        applied.journal_path.unwrap()
    }

    #[test]
    fn one_installs_record_cannot_undo_another_installs_files() {
        let fixture = Fixture::new("cross-install");
        let x = install_also(
            &fixture,
            "mod-x",
            &[
                ("Nintendo - NES/x-added.cht", None, b"x-added"),
                (
                    "Nintendo - NES/x-replaced.cfg",
                    Some(b"x-before"),
                    b"x-after",
                ),
            ],
        );
        let y = install_also(
            &fixture,
            "mod-y",
            &[
                ("Nintendo - NES/y-added.cht", None, b"y-added"),
                (
                    "Nintendo - NES/y-replaced.cfg",
                    Some(b"y-before"),
                    b"y-after",
                ),
            ],
        );
        let file = |name: &str| fixture.destination_root().join("Nintendo - NES").join(name);
        let refused_and_untouched = |record: &Path, what: &str| {
            assert_eq!(
                try_undo(&fixture, record, "tampered"),
                (false, None),
                "{what}"
            );
            assert_eq!(fs::read(file("x-added.cht")).unwrap(), b"x-added", "{what}");
            assert_eq!(
                fs::read(file("x-replaced.cfg")).unwrap(),
                b"x-after",
                "{what}"
            );
            assert_eq!(fs::read(file("y-added.cht")).unwrap(), b"y-added", "{what}");
            assert_eq!(
                fs::read(file("y-replaced.cfg")).unwrap(),
                b"y-after",
                "{what}"
            );
        };
        let honest_x = fs::read(&x).unwrap();
        let honest_y = fs::read(&y).unwrap();
        let ys = read_journal(&y).unwrap();

        // X's record carrying Y's files in place of its own.
        rewrite_journal(&x, |journal| journal.entries = ys.entries.clone());
        refused_and_untouched(&x, "Y's entries in X's record");
        // X's record carrying Y's files beside its own.
        fs::write(&x, &honest_x).unwrap();
        rewrite_journal(&x, |journal| journal.entries.extend(ys.entries.clone()));
        refused_and_untouched(&x, "Y's entries added to X's record");
        // X's record claiming to be Y's operation, with Y's files.
        fs::write(&x, &honest_x).unwrap();
        rewrite_journal(&x, |journal| {
            journal.operation_id = ys.operation_id.clone();
            journal.plan_id = ys.plan_id.clone();
            journal.entries = ys.entries.clone();
        });
        refused_and_untouched(&x, "X's record renamed to Y's operation");
        // Y's whole record, byte for byte, filed under X's name.
        fs::write(&x, &honest_y).unwrap();
        refused_and_untouched(&x, "Y's record filed as X");
        fs::write(&x, &honest_x).unwrap();

        // Y's own record with one of its files left out: half an Undo.
        rewrite_journal(&y, |journal| journal.entries.truncate(1));
        refused_and_untouched(&y, "Y's record with a file left out");
        // Y's own record saying one of its files was never installed.
        for index in [0, 1] {
            fs::write(&y, &honest_y).unwrap();
            rewrite_journal(&y, |journal| {
                let entry = &mut journal.entries[index];
                entry.outcome = SharedApplyOutcome::AlreadyInstalled;
                entry.backup_path = None;
                entry.backup_digest = None;
            });
            refused_and_untouched(&y, "Y's record denying one of its files");
        }
        // Y's own record describing itself as something else.
        fs::write(&y, &honest_y).unwrap();
        rewrite_journal(&y, |journal| {
            journal.context.verified_game_identity = "another game".into();
        });
        refused_and_untouched(&y, "Y's record relabelled");

        // The honest records still undo, each its own install, whole, and
        // nothing of the other's.
        fs::write(&y, &honest_y).unwrap();
        assert_eq!(
            try_undo(&fixture, &y, "mod-y"),
            (true, Some(SharedApplyStatus::Success))
        );
        assert!(!file("y-added.cht").exists());
        assert_eq!(fs::read(file("y-replaced.cfg")).unwrap(), b"y-before");
        assert_eq!(fs::read(file("x-added.cht")).unwrap(), b"x-added");
        assert_eq!(fs::read(file("x-replaced.cfg")).unwrap(), b"x-after");
        assert_eq!(
            try_undo(&fixture, &x, "mod-x"),
            (true, Some(SharedApplyStatus::Success))
        );
        assert!(!file("x-added.cht").exists());
        assert_eq!(fs::read(file("x-replaced.cfg")).unwrap(), b"x-before");
    }

    #[test]
    fn a_record_cannot_pass_over_its_own_files_by_calling_them_already_restored() {
        let fixture = Fixture::new("passed-over");
        let journal = install_also(
            &fixture,
            "passed-over",
            &[
                ("Nintendo - NES/a-added.cht", None, b"a-added"),
                ("Nintendo - NES/b-added.cht", None, b"b-added"),
                (
                    "Nintendo - NES/c-replaced.cfg",
                    Some(b"c-before"),
                    b"c-after",
                ),
            ],
        );
        let file = |name: &str| fixture.destination_root().join("Nintendo - NES").join(name);
        let untouched = |what: &str| {
            assert_eq!(fs::read(file("a-added.cht")).unwrap(), b"a-added", "{what}");
            assert_eq!(fs::read(file("b-added.cht")).unwrap(), b"b-added", "{what}");
            assert_eq!(
                fs::read(file("c-replaced.cfg")).unwrap(),
                b"c-after",
                "{what}"
            );
        };
        let honest = fs::read(&journal).unwrap();

        // The replaced file is said to be back already: its backup is said
        // to hold what the file holds now. The backup in the store holds
        // the original, and that is what counts.
        rewrite_journal(&journal, |journal| {
            let entry = &mut journal.entries[2];
            entry.final_destination_digest = Some(digest_bytes(b"something else"));
            entry.backup_digest = Some(digest_bytes(b"c-after"));
        });
        assert_eq!(try_undo(&fixture, &journal, "c-passed-over"), (false, None));
        untouched("a replaced file called already restored");

        // An added file is said to have been a replacement that is back
        // already. The install never planned to replace it.
        fs::write(&journal, &honest).unwrap();
        rewrite_journal(&journal, |journal| {
            let entry = &mut journal.entries[1];
            entry.outcome = SharedApplyOutcome::ReplacedExisting;
            entry.destination_existed_before_apply = Some(true);
            entry.final_destination_digest = Some(digest_bytes(b"something else"));
            entry.backup_digest = Some(digest_bytes(b"b-added"));
        });
        let preview = preview_shared_rollback(
            &journal,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert_eq!(
            preview.entries[1].outcome,
            SharedRollbackOutcome::JournalMalformed
        );
        assert_eq!(try_undo(&fixture, &journal, "b-passed-over"), (false, None));
        untouched("an added file called a restored replacement");

        // A replaced file really is back when it holds what the operation's
        // own backup holds: an Undo that was interrupted after restoring it
        // can still be finished.
        fs::write(&journal, &honest).unwrap();
        fs::write(file("c-replaced.cfg"), b"c-before").unwrap();
        assert_eq!(
            try_undo(&fixture, &journal, "passed-over"),
            (true, Some(SharedApplyStatus::Success))
        );
        assert!(!file("a-added.cht").exists());
        assert!(!file("b-added.cht").exists());
        assert_eq!(fs::read(file("c-replaced.cfg")).unwrap(), b"c-before");
    }

    #[test]
    fn a_record_cannot_deny_one_of_its_files_because_that_file_has_changed() {
        let fixture = Fixture::new("denied-changed");
        let journal = install_also(
            &fixture,
            "denied-changed",
            &[
                ("Nintendo - NES/a-added.cht", None, b"a-added"),
                ("Nintendo - NES/b-added.cht", None, b"b-added"),
                (
                    "Nintendo - NES/c-replaced.cfg",
                    Some(b"c-before"),
                    b"c-after",
                ),
            ],
        );
        let file = |name: &str| fixture.destination_root().join("Nintendo - NES").join(name);
        let honest = fs::read(&journal).unwrap();
        for (index, name, installed, edited) in [
            (
                1,
                "b-added.cht",
                b"b-added".as_slice(),
                b"b-edited".as_slice(),
            ),
            (
                2,
                "c-replaced.cfg",
                b"c-after".as_slice(),
                b"c-edited".as_slice(),
            ),
        ] {
            // The person edits one file, and only that one. An honest Undo
            // is refused as a whole: it no longer holds what was installed.
            fs::write(&journal, &honest).unwrap();
            fs::write(file(name), edited).unwrap();
            assert_eq!(
                try_undo(&fixture, &journal, "honest"),
                (false, None),
                "{name}"
            );
            // The record is rewritten to say that file was never installed,
            // so that the rest would be undone and the install marked undone.
            rewrite_journal(&journal, |journal| {
                let entry = &mut journal.entries[index];
                entry.outcome = SharedApplyOutcome::WriteFailed;
                entry.verification_succeeded = false;
                entry.failures.clear();
                entry.backup_path = None;
                entry.backup_digest = None;
            });
            let preview = preview_shared_rollback(
                &journal,
                &fixture.destination_root(),
                &fixture.backup_root(),
            );
            // Every other file is ready to be undone; this one alone
            // refuses the Undo.
            for (other, entry) in preview.entries.iter().enumerate() {
                if other == index {
                    assert_eq!(
                        entry.outcome,
                        SharedRollbackOutcome::DestinationChanged,
                        "{name}"
                    );
                } else {
                    assert_eq!(entry.outcome, SharedRollbackOutcome::Available, "{name}");
                }
            }
            assert_eq!(
                try_undo(&fixture, &journal, "denied"),
                (false, None),
                "{name}"
            );
            assert_eq!(fs::read(file("a-added.cht")).unwrap(), b"a-added", "{name}");
            assert_eq!(fs::read(file(name)).unwrap(), edited, "{name}");
            // Put back as installed for the next case.
            fs::write(file(name), installed).unwrap();
        }
    }

    #[test]
    fn a_held_receipt_contradicts_a_denied_file_only_while_something_of_the_install_may_be_there() {
        // The check a record's "not installed" is put to, by itself. What a
        // crash between writing a receipt and publishing the file leaves:
        // the receipt, and no file (or, for a replacement, the original).
        // The record honestly says that file was not installed.
        let fixture = Fixture::new("orphan-receipt");
        let journal = install_also(
            &fixture,
            "orphan-receipt",
            &[("Nintendo - NES/a-added.cht", None, b"a-added")],
        );
        let root = fixture.destination_root();
        let absent = Path::new("Nintendo - NES/never-added.cht");
        let original = Path::new("Nintendo - NES/never-replaced.cfg");
        fs::write(root.join(original), b"original").unwrap();
        let plan = |relative: &Path, action, pre: Option<&[u8]>| {
            let mut entry = read_journal(&journal).unwrap().entries[0]
                .plan_entry
                .clone();
            entry.destination_relative_path = SharedTransactionPath::from_path(relative);
            entry.proposed_action = action;
            entry.destination_pre_digest = pre.map(digest_bytes);
            entry
        };
        for (relative, suffix, action, pre) in [
            (
                absent,
                INSTALL_RECEIPT_SUFFIX,
                PreviewProposedAction::Install,
                None,
            ),
            (
                original,
                REPLACEMENT_RECEIPT_SUFFIX,
                PreviewProposedAction::Replace,
                Some(b"original".as_slice()),
            ),
        ] {
            write_receipt(
                suffix,
                &root,
                relative,
                &digest_bytes(b"was to be installed"),
                &fixture.backup_root(),
                "orphan-receipt",
            )
            .unwrap();
            assert!(
                !receipt_contradicts_not_installed(
                    &fixture.backup_root(),
                    "orphan-receipt",
                    &root,
                    relative,
                    &root.join(relative),
                    &plan(relative, action, pre),
                ),
                "{relative:?}"
            );
        }
        // A planned replacement whose file is gone altogether: nothing of
        // the install is there either.
        fs::remove_file(root.join(original)).unwrap();
        assert!(!receipt_contradicts_not_installed(
            &fixture.backup_root(),
            "orphan-receipt",
            &root,
            original,
            &root.join(original),
            &plan(
                original,
                PreviewProposedAction::Replace,
                Some(b"original".as_slice())
            ),
        ));
        // Something there that cannot be read as a file: not believed.
        fs::create_dir(root.join(absent)).unwrap();
        assert!(receipt_contradicts_not_installed(
            &fixture.backup_root(),
            "orphan-receipt",
            &root,
            absent,
            &root.join(absent),
            &plan(absent, PreviewProposedAction::Install, None),
        ));
        fs::remove_dir(root.join(absent)).unwrap();
        // The same receipts with a file of the install's there: refused.
        fs::write(root.join(absent), b"was to be installed").unwrap();
        fs::write(root.join(original), b"was to be installed").unwrap();
        for (relative, action, pre) in [
            (absent, PreviewProposedAction::Install, None),
            (
                original,
                PreviewProposedAction::Replace,
                Some(b"original".as_slice()),
            ),
        ] {
            assert!(
                receipt_contradicts_not_installed(
                    &fixture.backup_root(),
                    "orphan-receipt",
                    &root,
                    relative,
                    &root.join(relative),
                    &plan(relative, action, pre),
                ),
                "{relative:?}"
            );
        }
    }

    #[test]
    fn an_older_installs_record_filed_under_another_name_is_refused() {
        let fixture = Fixture::new("legacy-renamed");
        let journal = honest_replacement(&fixture, "legacy-renamed");
        forget_receipts(&fixture, "legacy-renamed");
        let copy = fixture.history_root().join("some-other-install.json");
        fs::copy(&journal, &copy).unwrap();
        assert_eq!(try_undo(&fixture, &copy, "copy"), (false, None));
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"new"
        );
        // Under its own name it is still restored.
        assert_eq!(
            try_undo(&fixture, &journal, "legacy-renamed"),
            (true, Some(SharedApplyStatus::Success))
        );
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"old"
        );
    }

    #[test]
    fn an_operation_receipt_that_is_not_a_plain_file_refuses_undo() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("operation-receipt-link");
        let journal = honest_install(&fixture, "operation-receipt-link");
        let receipt = fixture
            .backup_root()
            .join("operation-receipt-link")
            .join(OPERATION_RECEIPT_NAME);
        // A link in its place, to a file holding the right text.
        let elsewhere = fixture.0.join("elsewhere");
        fs::copy(&receipt, &elsewhere).unwrap();
        fs::remove_file(&receipt).unwrap();
        symlink(&elsewhere, &receipt).unwrap();
        assert_eq!(try_undo(&fixture, &journal, "link"), (false, None));
        // A folder in its place.
        fs::remove_file(&receipt).unwrap();
        fs::create_dir(&receipt).unwrap();
        assert_eq!(try_undo(&fixture, &journal, "folder"), (false, None));
        assert!(fixture.destination_root().join(INSTALLED).exists());
    }

    #[test]
    fn an_install_leaves_its_operation_receipt_before_it_touches_anything() {
        let fixture = Fixture::new("operation-receipt");
        let journal = honest_install(&fixture, "operation-receipt");
        let receipt = fixture
            .backup_root()
            .join("operation-receipt")
            .join(OPERATION_RECEIPT_NAME);
        let held = fs::read_to_string(&receipt).unwrap();
        assert!(held.starts_with("1:"), "{held}");
        // It is what the record works out to, and stops being so the moment
        // the record says anything else about the install.
        assert!(
            operation_receipt_violation(
                &read_journal(&journal).unwrap(),
                &journal,
                &fixture.backup_root()
            )
            .is_none()
        );
        // Without it, an install that kept receipts is not believed.
        fs::remove_file(&receipt).unwrap();
        assert_eq!(
            try_undo(&fixture, &journal, "no-operation-receipt"),
            (false, None)
        );
        assert!(fixture.destination_root().join(INSTALLED).exists());

        // A dry run leaves nothing.
        let fixture = Fixture::new("operation-receipt-dry");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "operation-receipt-dry", true, true, false),
        );
        assert!(!fixture.backup_root().join("operation-receipt-dry").exists());

        // An install interrupted before its first file still left it.
        let fixture = Fixture::new("operation-receipt-early");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::Rename));
        execute_shared_apply(
            &plan,
            &options(
                &fixture,
                &plan,
                "operation-receipt-early",
                false,
                true,
                false,
            ),
        );
        inject_fault(None);
        assert!(
            fixture
                .backup_root()
                .join("operation-receipt-early")
                .join(OPERATION_RECEIPT_NAME)
                .is_file()
        );
    }

    /// The operation receipt is kept for as long as an install can be
    /// undone, and is worked out again from the record at every Undo. This
    /// pins what goes into it. If this test has to change, installs made
    /// before the change would all be refused Undo: give the new layout a
    /// new [`OPERATION_RECEIPT_FORMAT`] and keep reading the old one.
    #[test]
    fn what_goes_into_an_operation_receipt_does_not_drift() {
        let path = |text: &str| SharedTransactionPath::from_path(Path::new(text));
        let context = SharedApplyContext {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: path("/roms/game.zip"),
            verified_game_identity: "archive-1".into(),
            profile_id: "retroarch-native".into(),
            source_mode: "trusted-catalogue".into(),
        };
        let entry = SharedPlanEntry {
            adapter: PreviewAdapter::RetroArch,
            selected_archive: path("/roms/game.zip"),
            verified_game_identity: "archive-1".into(),
            source_path: path("/staged/game.cht"),
            source_digest: "aa".into(),
            destination_root: path("/cheats"),
            destination_relative_path: path("NES/game.cht"),
            destination_pre_state: PreviewDestinationState::Missing,
            destination_pre_digest: None,
            proposed_action: PreviewProposedAction::Install,
            backup_required: false,
            parent_creation_approved: true,
            content_verification: None,
        };
        let document = operation_identity_document(
            "op-1",
            &context,
            &path("/staged"),
            &path("/cheats"),
            "plan-1",
            &[&entry],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(document).unwrap(),
            concat!(
                r#"[1,"op-1","#,
                r#"{"adapter":"retro_arch","#,
                r#""selected_archive":{"display":"/roms/game.zip","unix_bytes_hex":"2f726f6d732f67616d652e7a6970"},"#,
                r#""verified_game_identity":"archive-1","profile_id":"retroarch-native","source_mode":"trusted-catalogue"},"#,
                r#"{"display":"/staged","unix_bytes_hex":"2f737461676564"},"#,
                r#"{"display":"/cheats","unix_bytes_hex":"2f636865617473"},"#,
                r#""plan-1","#,
                r#"[{"adapter":"retro_arch","#,
                r#""selected_archive":{"display":"/roms/game.zip","unix_bytes_hex":"2f726f6d732f67616d652e7a6970"},"#,
                r#""verified_game_identity":"archive-1","#,
                r#""source_path":{"display":"/staged/game.cht","unix_bytes_hex":"2f7374616765642f67616d652e636874"},"#,
                r#""source_digest":"aa","#,
                r#""destination_root":{"display":"/cheats","unix_bytes_hex":"2f636865617473"},"#,
                r#""destination_relative_path":{"display":"NES/game.cht","unix_bytes_hex":"4e45532f67616d652e636874"},"#,
                r#""destination_pre_state":"missing","destination_pre_digest":null,"#,
                r#""proposed_action":"install","backup_required":false,"#,
                r#""parent_creation_approved":true,"content_verification":null}]]"#,
            )
        );
    }

    #[test]
    fn a_receipt_that_is_a_link_is_not_accepted() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("receipt-link");
        let journal = honest_install(&fixture, "receipt-link");
        let receipt = receipt_for(&fixture, "receipt-link", INSTALLED);
        // A link in the receipt's place, to a file holding the right
        // digest. It is not followed.
        let elsewhere = fixture.0.join("elsewhere");
        fs::write(&elsewhere, digest_bytes(b"new")).unwrap();
        fs::remove_file(&receipt).unwrap();
        symlink(&elsewhere, &receipt).unwrap();
        assert_eq!(try_undo(&fixture, &journal, "receipt-link"), (false, None));
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"new"
        );
    }

    #[test]
    fn a_record_cannot_claim_someones_own_file_inside_the_root_however_recently_it_was_made() {
        let fixture = Fixture::new("forged-inside-root");
        let journal = honest_install(&fixture, "forged-inside-root");
        // Someone's own file inside the destination root, created this
        // moment, holding the same bytes the install wrote. The record is
        // rewritten straight away to claim it.
        let mine = fixture.destination_root().join("Nintendo - NES/mine.cht");
        fs::write(&mine, b"new").unwrap();
        rewrite_journal(&journal, |journal| {
            journal.entries[0].plan_entry.destination_relative_path =
                SharedTransactionPath::from_path(Path::new("Nintendo - NES/mine.cht"));
        });
        let recorded = fs::read(&journal).unwrap();

        let outcome = try_undo(&fixture, &journal, "inside");
        assert_eq!(
            fs::read(&mine).unwrap_or_default(),
            b"new",
            "a file EmuWiz did not install was removed on a record's word \
             (offered: {}, status: {:?})",
            outcome.0,
            outcome.1
        );
        assert!(
            !outcome.0,
            "Undo was offered for a file the record cannot own"
        );
        // Refused, not repaired: the record is exactly as it was, and what
        // the install really wrote is untouched, with its receipt.
        assert_eq!(fs::read(&journal).unwrap(), recorded);
        assert!(fixture.destination_root().join(INSTALLED).exists());
        assert!(receipt_for(&fixture, "forged-inside-root", INSTALLED).exists());

        // The preview says why, for the person looking at it: this is no
        // longer the record of the install EmuWiz kept under this name.
        let preview = preview_shared_rollback(
            &journal,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert_eq!(
            preview.entries[0].outcome,
            SharedRollbackOutcome::JournalMalformed
        );
        assert!(
            preview.entries[0]
                .failure
                .as_ref()
                .is_some_and(|failure| failure.detail.contains("not the record of the install")),
            "{:?}",
            preview.entries[0].failure
        );
    }

    #[test]
    fn a_receipt_for_other_bytes_or_another_operation_does_not_make_a_file_the_records() {
        // The receipt says other bytes were installed at this path.
        let fixture = Fixture::new("receipt-other-bytes");
        let journal = honest_install(&fixture, "receipt-other-bytes");
        let receipt = receipt_for(&fixture, "receipt-other-bytes", INSTALLED);
        fs::write(&receipt, digest_bytes(b"something else")).unwrap();
        assert_eq!(try_undo(&fixture, &journal, "other-bytes"), (false, None));
        assert!(fixture.destination_root().join(INSTALLED).exists());

        // The record claims to be another operation: that operation has no
        // receipt for this file.
        let fixture = Fixture::new("receipt-other-operation");
        let journal = honest_install(&fixture, "receipt-other-operation");
        rewrite_journal(&journal, |journal| {
            journal.operation_id = "some-other-operation".into();
        });
        assert_eq!(
            try_undo(&fixture, &journal, "other-operation"),
            (false, None)
        );
        assert!(fixture.destination_root().join(INSTALLED).exists());
    }

    #[test]
    fn a_record_from_before_receipts_existed_is_refused_and_everything_is_kept() {
        // An install made by an earlier EmuWiz left no receipt for its new
        // files. Nothing but the record says they are its own, so Undo is
        // not offered, and the files and the record stay.
        let fixture = Fixture::new("legacy-new");
        let journal = honest_install(&fixture, "legacy-new");
        forget_receipts(&fixture, "legacy-new");
        let recorded = fs::read(&journal).unwrap();
        assert_eq!(try_undo(&fixture, &journal, "legacy-new"), (false, None));
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"new"
        );
        assert_eq!(fs::read(&journal).unwrap(), recorded);

        // A replaced file from such an install is still restored: its
        // backup has always been kept, where Undo expects it, and there
        // never was a receipt beside it.
        let fixture = Fixture::new("legacy-replace");
        let journal = honest_replacement(&fixture, "legacy-replace");
        forget_receipts(&fixture, "legacy-replace");
        assert_eq!(
            try_undo(&fixture, &journal, "legacy-replace"),
            (true, Some(SharedApplyStatus::Success))
        );
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"old"
        );
    }

    #[test]
    fn a_backup_that_is_not_this_operations_own_is_not_restored() {
        let fixture = Fixture::new("forged-backup-inside");
        let journal = honest_replacement(&fixture, "forged-backup-inside");
        // A plain file inside the backup folder, as another operation's
        // backup would be. It is not where this operation's backup of this
        // file is kept.
        let other = fixture.backup_root().join("another-operation");
        fs::create_dir(&other).unwrap();
        let planted = other.join("planted.bak");
        fs::write(&planted, b"not the original").unwrap();
        rewrite_journal(&journal, |journal| {
            journal.entries[0].backup_path = Some(SharedTransactionPath::from_path(&planted));
            journal.entries[0].backup_digest = Some(digest_bytes(b"not the original"));
        });
        let outcome = try_undo(&fixture, &journal, "forged-backup-inside");
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"new",
            "Undo wrote another backup's bytes (offered: {}, status: {:?})",
            outcome.0,
            outcome.1
        );
        assert!(!outcome.0);
    }

    #[test]
    fn a_rewritten_record_cannot_put_the_original_back_over_a_file_changed_since() {
        let fixture = Fixture::new("forged-installed-digest");
        let journal = honest_replacement(&fixture, "forged-installed-digest");
        let live = fixture.destination_root().join(INSTALLED);
        // The file is changed after the install. Undo is refused: it no
        // longer holds what EmuWiz installed.
        fs::write(&live, b"changed since").unwrap();
        assert_eq!(try_undo(&fixture, &journal, "honest"), (false, None));
        // The record is rewritten to say that this is what was installed.
        // The backup is genuine and is this operation's own; that is not
        // enough. EmuWiz's receipt says what it installed, and it was not
        // this.
        rewrite_journal(&journal, |journal| {
            journal.entries[0].final_destination_digest = Some(digest_bytes(b"changed since"));
        });
        let preview = preview_shared_rollback(
            &journal,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert_eq!(
            preview.entries[0].outcome,
            SharedRollbackOutcome::JournalMalformed
        );
        assert_eq!(try_undo(&fixture, &journal, "forged"), (false, None));
        assert_eq!(fs::read(&live).unwrap(), b"changed since");
    }

    /// The limit that remains, for installs from before these receipts
    /// existed. Their backups have no receipt beside them, so what was
    /// installed over the file is still only the record's word: a rewritten
    /// record can have Undo put the genuine original back over a file that
    /// has changed since. Refusing instead would make every backup made by
    /// an earlier EmuWiz unusable.
    #[test]
    fn for_an_install_from_before_receipts_a_rewritten_record_can_still_restore_over_a_changed_file()
     {
        let fixture = Fixture::new("legacy-forged-installed-digest");
        let journal = honest_replacement(&fixture, "legacy-forged-installed-digest");
        forget_receipts(&fixture, "legacy-forged-installed-digest");
        let live = fixture.destination_root().join(INSTALLED);
        fs::write(&live, b"changed since").unwrap();
        assert_eq!(try_undo(&fixture, &journal, "honest"), (false, None));
        rewrite_journal(&journal, |journal| {
            journal.entries[0].final_destination_digest = Some(digest_bytes(b"changed since"));
        });
        assert_eq!(
            try_undo(&fixture, &journal, "forged"),
            (true, Some(SharedApplyStatus::Success))
        );
        // What comes back is the original EmuWiz itself kept, from this
        // operation's own backup of this file: never bytes a record names.
        assert_eq!(fs::read(&live).unwrap(), b"old");
    }

    #[test]
    fn an_honest_replacement_is_undone_and_its_backup_is_kept() {
        let fixture = Fixture::new("replace-honest");
        let journal = honest_replacement(&fixture, "replace-honest");
        let live = fixture.destination_root().join(INSTALLED);
        let receipt = replacement_receipt_for(&fixture, "replace-honest", INSTALLED);
        let backup = operation_evidence_path(
            &fixture.backup_root(),
            "replace-honest",
            &live,
            BACKUP_SUFFIX,
        )
        .unwrap();
        assert_eq!(fs::read(&live).unwrap(), b"new");
        assert_eq!(fs::read(&receipt).unwrap(), digest_bytes(b"new").as_bytes());
        let recorded = fs::read(&journal).unwrap();
        assert_eq!(
            try_undo(&fixture, &journal, "replace-honest"),
            (true, Some(SharedApplyStatus::Success))
        );
        assert_eq!(fs::read(&live).unwrap(), b"old");
        // The backup and the record stay. The receipt went with what it
        // vouched for: EmuWiz's file is no longer there.
        assert_eq!(fs::read(&backup).unwrap(), b"old");
        assert_eq!(fs::read(&journal).unwrap(), recorded);
        assert!(!receipt.exists());
        // The same bytes put there again later are not this install's to
        // undo, though its record is put back as it was.
        fs::write(&live, b"new").unwrap();
        for entry in fs::read_dir(fixture.history_root()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() && path != journal {
                fs::remove_file(path).unwrap();
            }
        }
        assert_eq!(try_undo(&fixture, &journal, "replace-again"), (false, None));
        assert_eq!(fs::read(&live).unwrap(), b"new");
    }

    #[test]
    fn a_replacement_receipt_cannot_be_used_to_have_the_file_removed() {
        let fixture = Fixture::new("replace-as-new");
        let journal = honest_replacement(&fixture, "replace-as-new");
        let live = fixture.destination_root().join(INSTALLED);
        // The record is rewritten to say the file was installed where
        // there was none. The receipt it would need is the receipt of a
        // new file, which this operation never wrote.
        rewrite_journal(&journal, |journal| {
            let entry = &mut journal.entries[0];
            entry.outcome = SharedApplyOutcome::InstalledNew;
            entry.destination_existed_before_apply = Some(false);
            entry.backup_path = None;
            entry.backup_digest = None;
        });
        assert_eq!(
            try_undo(&fixture, &journal, "replace-as-new"),
            (false, None)
        );
        assert_eq!(fs::read(&live).unwrap(), b"new");
    }

    #[test]
    fn a_replacement_that_failed_leaves_no_receipt_and_its_backup_cannot_be_misused() {
        let fixture = Fixture::new("replace-failed");
        let report = preview(&fixture, b"new", Some(b"old"));
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::Rename));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "replace-failed", false, true, true),
        );
        inject_fault(None);
        assert_ne!(result.journal.status, SharedApplyStatus::Success);
        let live = fixture.destination_root().join(INSTALLED);
        assert_eq!(fs::read(&live).unwrap(), b"old", "nothing was replaced");
        // The backup was made before the failed publish. The receipt was
        // withdrawn with it: EmuWiz replaced nothing.
        let receipt = replacement_receipt_for(&fixture, "replace-failed", INSTALLED);
        assert!(!receipt.exists());
        let backup = operation_evidence_path(
            &fixture.backup_root(),
            "replace-failed",
            &live,
            BACKUP_SUFFIX,
        )
        .unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"old");
        // Later the file is the person's own. A record written to claim
        // that it is what this operation installed is refused: this install
        // kept receipts, and has none for a replacement here. The backup
        // does not come to look like an older install's.
        fs::write(&live, b"mine").unwrap();
        let journal = result
            .journal_path
            .clone()
            .unwrap_or_else(|| fixture.history_root().join("replace-failed.json"));
        let mut forged = result.journal.clone();
        {
            let entry = &mut forged.entries[0];
            entry.outcome = SharedApplyOutcome::ReplacedExisting;
            entry.verification_succeeded = true;
            entry.publication = SharedPublicationState::Published;
            entry.failures.clear();
            entry.destination_existed_before_apply = Some(true);
            entry.final_destination_digest = Some(digest_bytes(b"mine"));
            entry.backup_path = Some(SharedTransactionPath::from_path(&backup));
            entry.backup_digest = Some(digest_bytes(b"old"));
        }
        forged.status = SharedApplyStatus::Success;
        fs::create_dir_all(fixture.history_root()).unwrap();
        fs::write(&journal, serde_json::to_vec_pretty(&forged).unwrap()).unwrap();
        assert_eq!(
            try_undo(&fixture, &journal, "replace-failed"),
            (false, None)
        );
        assert_eq!(fs::read(&live).unwrap(), b"mine");
    }

    #[test]
    fn a_replacement_receipt_that_is_not_a_plain_file_refuses_undo() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("replace-receipt-link");
        let journal = honest_replacement(&fixture, "replace-receipt-link");
        let live = fixture.destination_root().join(INSTALLED);
        let receipt = replacement_receipt_for(&fixture, "replace-receipt-link", INSTALLED);
        // The receipt is swapped for a link to a file holding the right
        // digest. It is not followed.
        let elsewhere = fixture.0.join("elsewhere");
        fs::write(&elsewhere, digest_bytes(b"new")).unwrap();
        fs::remove_file(&receipt).unwrap();
        symlink(&elsewhere, &receipt).unwrap();
        assert_eq!(try_undo(&fixture, &journal, "link"), (false, None));
        assert_eq!(fs::read(&live).unwrap(), b"new");
        // A folder in its place is no receipt either.
        fs::remove_file(&receipt).unwrap();
        fs::create_dir(&receipt).unwrap();
        assert_eq!(try_undo(&fixture, &journal, "folder"), (false, None));
        assert_eq!(fs::read(&live).unwrap(), b"new");
    }

    #[test]
    fn an_interrupted_install_recovered_long_afterwards_is_still_undone() {
        // Crash part-way through three new files; nothing but the pending
        // record survives. Recovery is not run until much later.
        let fixture = Fixture::new("recovered-later");
        let report = multi_preview(&fixture, 3);
        let plan = make_plan(&fixture, &report);
        inject_fault_after(FaultPoint::ApplyCheckpoint, 1);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "recovered-later", false, true, false),
        );
        inject_fault(None);
        assert!(result.journal_path.is_none());
        let installed = [
            fixture.destination_root().join("Nintendo - NES/game-0.cht"),
            fixture.destination_root().join("Nintendo - NES/game-1.cht"),
        ];
        // Ownership does not depend on when anything happened: make the
        // files look ten days old before recovery writes its record now.
        let ten_days_ago = SystemTime::now() - Duration::from_secs(10 * 24 * 60 * 60);
        for file in &installed {
            assert!(file.exists());
            File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(ten_days_ago)
                .unwrap();
        }
        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        assert_eq!(recovered.len(), 1);
        let journal_path = recovered[0]
            .journal_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        let preview = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(preview.available, "{:?}", preview.entries);
        let undo = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "recovered-later-undo".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(undo.status, SharedApplyStatus::Success);
        for file in &installed {
            assert!(!file.exists(), "{} was left behind", file.display());
        }
    }

    #[test]
    fn an_honest_journal_is_still_undone_in_full() {
        // A new file: removed, with the folder the install made for it.
        let fixture = Fixture::new("honest-new");
        let journal = honest_install(&fixture, "honest-new");
        let outcome = try_undo(&fixture, &journal, "honest-new");
        assert_eq!(outcome, (true, Some(SharedApplyStatus::Success)));
        assert!(!fixture.destination_root().join(INSTALLED).exists());
        assert!(!fixture.destination_root().join("Nintendo - NES").exists());
        assert!(fixture.destination_root().is_dir());

        // A replaced file: the original bytes are back.
        let fixture = Fixture::new("honest-replace");
        let journal = honest_replacement(&fixture, "honest-replace");
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"new"
        );
        let outcome = try_undo(&fixture, &journal, "honest-replace");
        assert_eq!(outcome, (true, Some(SharedApplyStatus::Success)));
        assert_eq!(
            fs::read(fixture.destination_root().join(INSTALLED)).unwrap(),
            b"old"
        );
    }

    // --- malformed / out-of-order / out-of-root created-root chain ---------

    #[test]
    fn an_out_of_order_created_root_chain_is_rejected() {
        let fixture = Fixture::new("bootstrap-malformed-out-of-order");
        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "malformed-order", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        let journal_path = result.journal_path.unwrap();

        let mut tampered = read_journal(&journal_path).unwrap();
        tampered.created_root_directories.reverse();
        fs::write(&journal_path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();

        let rollback = preview_shared_rollback(&journal_path, &root, &fixture.backup_root());
        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "malformed-order-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        assert!(
            root.exists(),
            "a reordered chain must be rejected wholesale"
        );
    }

    #[test]
    fn a_created_root_chain_pointing_outside_the_destination_root_is_rejected() {
        let fixture = Fixture::new("bootstrap-malformed-out-of-root");
        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "malformed-out-of-root", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        let journal_path = result.journal_path.unwrap();

        let outside = fixture.0.join("somewhere-else");
        fs::create_dir(&outside).unwrap();
        let mut tampered = read_journal(&journal_path).unwrap();
        // Point the final (leaf) entry at a directory entirely outside the
        // real chain, while still (dishonestly) claiming to be the
        // transaction's own destination root.
        let last = tampered.created_root_directories.last_mut().unwrap();
        last.path = SharedTransactionPath::from_path(&outside);
        fs::write(&journal_path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();

        let rollback = preview_shared_rollback(&journal_path, &root, &fixture.backup_root());
        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "malformed-out-of-root-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        assert!(
            outside.is_dir(),
            "a chain that does not terminate at the real destination root must never be acted on"
        );
    }

    // --- symlink ancestor swapped in before creation ------------------------

    #[cfg(unix)]
    #[test]
    fn an_ancestor_swapped_to_a_symlink_before_creation_is_rejected() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("bootstrap-symlink-ancestor");
        let dolphin = fixture.0.join("dolphin");
        let root = dolphin.join("Load").join("Textures");
        // Preview/plan are built while `dolphin` genuinely does not exist
        // yet - a real, honest approval. Only *after* that (the real
        // caller's own earlier check) does `dolphin` get swapped to a
        // symlink, simulating a race that lands squarely between that
        // earlier check and `execute_shared_apply`'s own bootstrap.
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let real_target = fixture.0.join("real-target");
        fs::create_dir(&real_target).unwrap();
        symlink(&real_target, &dolphin).unwrap();

        let link = dolphin;
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "symlink-ancestor", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Failed);
        assert!(
            !real_target.join("Load").exists(),
            "nothing may be created through a symlinked ancestor, even one whose target is safe"
        );
        assert!(link.is_symlink(), "the symlink itself must be untouched");
    }

    // --- creation cannot escape through a swapped ancestor ------------------

    #[cfg(unix)]
    #[test]
    fn creation_cannot_escape_through_an_intermediate_symlinked_ancestor() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("bootstrap-escape-attempt");
        // A directory well outside the destination root's own lineage -
        // proves creation never lands here even though the symlink below
        // (introduced after preview/plan approval) points straight at it.
        let escape_target = fixture.0.join("escape-target");
        fs::create_dir(&escape_target).unwrap();
        let dolphin = fixture.0.join("dolphin");
        let load_link = dolphin.join("Load");
        let root = load_link.join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        // Only now does `Load` become a symlink pointing outside `dolphin`
        // entirely - after the real approval, before the real apply. A
        // pathname-only re-check between validation and creation would
        // have happily walked straight through it.
        fs::create_dir(&dolphin).unwrap();
        symlink(&escape_target, &load_link).unwrap();

        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "escape-attempt", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Failed);
        assert!(
            !escape_target.join("Textures").exists(),
            "creation must never be redirected through a symlinked intermediate ancestor"
        );
        assert!(
            load_link.is_symlink(),
            "the symlink itself must be untouched"
        );
    }

    // --- partial-chain failure cleans only this attempt's own directories --

    #[test]
    fn a_parent_creation_race_cleans_up_only_the_partial_chain_this_attempt_made() {
        let fixture = Fixture::new("bootstrap-race-cleanup");
        // A directory that already existed before this transaction, right
        // next to where the transaction's own chain will be created -
        // proves cleanup never reaches sideways into it.
        let sibling = fixture.0.join("sibling-pre-existing");
        fs::create_dir(&sibling).unwrap();
        fs::write(sibling.join("keep.txt"), b"unrelated").unwrap();

        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::ParentCreationRace));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "race-cleanup", false, true, false),
        );
        inject_fault(None);
        assert_eq!(result.journal.status, SharedApplyStatus::Failed);
        assert!(
            !fixture.0.join("dolphin").exists(),
            "a chain that failed partway through must leave nothing behind, not even the first \
             level"
        );
        assert!(
            sibling.is_dir(),
            "an unrelated pre-existing directory must be untouched"
        );
        assert!(sibling.join("keep.txt").is_file());
    }

    // --- all entries fail after bootstrap -----------------------------------

    #[test]
    fn all_entries_failing_after_bootstrap_leaves_no_unreachable_empty_root() {
        let fixture = Fixture::new("bootstrap-all-fail-cleanup");
        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::SourceMutation));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "all-fail-cleanup", false, true, false),
        );
        inject_fault(None);
        assert_eq!(result.journal.status, SharedApplyStatus::Failed);
        assert_eq!(
            result.journal.entries[0].outcome,
            SharedApplyOutcome::SourceChanged
        );
        // A journal was still written (the lock was held, entries were
        // attempted) - but since nothing was actually installed, the
        // freshly bootstrapped, still-empty root was cleaned up in the
        // same call rather than left as an empty directory a journal
        // claims but normal rollback (which requires at least one
        // `Available` per-file entry) could never reach.
        assert!(result.journal_path.is_some());
        assert!(!root.exists());
        assert!(!fixture.0.join("dolphin").join("Load").exists());
        assert!(!fixture.0.join("dolphin").exists());
        assert!(result.journal.created_root_directories.is_empty());
    }

    // --- journal-write failure with zero successful installs ---------------

    #[test]
    fn journal_write_failure_with_zero_successful_installs_still_cleans_up_safely() {
        let fixture = Fixture::new("bootstrap-journal-fail-zero-writes");
        let root = fixture.0.join("dolphin").join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        // A real (non-injected) reason for zero successful writes: the
        // source no longer matches what the plan approved.
        fs::write(fixture.source_root().join("game.cht"), b"tampered").unwrap();
        inject_fault(Some(FaultPoint::JournalWrite));
        let result = execute_shared_apply(
            &plan,
            &options(
                &fixture,
                &plan,
                "journal-fail-zero-writes",
                false,
                true,
                false,
            ),
        );
        inject_fault(None);
        assert_eq!(result.journal.status, SharedApplyStatus::Failed);
        assert!(
            result.journal_path.is_none(),
            "the journal write itself failed"
        );
        assert!(
            !root.exists(),
            "zero installs means the bootstrap-created root is safely removable even though the \
             journal recording that never made it to disk"
        );
        assert!(!fixture.0.join("dolphin").exists());
    }

    #[test]
    fn a_parent_creation_race_is_rejected_and_the_partial_chain_is_cleaned_up() {
        let fixture = Fixture::new("bootstrap-helper-race-cleanup");
        let root = fixture.0.join("a").join("b").join("c");
        inject_fault(Some(FaultPoint::ParentCreationRace));
        let result = bootstrap_missing_destination_root(&root);
        inject_fault(None);
        assert!(matches!(
            result,
            Err(SharedApplyFailureKind::ParentCreationFailed)
        ));
        assert!(
            !fixture.0.join("a").exists(),
            "a chain that failed partway through must leave nothing behind, not even the first \
             level"
        );
    }

    #[test]
    fn target_becomes_a_file_before_locking_is_rejected() {
        let fixture = Fixture::new("bootstrap-target-becomes-file");
        let dolphin = fixture.0.join("dolphin");
        fs::create_dir(&dolphin).unwrap();
        // The exact destination root path is a regular file, not a
        // directory - the same shape a symlink-replacement race would
        // leave behind at the instant this is (re)validated.
        let root = dolphin.join("Load");
        fs::write(&root, b"not a directory").unwrap();
        let result = bootstrap_missing_destination_root(&root);
        assert!(matches!(
            result,
            Err(SharedApplyFailureKind::DestinationUnsafe)
        ));
        assert!(root.is_file(), "the existing file must be left untouched");
    }

    #[test]
    fn old_journal_without_the_new_field_still_loads_and_never_gains_root_cleanup_authority() {
        let fixture = Fixture::new("bootstrap-old-journal-compat");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "old-journal", false, true, false),
        );
        let journal_path = result.journal_path.unwrap();

        // Simulate a journal written before `created_root_directories`
        // existed: strip the field out of the persisted JSON entirely.
        let bytes = fs::read(&journal_path).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("created_root_directories");
        fs::write(&journal_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let loaded = read_journal(&journal_path).unwrap();
        assert!(loaded.created_root_directories.is_empty());

        let rollback = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(rollback.available);
        let rolled_back = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "old-journal-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        assert!(
            !fixture
                .destination_root()
                .join("Nintendo - NES/game.cht")
                .exists()
        );
        // The pre-existing destination root itself (never created by this
        // transaction, and absent from the old journal by construction)
        // must never be removed - identical to `install_new_is_atomic_
        // journaled_and_rollback_is_bound_and_idempotent`'s own assertion.
        assert!(fixture.destination_root().is_dir());
    }
}

/// Night Watch Hard Job #3: crash, interruption and restart recovery for
/// Blue's shared cheat/mod transaction engine, and S04-05 (Undo deletes an
/// installed file without a fresh check). Every scenario checks exact bytes
/// before and after, never only a status.
#[cfg(test)]
mod crash_recovery_tests {
    use super::tests_support::*;
    use super::*;

    fn rollback_options(fixture: &Fixture, id: &str, preview_id: &str) -> SharedRollbackOptions {
        SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview_id.to_owned(),
                approved: true,
            },
            rollback_operation_id: id.into(),
            timestamp_unix_seconds: 1_700_000_002,
            history_root: fixture.history_root(),
            backup_root: fixture.backup_root(),
        }
    }

    fn destination(fixture: &Fixture, name: &str) -> PathBuf {
        fixture.destination_root().join("Nintendo - NES").join(name)
    }

    fn history_files(fixture: &Fixture) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(fixture.history_root())
            .map(|dir| {
                dir.flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn platform_dir_residue(fixture: &Fixture) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(fixture.destination_root().join("Nintendo - NES"))
                .map(|dir| {
                    dir.flatten()
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .filter(|name| name.starts_with('.'))
                        .collect()
                })
                .unwrap_or_default();
        names.sort();
        names
    }

    /// Crash between the first and second of three writes, before any
    /// terminal journal: recovery records exactly which files landed, Undo
    /// removes them and leaves the never-written one alone.
    #[test]
    fn crash_between_writes_is_settled_and_undo_restores_exact_state() {
        let fixture = Fixture::new("crash-between-writes");
        let report = multi_preview(&fixture, 3);
        let plan = make_plan(&fixture, &report);
        inject_fault_after(FaultPoint::ApplyCheckpoint, 1);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "crash-between", false, true, false),
        );
        inject_fault(None);
        assert!(result.journal_path.is_none(), "no receipt was written");
        assert_eq!(
            fs::read(destination(&fixture, "game-0.cht")).unwrap(),
            b"new-0"
        );
        assert_eq!(
            fs::read(destination(&fixture, "game-1.cht")).unwrap(),
            b"new-1"
        );
        assert!(!destination(&fixture, "game-2.cht").exists());
        assert!(
            discover_shared_apply_history(&fixture.history_root())
                .journals
                .is_empty(),
            "before recovery History cannot see the interrupted install"
        );

        // "Restart": nothing in memory, only the pending record on disk.
        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].outcome,
            SharedInterruptedApplyOutcome::SettledForUndo
        );
        assert_eq!(
            recovered[0].entry_states,
            vec![
                SharedEntryRecoveryState::Applied,
                SharedEntryRecoveryState::Applied,
                SharedEntryRecoveryState::Planned
            ]
        );
        assert_eq!(
            history_files(&fixture),
            vec!["crash-between.json".to_string()]
        );
        assert!(discover_pending_operations(&fixture.history_root()).is_empty());
        // Idempotent: a second pass finds nothing to settle.
        assert!(
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root())
                .is_empty()
        );

        let history = discover_shared_apply_history(&fixture.history_root());
        assert_eq!(history.journals.len(), 1);
        let journal = &history.journals[0].1;
        assert_eq!(journal.status, SharedApplyStatus::PartialFailure);
        assert_eq!(journal.entries[0].outcome, SharedApplyOutcome::InstalledNew);
        assert_eq!(journal.entries[1].outcome, SharedApplyOutcome::InstalledNew);
        assert_eq!(journal.entries[2].outcome, SharedApplyOutcome::WriteFailed);
        assert!(
            journal.entries[0]
                .stages
                .contains(&SharedTransactionStage::JournalFailedAfterSuccessfulWrite)
        );

        let journal_path = history.journals[0].0.to_path_buf().unwrap();
        let preview = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(preview.available, "{:?}", preview.entries);
        assert_eq!(
            preview.entries[2].outcome,
            SharedRollbackOutcome::NoChangeRequired
        );
        let undo = execute_shared_rollback(
            &preview,
            &rollback_options(&fixture, "crash-between-undo", &preview.preview_id),
        );
        assert_eq!(undo.status, SharedApplyStatus::Success);
        assert!(!destination(&fixture, "game-0.cht").exists());
        assert!(!destination(&fixture, "game-1.cht").exists());
        assert!(!destination(&fixture, "game-2.cht").exists());
        assert!(
            platform_dir_residue(&fixture).is_empty(),
            "no hidden residue"
        );
    }

    /// Crash immediately after the replacement was published but before the
    /// receipt: recovery proves the backup and Undo restores the user's
    /// original bytes exactly.
    #[test]
    fn crash_after_replacement_is_settled_and_undo_restores_original_bytes() {
        let fixture = Fixture::new("crash-after-replace");
        let report = preview(&fixture, b"catalogue cheats\n", Some(b"USER EDITED\n"));
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::TerminalJournal));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "crash-replace", false, true, true),
        );
        inject_fault(None);
        assert!(result.journal_path.is_none());
        let file = destination(&fixture, "game.cht");
        assert_eq!(fs::read(&file).unwrap(), b"catalogue cheats\n");

        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].outcome,
            SharedInterruptedApplyOutcome::SettledForUndo
        );
        let journal_path = recovered[0]
            .journal_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        let journal = read_journal(&journal_path).unwrap();
        assert_eq!(
            journal.entries[0].outcome,
            SharedApplyOutcome::ReplacedExisting
        );
        assert_eq!(
            journal.entries[0].backup_digest.as_deref(),
            Some(digest_bytes(b"USER EDITED\n").as_str())
        );
        assert_ne!(journal.status, SharedApplyStatus::Success);

        let preview = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(preview.available, "{:?}", preview.entries);
        let undo = execute_shared_rollback(
            &preview,
            &rollback_options(&fixture, "crash-replace-undo", &preview.preview_id),
        );
        assert_eq!(undo.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(&file).unwrap(), b"USER EDITED\n");
    }

    /// A pending record whose destination root is locked belongs to an apply
    /// that is running right now: recovery leaves it completely alone.
    #[test]
    fn recovery_leaves_a_running_apply_alone() {
        let fixture = Fixture::new("recovery-live-lock");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::TerminalJournal));
        let _ = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "live-lock", false, true, false),
        );
        inject_fault(None);
        let lock = RootLock::acquire(&fixture.destination_root(), Duration::ZERO).unwrap();
        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].outcome,
            SharedInterruptedApplyOutcome::StillRunning
        );
        assert_eq!(
            history_files(&fixture),
            vec!["live-lock.pending.json".to_string()]
        );
        drop(lock);
        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        assert_eq!(
            recovered[0].outcome,
            SharedInterruptedApplyOutcome::SettledForUndo
        );
    }

    /// The destination was edited after the interrupted install: recovery
    /// records the file as unexplained, Undo is blocked for the whole
    /// operation, and the edited bytes are never touched.
    #[test]
    fn edited_destination_after_crash_blocks_undo_and_is_preserved() {
        let fixture = Fixture::new("crash-then-edit");
        let report = multi_preview(&fixture, 2);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::TerminalJournal));
        let _ = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "crash-edit", false, true, false),
        );
        inject_fault(None);
        fs::write(
            destination(&fixture, "game-1.cht"),
            b"edited in RetroArch\n",
        )
        .unwrap();

        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        assert_eq!(
            recovered[0].outcome,
            SharedInterruptedApplyOutcome::SettledForUndo
        );
        assert_eq!(
            recovered[0].entry_states,
            vec![
                SharedEntryRecoveryState::Applied,
                SharedEntryRecoveryState::NeedsReview
            ]
        );
        let journal_path = recovered[0]
            .journal_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        let preview = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(!preview.available);
        assert_eq!(
            preview.entries[1].outcome,
            SharedRollbackOutcome::DestinationChanged
        );
        let undo = execute_shared_rollback(
            &preview,
            &rollback_options(&fixture, "crash-edit-undo", &preview.preview_id),
        );
        assert_eq!(undo.status, SharedApplyStatus::Failed);
        assert_eq!(
            fs::read(destination(&fixture, "game-0.cht")).unwrap(),
            b"new-0"
        );
        assert_eq!(
            fs::read(destination(&fixture, "game-1.cht")).unwrap(),
            b"edited in RetroArch\n"
        );
    }

    /// Recovery itself failing to write the receipt keeps the pending
    /// record and changes nothing else.
    #[test]
    fn recovery_journal_write_failure_keeps_the_pending_record() {
        let fixture = Fixture::new("recovery-write-fails");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::TerminalJournal));
        let _ = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "recovery-fail", false, true, false),
        );
        inject_fault(None);
        inject_fault(Some(FaultPoint::JournalWrite));
        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        inject_fault(None);
        assert_eq!(
            recovered[0].outcome,
            SharedInterruptedApplyOutcome::NeedsReview
        );
        assert_eq!(
            history_files(&fixture),
            vec!["recovery-fail.pending.json".to_string()]
        );
        assert_eq!(fs::read(destination(&fixture, "game.cht")).unwrap(), b"new");
        // A later pass succeeds.
        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        assert_eq!(
            recovered[0].outcome,
            SharedInterruptedApplyOutcome::SettledForUndo
        );
    }

    /// S04-05: the installed file is rewritten in the window between Undo's
    /// last check and the removal. The newer bytes survive in place; Undo
    /// reports the failure and leaves no hidden quarantine file behind.
    #[test]
    fn undo_never_deletes_a_file_rewritten_after_its_check() {
        let fixture = Fixture::new("undo-race-remove");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "undo-race", false, true, false),
        );
        let journal_path = applied.journal_path.unwrap();
        let preview = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(preview.available);
        let racing = b"cheats = 1\ncheat0_desc = \"EDITED IN RETROARCH\"\n".to_vec();
        let racing_for_hook = racing.clone();
        set_before_remove(Some(Box::new(move |path: &Path| {
            fs::write(path, &racing_for_hook).unwrap();
        })));
        let undo = execute_shared_rollback(
            &preview,
            &rollback_options(&fixture, "undo-race-undo", &preview.preview_id),
        );
        set_before_remove(None);
        assert_ne!(undo.status, SharedApplyStatus::Success);
        assert_eq!(
            undo.preview.entries[0].outcome,
            SharedRollbackOutcome::Failed
        );
        assert_eq!(
            undo.preview.entries[0].failure.as_ref().unwrap().kind,
            SharedApplyFailureKind::DestinationChanged
        );
        assert_eq!(fs::read(destination(&fixture, "game.cht")).unwrap(), racing);
        assert!(
            platform_dir_residue(&fixture).is_empty(),
            "no quarantine residue"
        );
        // The operation stays undone-able-by-hand: its receipt is still in History.
        assert!(journal_path.exists());
    }

    /// An Undo that dies after removing the first of two new files can be
    /// retried after restart: the removed file counts as done, the other is
    /// still removable, and the retry completes.
    #[test]
    fn undo_interrupted_between_entries_can_be_retried_after_restart() {
        let fixture = Fixture::new("undo-crash-retry");
        let report = multi_preview(&fixture, 2);
        let plan = make_plan(&fixture, &report);
        let applied = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "undo-crash", false, true, false),
        );
        let journal_path = applied.journal_path.unwrap();
        let preview = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        inject_fault(Some(FaultPoint::RollbackAfterMutation));
        let first = execute_shared_rollback(
            &preview,
            &rollback_options(&fixture, "undo-crash-1", &preview.preview_id),
        );
        inject_fault(None);
        assert_eq!(first.status, SharedApplyStatus::PartialFailure);
        assert!(!destination(&fixture, "game-0.cht").exists());
        assert_eq!(
            fs::read(destination(&fixture, "game-1.cht")).unwrap(),
            b"new-1"
        );
        assert!(!discover_pending_operations(&fixture.history_root()).is_empty());

        let retry = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert_eq!(
            retry.entries[0].outcome,
            SharedRollbackOutcome::NoChangeRequired
        );
        assert_eq!(retry.entries[1].outcome, SharedRollbackOutcome::Available);
        assert!(retry.available, "{:?}", retry.entries);
        let second = execute_shared_rollback(
            &retry,
            &rollback_options(&fixture, "undo-crash-2", &retry.preview_id),
        );
        assert_eq!(second.status, SharedApplyStatus::Success);
        assert!(!destination(&fixture, "game-1.cht").exists());
        assert!(discover_pending_operations(&fixture.history_root()).is_empty());
        assert!(platform_dir_residue(&fixture).is_empty());
    }

    /// Review H1: startup recovery running while an apply is between its
    /// last write and its receipt must treat the root as still in use
    /// (the apply holds the lock until the receipt is durable), so the
    /// apply's own receipt wins and nothing is settled as "interrupted".
    #[test]
    fn recovery_during_a_finishing_apply_leaves_it_to_the_apply() {
        use std::sync::{Arc, Mutex};
        let fixture = Fixture::new("recovery-vs-finishing-apply");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        let seen: Arc<Mutex<Vec<SharedInterruptedApplyRecovery>>> =
            Arc::new(Mutex::new(Vec::new()));
        let seen_in_hook = Arc::clone(&seen);
        let (history, backups) = (fixture.history_root(), fixture.backup_root());
        // Runs right after publication, inside the apply, before the
        // receipt: the exact window the review named.
        set_after_publish(Some(Box::new(move |_destination: &Path| {
            let recovered = recover_interrupted_shared_applies(&history, &backups);
            seen_in_hook.lock().unwrap().extend(recovered);
        })));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "finishing-apply", false, true, false),
        );
        set_after_publish(None);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "the pending record was visible to recovery");
        assert_eq!(seen[0].outcome, SharedInterruptedApplyOutcome::StillRunning);
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(
            result.journal_path.is_some(),
            "the apply's own receipt was written"
        );
        assert!(result.journal_failure.is_none());
        assert_eq!(
            history_files(&fixture),
            vec!["finishing-apply.json".to_string()]
        );
        assert!(discover_pending_operations(&fixture.history_root()).is_empty());
        let journal = read_journal(&fixture.history_root().join("finishing-apply.json")).unwrap();
        assert!(
            !journal.entries[0]
                .stages
                .contains(&SharedTransactionStage::JournalFailedAfterSuccessfulWrite),
            "a completed apply is never recorded as interrupted"
        );
    }

    /// Review M1: a crash during the temporary write leaves a stray
    /// publication file beside the destination. Recovery must fail closed
    /// for that folder, name the stray file, and block Undo until a person
    /// has looked; proven-applied bytes are left exactly as they are.
    #[test]
    fn stray_temporary_file_after_a_crash_is_named_and_blocks_undo() {
        let fixture = Fixture::new("stray-temp-after-crash");
        let report = preview(&fixture, b"new", None);
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::TerminalJournal));
        let _ = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "stray-temp", false, true, false),
        );
        inject_fault(None);
        let stray = destination(&fixture, ".archivefs-apply-4242-7");
        fs::write(&stray, b"half-written").unwrap();

        let recovered =
            recover_interrupted_shared_applies(&fixture.history_root(), &fixture.backup_root());
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].outcome,
            SharedInterruptedApplyOutcome::SettledForUndo
        );
        assert_eq!(
            recovered[0].entry_states,
            vec![SharedEntryRecoveryState::NeedsReview]
        );
        assert!(
            recovered[0].detail.contains(&stray.display().to_string()),
            "{}",
            recovered[0].detail
        );
        let journal_path = recovered[0]
            .journal_path
            .as_ref()
            .unwrap()
            .to_path_buf()
            .unwrap();
        let journal = read_journal(&journal_path).unwrap();
        let failure = &journal.entries[0].failures[0];
        assert_eq!(failure.kind, SharedApplyFailureKind::PublishedUnverified);
        assert!(
            failure.detail.contains(&stray.display().to_string()),
            "{}",
            failure.detail
        );
        assert!(failure.detail.contains("remove the temporary file by hand"));
        let preview = preview_shared_rollback(
            &journal_path,
            &fixture.destination_root(),
            &fixture.backup_root(),
        );
        assert!(!preview.available);
        assert_eq!(fs::read(destination(&fixture, "game.cht")).unwrap(), b"new");
        assert_eq!(
            fs::read(&stray).unwrap(),
            b"half-written",
            "the stray file is never removed"
        );
    }

    /// Failure before the first write (the pending record cannot be
    /// written) changes nothing and reports Failed, never Success.
    #[test]
    fn failure_before_first_write_changes_nothing() {
        let fixture = Fixture::new("fail-before-write");
        let report = preview(&fixture, b"new", Some(b"keep me\n"));
        let plan = make_plan(&fixture, &report);
        inject_fault(Some(FaultPoint::DurableJournalWrite));
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "fail-before", false, true, true),
        );
        inject_fault(None);
        assert_eq!(result.journal.status, SharedApplyStatus::Failed);
        assert!(result.journal_failure.is_some());
        assert_eq!(
            fs::read(destination(&fixture, "game.cht")).unwrap(),
            b"keep me\n"
        );
        assert!(!fixture.backup_root().exists());
        assert!(platform_dir_residue(&fixture).is_empty());
    }
}
