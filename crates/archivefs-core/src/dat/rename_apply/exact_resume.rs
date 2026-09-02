//! Approval-bound exact resume for DAT rename transactions.
//!
//! Exact resume is deliberately a separate executor from fresh apply. It
//! consumes the immutable approval envelope captured when the DAT plan was
//! reviewed, proves the envelope still matches the journal and current plan
//! identity, reconciles every operation against the filesystem, and only then
//! reuses the existing no-clobber mutation primitive.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::dat::rename_plan::RenamePlan;
use crate::safe_read::TrustedRoots;

use super::executor::apply_mutation;
use super::identity::{capture_identity, identity_matches};
use super::journal::write_journal;
use super::model::{
    EntryState, ExactResumeEnvelope, ExactResumeOperation, ExactResumeState, RenameTransaction,
    TransactionOperation, TransactionState,
};
use super::preflight::{DirectoryPolicy, PreflightOptions, batch_destinations, run_preflight};

/// The first approval-envelope format. Unknown versions are never guessed at.
pub const EXACT_RESUME_FORMAT_VERSION: u32 = 1;

const ENVELOPE_KEY: &str = "emuwiz_exact_resume_envelope";
const STATE_KEY: &str = "emuwiz_exact_resume_state";

/// The read-only answer about whether an exact retry can be offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactResumeInspection {
    Available {
        pending_operations: usize,
        interrupted: bool,
    },
    AlreadyComplete,
    /// A legacy journal has no approval-bound envelope and cannot be upgraded.
    UnavailableLegacy,
    Refused(ExactResumeRefusal),
}

/// A typed reason exact resume is unavailable or must be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactResumeRefusal {
    UnsupportedVersion(u32),
    MissingApprovalEvidence,
    MissingCheckpoint,
    EnvelopeTransactionMismatch,
    EnvelopeOperationSetMismatch,
    DuplicateApprovalPath,
    PlanDigestMismatch,
    GenerationMismatch { approved: u64, current: u64 },
    UnsupportedOperation,
    ChangedSource(PathBuf),
    DestinationConflict(PathBuf),
    CannotProveRemaining(PathBuf),
    AlreadySettled(TransactionState),
    InconsistentCompletion,
    NotResumable,
}

impl std::fmt::Display for ExactResumeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported exact-resume format version {version}")
            }
            Self::MissingApprovalEvidence => write!(
                f,
                "exact resume unavailable: required approval evidence is missing"
            ),
            Self::MissingCheckpoint => {
                write!(f, "exact resume unavailable: durable checkpoint is missing")
            }
            Self::EnvelopeTransactionMismatch => {
                write!(f, "exact resume envelope does not match its transaction")
            }
            Self::EnvelopeOperationSetMismatch => {
                write!(f, "exact resume operation set does not match the journal")
            }
            Self::DuplicateApprovalPath => write!(
                f,
                "exact resume approval evidence contains a duplicate path"
            ),
            Self::PlanDigestMismatch => write!(f, "the approved plan digest no longer matches"),
            Self::GenerationMismatch { approved, current } => write!(
                f,
                "the approved generation is stale (approved {approved}, current {current})"
            ),
            Self::UnsupportedOperation => {
                write!(f, "the exact-resume operation kind is unsupported")
            }
            Self::ChangedSource(path) => {
                write!(f, "the approved source changed: {}", path.display())
            }
            Self::DestinationConflict(path) => write!(
                f,
                "the destination conflicts with the approved operation: {}",
                path.display()
            ),
            Self::CannotProveRemaining(path) => write!(
                f,
                "the filesystem cannot prove the exact remaining operation: {}",
                path.display()
            ),
            Self::AlreadySettled(state) => {
                write!(f, "the transaction is already settled as {state:?}")
            }
            Self::InconsistentCompletion => write!(
                f,
                "the completion checkpoint is inconsistent with the operation states"
            ),
            Self::NotResumable => write!(f, "the transaction is marked not resumable"),
        }
    }
}

/// Errors after an exact resume has passed its structural gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactResumeError {
    Refused(ExactResumeRefusal),
    Journal(String),
    Preflight { path: PathBuf, reasons: Vec<String> },
    Mutation { path: PathBuf, reason: String },
}

