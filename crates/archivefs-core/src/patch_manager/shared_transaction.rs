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
};
use super::shared_preview::{
    PreviewAdapter, PreviewDestinationState, PreviewEligibility, PreviewProposedAction,
    SharedPreviewReport,
};
use crate::default_database_path;

pub const SHARED_APPLY_SCHEMA_VERSION: u32 = 1;
pub const SHARED_MAX_ENTRIES: usize = 128;
pub const SHARED_MAX_SOURCE_BYTES: u64 = 1024 * 1024;
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
enum FaultPoint {
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
}

#[cfg(test)]
thread_local! {
    static INJECTED_FAULT: std::cell::Cell<Option<FaultPoint>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn inject_fault(point: Option<FaultPoint>) {
    INJECTED_FAULT.with(|fault| fault.set(point));
}

#[cfg(test)]
fn should_inject(point: FaultPoint) -> bool {
    INJECTED_FAULT.with(|fault| fault.get() == Some(point))
}

#[cfg(not(test))]
fn should_inject(_point: FaultPoint) -> bool {
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
        | PreviewAdapter::Dolphin
        | PreviewAdapter::Xenia => SharedAdapterWriteSupport::ApplyAndRollback,
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
    pub temporary_path: Option<SharedTransactionPath>,
    pub final_destination_digest: Option<String>,
    pub created_directories: Vec<SharedTransactionPath>,
    pub replacement_approved: bool,
    pub verification_succeeded: bool,
    pub outcome: SharedApplyOutcome,
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
}

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

pub fn execute_shared_apply(
    plan: &SharedTransactionPlan,
    options: &SharedApplyOptions,
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
        let managed_overlap = roots_overlap(&options.history_root, &source_root)
            || roots_overlap(&options.history_root, &destination_root)
            || roots_overlap(&options.backup_root, &source_root)
            || roots_overlap(&options.backup_root, &destination_root);
        if operation.is_err() || duplicate || managed_overlap {
            let (kind, detail) = if duplicate {
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
    }
    let replacement_approved = confirmation.is_some_and(|value| value.replacement_approved);
    let mut written = 0_u64;
    let mut backup_bytes = 0_u64;
    for entry in &plan.entries {
        journal.entries.push(apply_one(
            entry,
            &source_root,
            &destination_root,
            &options.backup_root,
            &options.operation_id,
            effective_dry_run,
            replacement_approved,
            &mut written,
            &mut backup_bytes,
        ));
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
    drop(lock);
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
        return SharedApplyResult {
            journal,
            journal_path: None,
            journal_failure: None,
        };
    }
    match write_journal_once(&journal, &options.history_root) {
        Ok(path) => {
            log::info!(
                "shared apply {}: journal written to {}",
                journal.operation_id,
                path.display(),
            );
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
            SharedApplyResult {
                journal,
                journal_path: None,
                journal_failure: Some(error),
            }
        }
    }
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
) -> SharedApplyEntry {
    let mut result = SharedApplyEntry {
        plan_entry: plan.clone(),
        destination_existed_before_apply: None,
        destination_parent_existed_before_apply: None,
        observed_source_digest: None,
        observed_destination_digest: None,
        backup_path: None,
        backup_digest: None,
        temporary_path: None,
        final_destination_digest: None,
        created_directories: Vec::new(),
        replacement_approved,
        verification_succeeded: false,
        outcome: SharedApplyOutcome::SkippedNotEligible,
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
    let source_hash = match stable_hash(&source, SHARED_MAX_SOURCE_BYTES) {
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
    if source_hash.bytes > SHARED_MAX_TOTAL_WRITTEN_BYTES.saturating_sub(*written) {
        return fail_result(
            result,
            SharedApplyOutcome::WriteFailed,
            SharedApplyFailureKind::ResourceLimitReached,
            Some(&source),
            "transaction write-byte limit reached",
        );
    }
    let Some((category, filename)) = exactly_two_components(&relative) else {
        return fail_result(
            result,
            SharedApplyOutcome::SkippedConflict,
            SharedApplyFailureKind::DestinationUnsafe,
            None,
            "destination must contain exactly two normal components",
        );
    };
    let assessment = match assess_destination(destination_root, &category, &filename) {
        Ok(value) => value,
        Err(error) => {
            return fail_result(
                result,
                SharedApplyOutcome::DestinationChanged,
                SharedApplyFailureKind::DestinationUnsafe,
                Some(destination_root),
                &error.to_string(),
            );
        }
    };
    let destination = assessment.proposed_destination.path().to_path_buf();
    let parent = destination.parent().unwrap_or(destination_root);
    let current = if assessment.destination_state == DestinationState::RegularFile {
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
        PreviewProposedAction::Install => assessment.destination_state == DestinationState::Absent,
        PreviewProposedAction::Replace | PreviewProposedAction::Skip => {
            assessment.destination_state == DestinationState::RegularFile
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
            "destination state or digest changed since approved preview",
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
        if !plan.parent_creation_approved
            || !matches!(
                plan.adapter,
                PreviewAdapter::RetroArch
                    | PreviewAdapter::Pcsx2
                    | PreviewAdapter::Dolphin
                    | PreviewAdapter::Xenia
            )
        {
            return fail_result(
                result,
                SharedApplyOutcome::WriteFailed,
                SharedApplyFailureKind::ParentCreationFailed,
                Some(parent),
                "parent creation was not approved by preview and adapter contract",
            );
        }
        if let Err(kind) = create_one_parent(destination_root, parent) {
            return fail_result(
                result,
                SharedApplyOutcome::WriteFailed,
                kind,
                Some(parent),
                "approved destination parent could not be created safely",
            );
        }
        result
            .created_directories
            .push(SharedTransactionPath::from_path(parent));
    }
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
                result.stages.push(SharedTransactionStage::BackupCreated);
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
    match atomic_write(
        &source,
        &destination,
        &source_hash.digest,
        plan.proposed_action == PreviewProposedAction::Install,
    ) {
        Ok(temp) => {
            result.temporary_path = Some(SharedTransactionPath::from_path(&temp));
            match verify_entry_content(plan, &destination) {
                Ok(()) => {
                    result.final_destination_digest = Some(source_hash.digest);
                    result.verification_succeeded = true;
                    result.outcome = if plan.proposed_action == PreviewProposedAction::Install {
                        SharedApplyOutcome::InstalledNew
                    } else {
                        SharedApplyOutcome::ReplacedExisting
                    };
                    result.stages.push(SharedTransactionStage::Success);
                    *written += source_hash.bytes;
                }
                Err(detail) => {
                    let restore = restore_after_failed_verification(plan, &result, &destination);
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
            result = fail_result(
                result,
                if kind == SharedApplyFailureKind::VerificationFailed {
                    SharedApplyOutcome::VerificationFailed
                } else {
                    SharedApplyOutcome::WriteFailed
                },
                kind,
                Some(&destination),
                "atomic destination write failed",
            );
        }
    }
    result
}

fn verify_entry_content(plan: &SharedPlanEntry, destination: &Path) -> Result<(), String> {
    let Some(contract) = &plan.content_verification else {
        return Ok(());
    };
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
    }
}

fn restore_after_failed_verification(
    plan: &SharedPlanEntry,
    result: &SharedApplyEntry,
    destination: &Path,
) -> Result<(), SharedApplyFailureKind> {
    if plan.proposed_action == PreviewProposedAction::Install {
        return remove_and_verify_new_destination(destination);
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
    atomic_write(&backup, destination, digest, false)
        .map(|_| ())
        .map_err(|(kind, _)| kind)
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
                }],
                available: false,
            };
        }
    };
    let journal_root = journal.destination_root.to_path_buf().ok();
    let root_matches = journal_root.as_deref() == Some(expected_destination_root);
    let rollback_marker_exists = journal_path.parent().is_some_and(|parent| {
        safe_identifier(&journal.operation_id)
            .ok()
            .is_some_and(|operation| parent.join(format!("{operation}.rollback.json")).exists())
    });
    let mut entries = Vec::new();
    for entry in journal.entries.iter().take(SHARED_MAX_ROLLBACK_ENTRIES) {
        entries.push(rollback_entry_preview(
            entry,
            expected_destination_root,
            backup_root,
            root_matches,
            journal.rollback_operation_id.is_some() || rollback_marker_exists,
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
        original_operation_id: journal.operation_id,
        destination_root: SharedTransactionPath::from_path(expected_destination_root),
        entries,
        available,
    };
    preview.preview_id = rollback_preview_digest(&preview);
    preview
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
    let original = read_journal(&journal_path).expect("fresh rollback preview parsed journal");
    let mut applied = fresh.clone();
    for (rollback, install) in applied.entries.iter_mut().zip(&original.entries) {
        let destination = install
            .plan_entry
            .destination_root
            .to_path_buf()
            .and_then(|root| {
                install
                    .plan_entry
                    .destination_relative_path
                    .to_path_buf()
                    .map(|relative| root.join(relative))
            });
        let Ok(destination) = destination else {
            rollback.outcome = SharedRollbackOutcome::Failed;
            continue;
        };
        let existed_before = install.destination_existed_before_apply.unwrap_or(matches!(
            install.outcome,
            SharedApplyOutcome::ReplacedExisting
        ));
        match (install.outcome, existed_before) {
            (SharedApplyOutcome::InstalledNew, false) => {
                match remove_and_verify_new_destination(&destination) {
                    Ok(()) => {
                        rollback.outcome = SharedRollbackOutcome::RemovedInstalledFile;
                        cleanup_created_directories(install, &root);
                    }
                    Err(kind) => {
                        rollback.outcome = SharedRollbackOutcome::Failed;
                        rollback.failure = Some(failure(
                            kind,
                            Some(&destination),
                            "newly installed destination could not be removed and verified absent",
                        ));
                    }
                }
            }
            (SharedApplyOutcome::ReplacedExisting, true) => {
                let Some(backup) = install.backup_path.as_ref() else {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    continue;
                };
                let Ok(backup) = backup.to_path_buf() else {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    continue;
                };
                let expected = install.backup_digest.as_deref().unwrap_or_default();
                if should_inject(FaultPoint::RollbackRestore) {
                    rollback.outcome = SharedRollbackOutcome::Failed;
                    continue;
                }
                match atomic_write(&backup, &destination, expected, false) {
                    Ok(_) => rollback.outcome = SharedRollbackOutcome::RestoredBackup,
                    Err((kind, _)) => {
                        rollback.outcome = SharedRollbackOutcome::Failed;
                        rollback.failure = Some(failure(
                            kind,
                            Some(&destination),
                            "previous destination bytes could not be restored and verified",
                        ));
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
    }
    // Every per-entry installed file/child directory above has already
    // been removed (or left in place on failure) - only now can a
    // transaction-created root directory possibly be empty, so this always
    // runs last, and is a no-op for a journal that never bootstrapped a
    // root at all (including every journal written before that field
    // existed).
    cleanup_transaction_created_root_directories(&original, &root);
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
    SharedRollbackResult {
        preview: applied,
        journal_path,
        status: if success && marker_written {
            SharedApplyStatus::Success
        } else if success {
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
    Ok(StableHash {
        digest: hex_bytes(&digest.finalize()),
        bytes,
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
    prepare_managed_root(backup_root)?;
    let operation = backup_root.join(safe_identifier(operation_id)?);
    fs::create_dir(&operation).map_err(|_| SharedApplyFailureKind::BackupFailed)?;
    let final_path = operation.join(format!(
        "{}.bak",
        digest_text(&destination.to_string_lossy())
    ));
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

fn atomic_write(
    source: &Path,
    destination: &Path,
    expected: &str,
    no_replace: bool,
) -> Result<PathBuf, (SharedApplyFailureKind, Option<PathBuf>)> {
    if should_inject(FaultPoint::TemporaryWrite) {
        return Err((SharedApplyFailureKind::WriteFailed, None));
    }
    let bytes = read_bounded(source, SHARED_MAX_SOURCE_BYTES).map_err(|kind| (kind, None))?;
    if digest_bytes(&bytes) != expected {
        return Err((SharedApplyFailureKind::SourceChanged, None));
    }
    let parent = destination
        .parent()
        .ok_or((SharedApplyFailureKind::DestinationUnsafe, None))?;
    let temp = parent.join(format!(
        ".archivefs-apply-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        file.write_all(&bytes)
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        if should_inject(FaultPoint::Flush) {
            return Err(SharedApplyFailureKind::WriteFailed);
        }
        file.sync_all()
            .map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        set_permissions(&file).map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        let temp_hash = stable_hash(&temp, SHARED_MAX_SOURCE_BYTES)?;
        if should_inject(FaultPoint::Verification) || temp_hash.digest != expected {
            return Err(SharedApplyFailureKind::VerificationFailed);
        }
        if should_inject(FaultPoint::Rename) {
            return Err(SharedApplyFailureKind::WriteFailed);
        }
        if no_replace {
            rename_no_replace(&temp, destination)
                .map_err(|_| SharedApplyFailureKind::DestinationChanged)?;
        } else {
            fs::rename(&temp, destination).map_err(|_| SharedApplyFailureKind::WriteFailed)?;
        }
        sync_directory(parent);
        let final_hash = stable_hash(destination, SHARED_MAX_SOURCE_BYTES)?;
        if final_hash.digest != expected {
            return Err(SharedApplyFailureKind::VerificationFailed);
        }
        Ok(())
    })();
    if let Err(kind) = write_result {
        let _ = fs::remove_file(&temp);
        return Err((kind, Some(temp)));
    }
    Ok(temp)
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
    root_matches: bool,
    already_rolled_back: bool,
) -> SharedRollbackEntry {
    let destination = entry
        .plan_entry
        .destination_relative_path
        .to_path_buf()
        .map(|relative| root.join(relative));
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
    };
    if !root_matches {
        result.outcome = SharedRollbackOutcome::RootMismatch;
        return result;
    }
    if already_rolled_back {
        result.outcome = SharedRollbackOutcome::AlreadyRolledBack;
        return result;
    }
    if !matches!(
        entry.outcome,
        SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
    ) {
        result.outcome = SharedRollbackOutcome::NoChangeRequired;
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
    let Ok(destination) = destination else {
        result.outcome = SharedRollbackOutcome::DestinationUnsafe;
        return result;
    };
    match stable_hash(&destination, SHARED_MAX_SOURCE_BYTES) {
        Ok(hash) => {
            result.observed_destination_digest = Some(hash.digest.clone());
            if Some(hash.digest.as_str()) != entry.final_destination_digest.as_deref() {
                result.outcome = SharedRollbackOutcome::DestinationChanged;
                return result;
            }
        }
        Err(SharedApplyFailureKind::SourceMissing) => {
            result.outcome = SharedRollbackOutcome::DestinationMissing;
            return result;
        }
        Err(_) => {
            result.outcome = SharedRollbackOutcome::DestinationUnsafe;
            return result;
        }
    }
    if entry.outcome == SharedApplyOutcome::ReplacedExisting {
        let Some(backup) = entry.backup_path.as_ref() else {
            result.outcome = SharedRollbackOutcome::BackupMissing;
            return result;
        };
        let Ok(backup) = backup.to_path_buf() else {
            result.outcome = SharedRollbackOutcome::BackupMissing;
            return result;
        };
        if backup.strip_prefix(backup_root).is_err() {
            result.outcome = SharedRollbackOutcome::BackupChanged;
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

fn remove_and_verify_new_destination(destination: &Path) -> Result<(), SharedApplyFailureKind> {
    reject_symlink_components(destination)?;
    fs::remove_file(destination).map_err(|_| SharedApplyFailureKind::WriteFailed)?;
    if should_inject(FaultPoint::RollbackRemovalVerification) {
        fs::write(destination, b"").map_err(|_| SharedApplyFailureKind::VerificationFailed)?;
    }
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = destination.parent() {
                sync_directory(parent);
            }
            Ok(())
        }
        _ => Err(SharedApplyFailureKind::VerificationFailed),
    }
}

fn cleanup_created_directories(entry: &SharedApplyEntry, root: &Path) {
    if entry.destination_parent_existed_before_apply == Some(true) {
        return;
    }
    for encoded in entry.created_directories.iter().rev() {
        let Ok(path) = encoded.to_path_buf() else {
            continue;
        };
        if path.strip_prefix(root).is_ok()
            && path != root
            && fs::read_dir(&path)
                .ok()
                .is_some_and(|mut entries| entries.next().is_none())
        {
            let _ = fs::remove_dir(&path);
        }
    }
}

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
/// `openat`+`O_NOFOLLOW` *relative to that already-open parent descriptor*
/// - never a fresh top-level pathname lookup. If an ancestor is swapped to
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

    let mut removed = 0usize;
    for index in (0..matched).rev() {
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
///   there is no gap, no repeat, and no out-of-order entry; this also
///   structurally guarantees no entry can equal the chain's own implied
///   anchor (a path is never its own parent), which is what keeps "the
///   destination root's pre-existing ancestor" out of the chain a tampered
///   journal could otherwise try to claim;
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
    remove_verified_root_chain(&chain);
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

fn set_permissions(file: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
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
            temporary_path: None,
            final_destination_digest: None,
            created_directories: Vec::new(),
            replacement_approved: false,
            verification_succeeded: false,
            outcome: SharedApplyOutcome::SkippedNotEligible,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::{
        PreviewIdentity, PreviewIdentityKind, PreviewIdentityState, PreviewMatchStrength,
        PreviewSourceItem, SharedPreviewRequest, build_shared_preview,
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "archivefs-shared-transaction-{label}-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn source_root(&self) -> PathBuf {
            self.0.join("source")
        }

        fn destination_root(&self) -> PathBuf {
            self.0.join("destination")
        }

        fn history_root(&self) -> PathBuf {
            self.0.join("history")
        }

        fn backup_root(&self) -> PathBuf {
            self.0.join("backups")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn preview(
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

    fn make_plan(fixture: &Fixture, report: &SharedPreviewReport) -> SharedTransactionPlan {
        build_shared_transaction_plan(
            report,
            "retroarch-native",
            "trusted-catalogue",
            &fixture.source_root(),
        )
        .unwrap()
    }

    fn options(
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
        let mut options = options(&fixture, &plan, "partial", false, true, false);
        options.history_root = fixture.0.join("missing-parent/history");
        let result = execute_shared_apply(&plan, &options);
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
        assert!(!root.exists());
        assert!(!fixture.0.join("dolphin").join("Load").exists());
        assert!(!fixture.0.join("dolphin").exists());
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
            !dolphin.join("Load").exists(),
            "the bootstrap-created intermediate level must be removed"
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

    // --- malformed journal claiming the pre-existing ancestor --------------

    #[test]
    fn a_journal_claiming_the_pre_existing_ancestor_is_rejected() {
        let fixture = Fixture::new("bootstrap-malformed-claims-ancestor");
        let dolphin = fixture.0.join("dolphin");
        fs::create_dir(&dolphin).unwrap();
        let root = dolphin.join("Load").join("Textures");
        let report = preview_at_root(&fixture, root.clone(), b"new");
        let plan = make_plan(&fixture, &report);
        let result = execute_shared_apply(
            &plan,
            &options(&fixture, &plan, "malformed-ancestor", false, true, false),
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        let journal_path = result.journal_path.unwrap();

        // Tamper the persisted journal: prepend the pre-existing `dolphin`
        // ancestor to the claimed chain, as if this transaction had
        // created it too.
        let mut tampered = read_journal(&journal_path).unwrap();
        let mut identity = tampered.created_root_directories[0].identity;
        // A plausible-looking (but fabricated) identity - the chain must
        // still be rejected on structure alone before identity is ever
        // consulted for the injected entry.
        if let Some(value) = &mut identity {
            value.inode = value.inode.wrapping_add(1);
        }
        tampered.created_root_directories.insert(
            0,
            SharedCreatedRootDirectory {
                path: SharedTransactionPath::from_path(&dolphin),
                identity,
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
                rollback_operation_id: "malformed-ancestor-rollback".into(),
                timestamp_unix_seconds: 1_700_000_001,
                history_root: fixture.history_root(),
                backup_root: fixture.backup_root(),
            },
        );
        assert_eq!(rolled_back.status, SharedApplyStatus::Success);
        // The whole claimed chain is rejected as malformed (an entry whose
        // parent/child relationship to its neighbour does not hold), so
        // *nothing* in it is acted on - the pre-existing ancestor
        // survives, but so does the genuinely transaction-created leaf.
        assert!(
            dolphin.is_dir(),
            "the pre-existing ancestor must never be removed"
        );
        assert!(
            root.exists(),
            "a malformed chain must not be partially honoured either"
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