impl std::fmt::Display for ExactResumeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) => reason.fmt(f),
            Self::Journal(detail) => {
                write!(f, "could not write the exact-resume journal: {detail}")
            }
            Self::Preflight { path, reasons } => write!(
                f,
                "exact resume preflight failed for {}: {}",
                path.display(),
                reasons.join("; ")
            ),
            Self::Mutation { path, reason } => {
                write!(f, "exact resume failed for {}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for ExactResumeError {}

/// The result of an exact-resume attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactResumeResult {
    Completed,
    AlreadyComplete,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactResumeOutcome {
    pub result: ExactResumeResult,
    pub transaction: RenameTransaction,
}

/// Inputs to the separate exact-resume executor. The caller supplies the
/// current plan digest; no fresh plan is built by this path.
#[derive(Debug)]
pub struct ExactResumeExecution<'a> {
    pub transaction: &'a mut RenameTransaction,
    pub current_generation: u64,
    pub current_plan_digest: String,
    pub trusted: TrustedRoots,
    pub journal_dir: PathBuf,
    pub cancel: &'a AtomicBool,
}

/// Computes the digest bound into an approval envelope.
///
/// `RenamePlan` is intentionally a read-only model rather than a persisted
/// wire type. Its complete debug representation is combined with the exact,
/// sorted approval set so every plan field and approval decision participates
/// in the digest without introducing a second plan serializer.
pub fn compute_plan_digest(plan: &RenamePlan, approved_paths: &BTreeSet<String>) -> String {
    let canonical = format!("{plan:?}\nAPPROVED:{approved_paths:?}");
    hex(&Sha256::digest(canonical.as_bytes()))
}

/// Builds the immutable envelope at review/approval time.
pub fn build_envelope(
    plan: &RenamePlan,
    approved_paths: &BTreeSet<String>,
    transaction: &RenameTransaction,
    created_at_unix: u64,
) -> ExactResumeEnvelope {
    let operations = transaction
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| ExactResumeOperation {
            index,
            source_path: entry.source_path.clone(),
            destination_path: entry.destination_path.clone(),
            operation: entry.operation.clone(),
            identity: entry.identity.clone(),
            original_basename: entry.original_basename.clone(),
            proposed_basename: entry.proposed_basename.clone(),
        })
        .collect();
    ExactResumeEnvelope {
        format_version: EXACT_RESUME_FORMAT_VERSION,
        transaction_id: transaction.transaction_id.clone(),
        approved_generation: plan.generation,
        classifier_version: plan.classifier_version.clone(),
        plan_digest: compute_plan_digest(plan, approved_paths),
        source_scan_root: plan.scan_root.clone(),
        approved_source_paths: approved_paths.iter().cloned().collect(),
        operations,
        created_at_unix,
    }
}

/// Stores the typed envelope in the journal's forward-compatible payload.
/// Keeping this in `unknown` preserves the public transaction shape used by
/// GUI and non-DAT repair fixtures while making the presence of the reserved
/// keys an explicit legacy boundary.
pub(crate) fn store_envelope(transaction: &mut RenameTransaction, envelope: ExactResumeEnvelope) {
    transaction.unknown.insert(
        ENVELOPE_KEY.to_string(),
        serde_json::to_value(envelope).expect("exact resume envelope is serializable"),
    );
}

pub(crate) fn set_state(transaction: &mut RenameTransaction, state: ExactResumeState) {
    transaction.unknown.insert(
        STATE_KEY.to_string(),
        serde_json::to_value(state).expect("exact resume state is serializable"),
    );
}

pub(crate) fn has_envelope(transaction: &RenameTransaction) -> bool {
    transaction.unknown.contains_key(ENVELOPE_KEY)
}

/// Reads the durable exact-resume checkpoint without requiring a current plan.
///
/// History cleanup uses this to distinguish a legacy journal (no envelope), an
/// explicitly refused exact-resume record, and a record whose approved
/// envelope may still become usable after a current plan is loaded. A malformed
/// checkpoint remains an error so callers can fail closed.
pub fn exact_resume_state(
    transaction: &RenameTransaction,
) -> Result<Option<ExactResumeState>, ExactResumeRefusal> {
    read_state(transaction)
}

fn read_envelope(
    transaction: &RenameTransaction,
) -> Result<Option<ExactResumeEnvelope>, ExactResumeRefusal> {
    let Some(value) = transaction.unknown.get(ENVELOPE_KEY) else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|_| ExactResumeRefusal::MissingApprovalEvidence)
}

fn read_state(
    transaction: &RenameTransaction,
) -> Result<Option<ExactResumeState>, ExactResumeRefusal> {
    let Some(value) = transaction.unknown.get(STATE_KEY) else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|_| ExactResumeRefusal::MissingCheckpoint)
}

/// Inspects only the durable transaction/envelope. Filesystem reconciliation
/// is performed by [`resume_exact_transaction`] immediately before mutation.
pub fn inspect_exact_resume(
    transaction: &RenameTransaction,
    current_generation: u64,
    current_plan_digest: &str,
) -> ExactResumeInspection {
    let envelope = match read_envelope(transaction) {
        Ok(Some(envelope)) => envelope,
        Ok(None) => return ExactResumeInspection::UnavailableLegacy,
        Err(reason) => return ExactResumeInspection::Refused(reason),
    };
    if let Err(reason) = validate_structure(transaction, &envelope) {
        return ExactResumeInspection::Refused(reason);
    }
    if envelope.approved_generation != current_generation {
        return ExactResumeInspection::Refused(ExactResumeRefusal::GenerationMismatch {
            approved: envelope.approved_generation,
            current: current_generation,
        });
    }
    if envelope.plan_digest != current_plan_digest {
        return ExactResumeInspection::Refused(ExactResumeRefusal::PlanDigestMismatch);
    }
    let state = match read_state(transaction) {
        Ok(state) => state,
        Err(reason) => return ExactResumeInspection::Refused(reason),
    };
    match state {
        Some(ExactResumeState::NotResumable) => {
            ExactResumeInspection::Refused(ExactResumeRefusal::NotResumable)
        }
        Some(ExactResumeState::Completed) => {
            if transaction.state == TransactionState::Applied
                && transaction
                    .entries
                    .iter()
                    .all(|entry| entry.state == EntryState::Applied)
            {
                ExactResumeInspection::AlreadyComplete
            } else {
                ExactResumeInspection::Refused(ExactResumeRefusal::InconsistentCompletion)
            }
        }
        _ if transaction.state == TransactionState::RolledBack => {
            ExactResumeInspection::Refused(ExactResumeRefusal::AlreadySettled(transaction.state))
        }
        _ if transaction.state == TransactionState::Applied
            && transaction
                .entries
                .iter()
                .all(|entry| entry.state == EntryState::Applied) =>
        {
            ExactResumeInspection::AlreadyComplete
        }
        _ => ExactResumeInspection::Available {
            pending_operations: transaction
                .entries
                .iter()
                .filter(|entry| entry.state != EntryState::Applied)
                .count(),
            interrupted: transaction.state.needs_recovery()
                || state == Some(ExactResumeState::Interrupted),
        },
    }
}

/// Reconciles and executes the exact approved operation set. No fresh plan or
/// fresh approval set is accepted by this function.
pub fn resume_exact_transaction(
    execution: &mut ExactResumeExecution<'_>,
) -> Result<ExactResumeOutcome, ExactResumeError> {
    let envelope = match read_envelope(execution.transaction) {
        Ok(Some(envelope)) => envelope,
        Ok(None) => {
            return Err(ExactResumeError::Refused(
                ExactResumeRefusal::MissingApprovalEvidence,
            ));
        }
        Err(reason) => return refuse(execution.transaction, &execution.journal_dir, reason),
    };
    if let Err(reason) = validate_structure(execution.transaction, &envelope).and_then(|()| {
        validate_current_identity(
            execution.transaction,
            &envelope,
            execution.current_generation,
            &execution.current_plan_digest,
        )
    }) {
        return refuse(execution.transaction, &execution.journal_dir, reason);
    }
    let state = match read_state(execution.transaction) {
        Ok(Some(state)) => state,
        Ok(None) => {
            return refuse(
                execution.transaction,
                &execution.journal_dir,
                ExactResumeRefusal::MissingCheckpoint,
            );
        }
        Err(reason) => return refuse(execution.transaction, &execution.journal_dir, reason),
    };
    if state == ExactResumeState::NotResumable {
        return Err(ExactResumeError::Refused(ExactResumeRefusal::NotResumable));
    }
    if execution.transaction.state == TransactionState::RolledBack {
        return Err(ExactResumeError::Refused(
            ExactResumeRefusal::AlreadySettled(execution.transaction.state),
        ));
    }
    let all_applied = execution.transaction.state == TransactionState::Applied
        && execution
            .transaction
            .entries
            .iter()
            .all(|entry| entry.state == EntryState::Applied);
    if state == ExactResumeState::Completed && !all_applied {
        return refuse(
            execution.transaction,
            &execution.journal_dir,
            ExactResumeRefusal::InconsistentCompletion,
        );
    }
    if state == ExactResumeState::Completed || all_applied {
        return Ok(ExactResumeOutcome {
            result: ExactResumeResult::AlreadyComplete,
            transaction: execution.transaction.clone(),
        });
    }

    if let Err(reason) = reconcile_exact_entries(execution.transaction) {
        return refuse(execution.transaction, &execution.journal_dir, reason);
    }

    let pending = execution
        .transaction
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| (entry.state != EntryState::Applied).then_some(index))
        .collect::<Vec<_>>();
    if pending.is_empty() {
        execution.transaction.state = TransactionState::Applied;
        set_state(execution.transaction, ExactResumeState::Completed);
        persist(execution)?;
        return Ok(ExactResumeOutcome {
            result: ExactResumeResult::Completed,
            transaction: execution.transaction.clone(),
        });
    }

    execution.transaction.state = TransactionState::Applying;
    set_state(execution.transaction, ExactResumeState::Pending);
    persist(execution)?;

    let approved_paths = envelope
        .approved_source_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let destinations = batch_destinations(&execution.transaction.entries);
    for index in pending {
        if execution.cancel.load(Ordering::Relaxed) {
            execution.transaction.state = TransactionState::ApplyFailed;
            set_state(execution.transaction, ExactResumeState::Interrupted);
            persist(execution)?;
            return Ok(ExactResumeOutcome {
                result: ExactResumeResult::Interrupted,
                transaction: execution.transaction.clone(),
            });
        }
        let options = PreflightOptions {
            plan_generation: envelope.approved_generation,
            current_generation: execution.current_generation,
            approved_paths: &approved_paths,
            trusted: &execution.trusted,
            batch_destinations: &destinations,
            directory_policy: DirectoryPolicy::SameDirectory,
            allow_symlink_source: false,
        };
        if let Err(failures) = run_preflight(&execution.transaction.entries[index], &options) {
            let reasons = failures
                .iter()
                .map(|failure| failure.reason())
                .collect::<Vec<_>>();
            execution.transaction.entries[index].preflight_passed = false;
            execution.transaction.entries[index].preflight_failures = reasons.clone();
            execution.transaction.entries[index].failure_reason = Some(reasons.join("; "));
            execution.transaction.entries[index].state = EntryState::ApplyFailed;
            execution.transaction.state = TransactionState::ApplyFailed;
            set_state(execution.transaction, ExactResumeState::NotResumable);
            persist(execution)?;
            return Err(ExactResumeError::Preflight {
                path: execution.transaction.entries[index].source_path.clone(),
                reasons,
            });
        }
        execution.transaction.entries[index].preflight_passed = true;
        execution.transaction.entries[index].state = EntryState::Applying;
        persist(execution)?;
        match apply_mutation(&execution.transaction.entries[index]) {
            Ok(()) => {
                execution.transaction.entries[index].state = EntryState::Applied;
                execution.transaction.entries[index].applied_at_unix =
                    Some(crate::dat::sources::now_unix());
                persist(execution)?;
            }
            Err((state, reason)) => {
                execution.transaction.entries[index].state = state;
                execution.transaction.entries[index].failure_reason = Some(reason.clone());
                execution.transaction.state = TransactionState::ApplyFailed;
                set_state(execution.transaction, ExactResumeState::Failed);
                persist(execution)?;
                return Err(ExactResumeError::Mutation {
                    path: execution.transaction.entries[index].source_path.clone(),
                    reason,
                });
            }
        }
    }
    execution.transaction.state = TransactionState::Applied;
    set_state(execution.transaction, ExactResumeState::Completed);
    persist(execution)?;
    Ok(ExactResumeOutcome {
        result: ExactResumeResult::Completed,
        transaction: execution.transaction.clone(),
    })
}

fn persist(execution: &ExactResumeExecution<'_>) -> Result<(), ExactResumeError> {
    write_journal(&execution.journal_dir, execution.transaction)
        .map_err(|error| ExactResumeError::Journal(error.to_string()))
}

fn refuse(
    transaction: &mut RenameTransaction,
    journal_dir: &Path,
    reason: ExactResumeRefusal,
) -> Result<ExactResumeOutcome, ExactResumeError> {
    set_state(transaction, ExactResumeState::NotResumable);
    write_journal(journal_dir, transaction)
        .map_err(|error| ExactResumeError::Journal(error.to_string()))?;
    Err(ExactResumeError::Refused(reason))
}

fn validate_current_identity(
    transaction: &RenameTransaction,
    envelope: &ExactResumeEnvelope,
    current_generation: u64,
    current_plan_digest: &str,
) -> Result<(), ExactResumeRefusal> {
    if envelope.approved_generation != current_generation {
        return Err(ExactResumeRefusal::GenerationMismatch {
            approved: envelope.approved_generation,
            current: current_generation,
        });
    }
    if envelope.plan_digest != current_plan_digest {
        return Err(ExactResumeRefusal::PlanDigestMismatch);
    }
    if transaction.classifier_version.as_deref() != Some(envelope.classifier_version.as_str()) {
        return Err(ExactResumeRefusal::MissingApprovalEvidence);
    }
    Ok(())
}

fn validate_structure(
    transaction: &RenameTransaction,
    envelope: &ExactResumeEnvelope,
) -> Result<(), ExactResumeRefusal> {
    if envelope.format_version != EXACT_RESUME_FORMAT_VERSION {
        return Err(ExactResumeRefusal::UnsupportedVersion(
            envelope.format_version,
        ));
    }
    if read_state(transaction)?.is_none() {
        return Err(ExactResumeRefusal::MissingCheckpoint);
    }
    if envelope.transaction_id != transaction.transaction_id
        || envelope.approved_generation != transaction.plan_generation
        || envelope.source_scan_root != transaction.source_scan_root
    {
        return Err(ExactResumeRefusal::EnvelopeTransactionMismatch);
    }
    if transaction.classifier_version.as_deref() != Some(envelope.classifier_version.as_str())
        || envelope.plan_digest.is_empty()
        || envelope.approved_source_paths.is_empty()
    {
        return Err(ExactResumeRefusal::MissingApprovalEvidence);
    }
    let mut approvals = envelope.approved_source_paths.clone();
    approvals.sort();
    if approvals.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ExactResumeRefusal::DuplicateApprovalPath);
    }
    if envelope.operations.len() != transaction.entries.len() {
        return Err(ExactResumeRefusal::EnvelopeOperationSetMismatch);
    }
    for (index, (operation, entry)) in envelope
        .operations
        .iter()
        .zip(transaction.entries.iter())
        .enumerate()
    {
        if operation.index != index
            || operation.source_path != entry.source_path
            || operation.destination_path != entry.destination_path
            || operation.operation != entry.operation
            || operation.identity != entry.identity
            || operation.original_basename != entry.original_basename
            || operation.proposed_basename != entry.proposed_basename
        {
            return Err(ExactResumeRefusal::EnvelopeOperationSetMismatch);
        }
        if approvals
            .binary_search(&operation.source_path.to_string_lossy().into_owned())
            .is_err()
        {
            return Err(ExactResumeRefusal::MissingApprovalEvidence);
        }
        if !matches!(operation.operation, TransactionOperation::RenameMove) {
            return Err(ExactResumeRefusal::UnsupportedOperation);
        }
    }
    Ok(())
}

fn reconcile_exact_entries(transaction: &mut RenameTransaction) -> Result<(), ExactResumeRefusal> {
    for entry in &mut transaction.entries {
        let source_present = std::fs::symlink_metadata(&entry.source_path).is_ok();
        let destination_present = std::fs::symlink_metadata(&entry.destination_path).is_ok();
        let source_matches = source_present
            && capture_identity(&entry.source_path)
                .ok()
                .is_some_and(|identity| identity_matches(&entry.identity, &identity));
        let destination_matches = destination_present
            && capture_identity(&entry.destination_path)
                .ok()
                .is_some_and(|identity| identity_matches(&entry.identity, &identity));
        match (
            source_present,
            destination_present,
            source_matches,
            destination_matches,
        ) {
            (true, false, true, false) | (true, false, true, true) => {
                entry.state = EntryState::Planned;
                entry.failure_reason = None;
            }
            (false, true, false, true) => {
                entry.state = EntryState::Applied;
                entry
                    .applied_at_unix
                    .get_or_insert(crate::dat::sources::now_unix());
            }
            (true, true, _, _) | (false, true, _, false) => {
                return Err(ExactResumeRefusal::DestinationConflict(
                    entry.destination_path.clone(),
                ));
            }
            (true, false, false, _) => {
                return Err(ExactResumeRefusal::ChangedSource(entry.source_path.clone()));
            }
            (false, false, _, _) => {
                return Err(ExactResumeRefusal::CannotProveRemaining(
                    entry.source_path.clone(),
                ));
            }
            (false, true, _, true) => {
                entry.state = EntryState::Applied;
                entry
                    .applied_at_unix
                    .get_or_insert(crate::dat::sources::now_unix());
            }
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::rename_apply::journal::{journal_path, read_journal};
    use crate::dat::rename_apply::model::TransactionEntry;

    struct Fixture {
        dir: tempfile::TempDir,
        journal: PathBuf,
        transaction: RenameTransaction,
    }

    fn fixture(two_operations: bool) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("roms");
        let journal = dir.path().join("journal");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&journal).unwrap();
        let mut entries = Vec::new();
        for (source_name, destination_name) in [("a.bin", "A.bin"), ("b.bin", "B.bin")] {
            if !two_operations && source_name == "b.bin" {
                break;
            }
            let source = root.join(source_name);
            std::fs::write(&source, source_name.as_bytes()).unwrap();
            let identity = capture_identity(&source).unwrap();
            entries.push(TransactionEntry {
                source_path: source,
                destination_path: root.join(destination_name),
                original_basename: source_name.to_string(),
                proposed_basename: destination_name.to_string(),
                identity,
                operation: TransactionOperation::RenameMove,
                preflight_passed: false,
                preflight_failures: Vec::new(),
                state: EntryState::Applying,
                failure_reason: None,
                applied_at_unix: None,
                rolled_back_at_unix: None,
                unknown: Default::default(),
            });
        }
        let approved_source_paths = entries
            .iter()
            .map(|entry| entry.source_path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let operations = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| ExactResumeOperation {
                index,
                source_path: entry.source_path.clone(),
                destination_path: entry.destination_path.clone(),
                operation: entry.operation.clone(),
                identity: entry.identity.clone(),
                original_basename: entry.original_basename.clone(),
                proposed_basename: entry.proposed_basename.clone(),
            })
            .collect();
        let mut transaction = RenameTransaction {
            transaction_id: "exact-test".to_string(),
            plan_generation: 7,
            classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
            created_at_unix: 1,
            source_scan_root: root.to_string_lossy().into_owned(),
            state: TransactionState::Applying,
            entries,
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        };
        let transaction_id = transaction.transaction_id.clone();
        let approved_generation = transaction.plan_generation;
        let source_scan_root = transaction.source_scan_root.clone();
        store_envelope(
            &mut transaction,
            ExactResumeEnvelope {
                format_version: EXACT_RESUME_FORMAT_VERSION,
                transaction_id,
                approved_generation,
                classifier_version: crate::dat::classification::CLASSIFIER_VERSION.to_string(),
                plan_digest: "digest".to_string(),
                source_scan_root,
                approved_source_paths,
                operations,
                created_at_unix: 1,
            },
        );
        set_state(&mut transaction, ExactResumeState::Pending);
        Fixture {
            dir,
            journal,
            transaction,
        }
    }

    fn resume(fixture: &mut Fixture) -> Result<ExactResumeOutcome, ExactResumeError> {
        let root = fixture.dir.path().join("roms");
        let cancel = AtomicBool::new(false);
        resume_exact_transaction(&mut ExactResumeExecution {
            transaction: &mut fixture.transaction,
            current_generation: 7,
            current_plan_digest: "digest".to_string(),
            trusted: TrustedRoots::from_paths([&root]),
            journal_dir: fixture.journal.clone(),
            cancel: &cancel,
        })
    }

    #[test]
    fn interrupted_approved_rename_resumes_only_the_exact_remaining_operations() {
        let mut fixture = fixture(true);
        let first_source = fixture.transaction.entries[0].source_path.clone();
        let first_destination = fixture.transaction.entries[0].destination_path.clone();
        let first_identity = fixture.transaction.entries[0].identity.clone();
        std::fs::rename(&first_source, &first_destination).unwrap();
        fixture.transaction.entries[0].state = EntryState::Applied;
        write_journal(&fixture.journal, &fixture.transaction).unwrap();
        assert_eq!(
            inspect_exact_resume(&fixture.transaction, 7, "digest"),
            ExactResumeInspection::Available {
                pending_operations: 1,
                interrupted: true,
            }
        );

        let outcome = resume(&mut fixture).unwrap();
        assert_eq!(outcome.result, ExactResumeResult::Completed);
        assert_eq!(
            read_state(&outcome.transaction).unwrap(),
            Some(ExactResumeState::Completed)
        );
        assert!(!first_source.exists());
        assert!(first_destination.exists());
        assert!(identity_matches(
            &first_identity,
            &capture_identity(&first_destination).unwrap()
        ));
        assert!(!fixture.transaction.entries[1].source_path.exists());
        assert!(fixture.transaction.entries[1].destination_path.exists());
        let reloaded =
            read_journal(&journal_path(&fixture.journal, "exact-test").unwrap()).unwrap();
        assert_eq!(
            read_state(&reloaded).unwrap(),
            Some(ExactResumeState::Completed)
        );
        assert_eq!(
            inspect_exact_resume(&outcome.transaction, 7, "digest"),
            ExactResumeInspection::AlreadyComplete
        );
    }

    #[test]
    fn changed_source_refuses_before_mutation() {
        let mut fixture = fixture(false);
        let source = fixture.transaction.entries[0].source_path.clone();
        std::fs::remove_file(&source).unwrap();
        std::fs::write(&source, b"replacement").unwrap();
        let error = resume(&mut fixture).unwrap_err();
        assert_eq!(
            error,
            ExactResumeError::Refused(ExactResumeRefusal::ChangedSource(source.clone()))
        );
        assert!(!fixture.transaction.entries[0].destination_path.exists());
        assert_eq!(
            read_state(&fixture.transaction).unwrap(),
            Some(ExactResumeState::NotResumable)
        );
    }

    #[test]
    fn conflicting_destination_refuses_without_clobbering() {
        let mut fixture = fixture(false);
        let destination = fixture.transaction.entries[0].destination_path.clone();
        std::fs::write(&destination, b"user data").unwrap();
        let error = resume(&mut fixture).unwrap_err();
        assert_eq!(
            error,
            ExactResumeError::Refused(ExactResumeRefusal::DestinationConflict(destination.clone()))
        );
        assert_eq!(std::fs::read(destination).unwrap(), b"user data");
        assert!(fixture.transaction.entries[0].source_path.exists());
    }

    #[test]
    fn changed_plan_digest_refuses_before_filesystem_access() {
        let mut fixture = fixture(false);
        let root = fixture.dir.path().join("roms");
        let cancel = AtomicBool::new(false);
        let error = resume_exact_transaction(&mut ExactResumeExecution {
            transaction: &mut fixture.transaction,
            current_generation: 7,
            current_plan_digest: "different".to_string(),
            trusted: TrustedRoots::from_paths([root]),
            journal_dir: fixture.journal.clone(),
            cancel: &cancel,
        })
        .unwrap_err();
        assert_eq!(
            error,
            ExactResumeError::Refused(ExactResumeRefusal::PlanDigestMismatch)
        );
        assert!(fixture.transaction.entries[0].source_path.exists());
        assert!(!fixture.transaction.entries[0].destination_path.exists());
    }

    #[test]
    fn unknown_version_is_refused_and_legacy_is_never_upgraded() {
        let mut fixture = fixture(false);
        fixture.transaction.unknown.get_mut(ENVELOPE_KEY).unwrap()["format_version"] =
            serde_json::json!(99);
        assert_eq!(
            inspect_exact_resume(&fixture.transaction, 7, "digest"),
            ExactResumeInspection::Refused(ExactResumeRefusal::UnsupportedVersion(99))
        );

        let mut legacy = fixture.transaction.clone();
        legacy.unknown.remove(ENVELOPE_KEY);
        legacy.unknown.remove(STATE_KEY);
        let root = fixture.dir.path().join("roms");
        assert_eq!(
            inspect_exact_resume(&legacy, 7, "digest"),
            ExactResumeInspection::UnavailableLegacy
        );
        assert_eq!(
            resume_exact_transaction(&mut ExactResumeExecution {
                transaction: &mut legacy,
                current_generation: 7,
                current_plan_digest: "digest".to_string(),
                trusted: TrustedRoots::from_paths([root]),
                journal_dir: fixture.journal.clone(),
                cancel: &AtomicBool::new(false),
            })
            .unwrap_err(),
            ExactResumeError::Refused(ExactResumeRefusal::MissingApprovalEvidence)
        );
    }

    #[test]
    fn changed_operation_set_and_missing_checkpoint_fail_closed() {
        let mut changed = fixture(false);
        let changed_destination = changed.transaction.entries[0]
            .destination_path
            .with_file_name("other.bin");
        changed.transaction.entries[0].destination_path = changed_destination;
        assert_eq!(
            inspect_exact_resume(&changed.transaction, 7, "digest"),
            ExactResumeInspection::Refused(ExactResumeRefusal::EnvelopeOperationSetMismatch)
        );

        let mut no_checkpoint = fixture(false).transaction;
        no_checkpoint.unknown.remove(STATE_KEY);
        assert_eq!(
            inspect_exact_resume(&no_checkpoint, 7, "digest"),
            ExactResumeInspection::Refused(ExactResumeRefusal::MissingCheckpoint)
        );
    }

    #[test]
    fn malformed_journal_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.json");
        std::fs::write(&path, "{\"transaction_id\":").unwrap();
        assert!(read_journal(&path).is_err());
    }

    #[test]
    fn malformed_resume_envelope_fails_closed() {
        let mut fixture = fixture(false);
        fixture.transaction.unknown.insert(
            ENVELOPE_KEY.to_string(),
            serde_json::json!({"format_version": 1}),
        );
        assert_eq!(
            inspect_exact_resume(&fixture.transaction, 7, "digest"),
            ExactResumeInspection::Refused(ExactResumeRefusal::MissingApprovalEvidence)
        );
    }

    #[test]
    fn cancelled_exact_resume_persists_interrupted_checkpoint() {
        let mut fixture = fixture(false);
        let root = fixture.dir.path().join("roms");
        let cancel = AtomicBool::new(true);
        let outcome = resume_exact_transaction(&mut ExactResumeExecution {
            transaction: &mut fixture.transaction,
            current_generation: 7,
            current_plan_digest: "digest".to_string(),
            trusted: TrustedRoots::from_paths([root]),
            journal_dir: fixture.journal.clone(),
            cancel: &cancel,
        })
        .unwrap();
        assert_eq!(outcome.result, ExactResumeResult::Interrupted);
        assert_eq!(
            read_journal(&journal_path(&fixture.journal, "exact-test").unwrap())
                .unwrap()
                .unknown
                .get(STATE_KEY)
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
            Some(ExactResumeState::Interrupted)
        );
    }
}
