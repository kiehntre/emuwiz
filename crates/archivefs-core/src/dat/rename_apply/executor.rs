//! The gated transaction executor: the only place renames happen.
//!
//! The executor turns an explicitly approved subset of a rename plan into a
//! journal-backed transaction, preflights the whole batch, journals it durably
//! **before** mutating anything, then applies entries one at a time - each one
//! freshly preflighted, renamed with a no-clobber primitive, and confirmed by
//! the filesystem before it is marked Applied. Nothing here runs unattended;
//! it never retries a failed mutation, and a failure stops the batch.
//!
//! # Build and apply are separate layers
//!
//! [`build_transaction`] captures each approved source's identity **at review
//! time** (when the user previews the batch). [`apply_transaction`] then
//! preflights everything again **immediately before** each rename, so a hostile
//! change between review and apply - a different inode, a symlink substitution,
//! a resized file, an appearing destination - is caught. The GUI never calls
//! `std::fs::rename`; this module owns every mutation.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dat::rename_plan::{RenamePlan, RenameProposal};
use crate::safe_read::TrustedRoots;

use super::identity::capture_identity;
use super::journal::{checkpoint_journal as write_journal, new_transaction_id};
use super::model::{
    EntryState, RenameTransaction, TransactionEntry, TransactionOperation, TransactionState,
    TransactionSummary,
};
use super::noclobber::NoClobberError;
use super::preflight::{PreflightOptions, batch_destinations, run_preflight};

/// How the executor treats a batch that contains a hard preflight conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardConflictMode {
    /// Any hard conflict prevents the batch from starting at all; nothing is
    /// mutated and no journal is written.
    AbortAll,
    /// Only the entries that pass preflight are applied; every other entry is
    /// journaled as Skipped. The user must have explicitly chosen this.
    SkipUnsafeSubset,
}

/// Why a batch could not be built or started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// No proposals were both approved and actionable.
    NothingApproved,
    /// The plan is stale (its generation no longer matches the current one).
    StalePlan { plan: u64, current: u64 },
    /// The plan was generated with different (or unknown) classification
    /// rules and must be regenerated before it can be applied.
    StaleClassifierVersion {
        plan: Option<String>,
        current: String,
    },
    /// One or more entries failed preflight and the batch is in AbortAll mode.
    HardConflicts(Vec<(PathBuf, Vec<String>)>),
    /// The transaction id could not name a journal file.
    InvalidTransactionId(String),
    /// Writing the journal failed.
    Journal(String),
    /// The batch was cancelled before any mutation.
    Cancelled,
    /// The transaction has already reached (or begun entering) a rollback
    /// or otherwise-settled terminal state and must never be resumed by a
    /// fresh apply call - a caller must build a new transaction instead.
    /// [`apply_transaction`] itself refuses these states (and
    /// [`crate::platform_evidence_fusion::plan_transaction::apply_plan_transaction_with_mode`]
    /// also checks before invoking it): after a
    /// rollback, an entry's source path is restored to its original,
    /// recorded identity, which an ordinary preflight pass would otherwise
    /// treat as fresh and safe - silently resurrecting an already-reversed
    /// transaction (Batch 15 finding).
    AlreadySettled {
        transaction_id: String,
        state: TransactionState,
    },
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingApproved => {
                write!(f, "no approved, actionable proposals were selected")
            }
            Self::StalePlan { plan, current } => write!(
                f,
                "the plan is stale (generation {plan}, current {current}); run a fresh audit"
            ),
            Self::StaleClassifierVersion { .. } => write!(
                f,
                "Rename plan is stale because classification rules changed. Regenerate the plan before applying."
            ),
            Self::HardConflicts(conflicts) => {
                write!(f, "preflight found {} hard conflict(s)", conflicts.len())
            }
            Self::InvalidTransactionId(id) => write!(f, "transaction id '{id}' is not usable"),
            Self::Journal(detail) => write!(f, "could not write the transaction journal: {detail}"),
            Self::Cancelled => write!(f, "the apply was cancelled"),
            Self::AlreadySettled {
                transaction_id,
                state,
            } => write!(
                f,
                "transaction {transaction_id} has already reached {state:?} and cannot be \
                 reapplied; build a fresh transaction instead"
            ),
        }
    }
}

impl std::error::Error for ApplyError {}

/// The result of an apply pass.
#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    pub transaction: RenameTransaction,
    pub summary: TransactionSummary,
}

/// Builds the transaction for the approved, actionable proposals of a plan,
/// capturing each source's identity **at review time**.
///
/// Read-only (identity capture never follows a link) and side-effect free: it
/// writes no journal and renames nothing. The returned transaction is what a
/// caller shows in its read-only review before execution.
pub fn build_transaction(
    plan: &RenamePlan,
    approved_paths: &BTreeSet<String>,
    current_generation: u64,
) -> Result<RenameTransaction, ApplyError> {
    validate_classifier_version(Some(&plan.classifier_version))?;
    if plan.generation != current_generation {
        return Err(ApplyError::StalePlan {
            plan: plan.generation,
            current: current_generation,
        });
    }
    let entries = build_transaction_entries(plan, approved_paths);
    if entries.is_empty() {
        return Err(ApplyError::NothingApproved);
    }
    let transaction_id = new_transaction_id(crate::dat::sources::now_unix());
    let created_at_unix = crate::dat::sources::now_unix();
    let mut transaction = RenameTransaction {
        transaction_id,
        plan_generation: plan.generation,
        classifier_version: Some(plan.classifier_version.clone()),
        created_at_unix,
        source_scan_root: plan.scan_root.clone(),
        state: TransactionState::Planned,
        entries,
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let envelope =
        super::exact_resume::build_envelope(plan, approved_paths, &transaction, created_at_unix);
    super::exact_resume::store_envelope(&mut transaction, envelope);
    super::exact_resume::set_state(&mut transaction, super::model::ExactResumeState::Pending);
    Ok(transaction)
}

/// Builds the transaction entries for the approved, actionable proposals of a
/// plan, capturing each source's current identity. Read-only.
pub fn build_transaction_entries(
    plan: &RenamePlan,
    approved_paths: &BTreeSet<String>,
) -> Vec<TransactionEntry> {
    let mut entries = Vec::new();
    for proposal in &plan.proposals {
        if !is_applicable_proposal(proposal) {
            continue;
        }
        if !approved_paths.contains(&proposal.source_path.to_string_lossy().into_owned()) {
            continue;
        }
        let Some(proposed) = &proposal.proposed_basename else {
            continue;
        };
        let Some(parent) = proposal.source_path.parent() else {
            continue;
        };
        let Ok(current_identity) = capture_identity(&proposal.source_path) else {
            // The source is gone between plan and build; the entry cannot be
            // built with a recorded identity, so it is not included.
            continue;
        };
        let identity = if let Some(audited) = proposal.audited_identity.as_ref() {
            if !super::identity::identity_matches(audited, &current_identity) {
                continue;
            }
            audited.clone()
        } else {
            current_identity
        };
        entries.push(TransactionEntry {
            source_path: proposal.source_path.clone(),
            destination_path: parent.join(proposed),
            original_basename: proposal.current_basename.clone(),
            proposed_basename: proposed.clone(),
            identity,
            operation: Default::default(),
            preflight_passed: false,
            preflight_failures: Vec::new(),
            state: EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        });
    }
    entries
}

/// Whether a proposal may ever be applied: it must be a Suggested, actionable
/// proposal with no collision, a proposed name, and a regular-file source.
fn is_applicable_proposal(proposal: &RenameProposal) -> bool {
    use crate::dat::rename_plan::{ProposalState, SourceObjectKind};
    proposal.state == ProposalState::Suggested
        && proposal.actionable
        && proposal.collision.is_none()
        && proposal.proposed_basename.is_some()
        && proposal.object_kind == SourceObjectKind::RegularFile
}

/// The approval helper the GUI uses: does a recorded decision count as
/// approved for apply?
pub fn is_approved(decision: &crate::dat::rename_plan::ReviewDecision) -> bool {
    *decision == crate::dat::rename_plan::ReviewDecision::AcceptedForReview
}

/// Everything the executor needs to apply a transaction that was built at
/// review time.
#[derive(Debug)]
pub struct ApplyExecution<'a> {
    /// The transaction to apply (built by [`build_transaction`]).
    pub transaction: &'a mut RenameTransaction,
    /// The approved source paths the transaction was built from, re-checked
    /// by preflight.
    pub approved_paths: BTreeSet<String>,
    /// The current plan generation; must equal the transaction's.
    pub current_generation: u64,
    pub trusted: TrustedRoots,
    pub journal_dir: PathBuf,
    pub hard_conflict_mode: HardConflictMode,
    pub cancel: &'a AtomicBool,
    /// Whether the destination may live outside the source's directory.
    /// `SameDirectory` (the default) preserves rename-apply's behaviour.
    pub directory_policy: super::preflight::DirectoryPolicy,
    /// Whether a symlink object is an acceptable source (the ROM organiser's
    /// symlink-only mode). The link object itself is renamed; its target is
    /// never dereferenced and its identity is still re-verified.
    pub allow_symlink_source: bool,
}

/// Applies a prebuilt transaction. This is the only place a rename happens.
pub fn apply_transaction(execution: &mut ApplyExecution<'_>) -> Result<ApplyOutcome, ApplyError> {
    crate::launch::es_de_publish::operation::require_independent(
        &execution.journal_dir,
        execution.transaction,
    )
    .map_err(|error| ApplyError::Journal(error.to_string()))?;
    apply_transaction_inner(execution)
}

// Only the receipt-owning coordinator calls this after binding its child.
pub(crate) fn apply_retrodeck_transaction(
    execution: &mut ApplyExecution<'_>,
) -> Result<ApplyOutcome, ApplyError> {
    crate::launch::es_de_publish::operation::validate_child(
        &execution.journal_dir,
        execution.transaction,
    )
    .map_err(|error| ApplyError::Journal(error.to_string()))?;
    apply_transaction_inner(execution)
}

fn apply_transaction_inner(execution: &mut ApplyExecution<'_>) -> Result<ApplyOutcome, ApplyError> {
    let _live = super::journal::mark_transaction_live(
        &execution.journal_dir,
        &execution.transaction.transaction_id,
    );
    validate_classifier_version(execution.transaction.classifier_version.as_deref())?;
    if cancelled(execution.cancel) {
        return Err(ApplyError::Cancelled);
    }
    let transaction = &mut *execution.transaction;
    // After a rollback every source is back at its recorded identity, so
    // preflight alone would accept the transaction again and silently replay
    // a reversed batch. Refused before preflight or any journal write, for
    // every caller of the shared executor.
    if matches!(
        transaction.state,
        TransactionState::RolledBack
            | TransactionState::RollingBack
            | TransactionState::RollbackFailed
            | TransactionState::Finalising
            | TransactionState::Finalised
    ) {
        return Err(ApplyError::AlreadySettled {
            transaction_id: transaction.transaction_id.clone(),
            state: transaction.state,
        });
    }
    if transaction.plan_generation != execution.current_generation {
        return Err(ApplyError::StalePlan {
            plan: transaction.plan_generation,
            current: execution.current_generation,
        });
    }

    let destinations = batch_destinations(&transaction.entries);
    let preflight_options = PreflightOptions {
        plan_generation: transaction.plan_generation,
        current_generation: execution.current_generation,
        approved_paths: &execution.approved_paths,
        trusted: &execution.trusted,
        batch_destinations: &destinations,
        directory_policy: execution.directory_policy,
        allow_symlink_source: execution.allow_symlink_source,
    };

    // Preflight the whole batch first.
    let mut hard_conflicts: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for entry in &mut transaction.entries {
        if let Err(failures) = run_preflight(entry, &preflight_options) {
            entry.preflight_failures = failures.iter().map(|f| f.reason()).collect();
            entry.preflight_passed = false;
            hard_conflicts.push((entry.source_path.clone(), entry.preflight_failures.clone()));
        } else {
            entry.preflight_passed = true;
        }
    }

    match execution.hard_conflict_mode {
        HardConflictMode::AbortAll => {
            if !hard_conflicts.is_empty() {
                return Err(ApplyError::HardConflicts(hard_conflicts));
            }
        }
        HardConflictMode::SkipUnsafeSubset => {
            for entry in &mut transaction.entries {
                if !entry.preflight_passed {
                    entry.state = EntryState::Skipped;
                }
            }
        }
    }

    // Journal the exact batch before the first mutation.
    write_journal(&execution.journal_dir, transaction)
        .map_err(|error| ApplyError::Journal(error.to_string()))?;

    if transaction
        .entries
        .iter()
        .all(|e| e.state == EntryState::Skipped)
    {
        transaction.state = TransactionState::ApplyFailed;
        if super::exact_resume::has_envelope(transaction) {
            super::exact_resume::set_state(transaction, super::model::ExactResumeState::Failed);
        }
        write_journal(&execution.journal_dir, transaction)
            .map_err(|error| ApplyError::Journal(error.to_string()))?;
        let summary = TransactionSummary::from_transaction(transaction);
        return Ok(ApplyOutcome {
            transaction: transaction.clone(),
            summary,
        });
    }

    transaction.state = TransactionState::Applying;
    write_journal(&execution.journal_dir, transaction)
        .map_err(|error| ApplyError::Journal(error.to_string()))?;

    // Apply one at a time. For each entry the journal is durably checkpointed
    // to `Applying` BEFORE the rename syscall, so a crash at any point leaves
    // a recoverable record (the file is either at the source with the entry
    // Planned/Applying, or at the destination with the entry Applying, and
    // recovery reconciles either case).
    for index in 0..transaction.entries.len() {
        if cancelled(execution.cancel) {
            transaction.state = TransactionState::ApplyFailed;
            if super::exact_resume::has_envelope(transaction) {
                super::exact_resume::set_state(
                    transaction,
                    super::model::ExactResumeState::Interrupted,
                );
            }
            write_journal(&execution.journal_dir, transaction)
                .map_err(|error| ApplyError::Journal(error.to_string()))?;
            let summary = TransactionSummary::from_transaction(transaction);
            return Ok(ApplyOutcome {
                transaction: transaction.clone(),
                summary,
            });
        }
        if transaction.entries[index].state == EntryState::Skipped {
            continue;
        }

        // Preflight (before entering the mutation window).
        if let Err(failures) = run_preflight(&transaction.entries[index], &preflight_options) {
            transaction.entries[index].preflight_passed = false;
            transaction.entries[index].preflight_failures =
                failures.iter().map(|f| f.reason()).collect();
            transaction.entries[index].state = EntryState::ApplyFailed;
            transaction.entries[index].failure_reason = Some(
                failures
                    .iter()
                    .map(|f| f.reason())
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            transaction.state = TransactionState::ApplyFailed;
            if super::exact_resume::has_envelope(transaction) {
                super::exact_resume::set_state(transaction, super::model::ExactResumeState::Failed);
            }
            write_journal(&execution.journal_dir, transaction)
                .map_err(|error| ApplyError::Journal(error.to_string()))?;
            break;
        }
        transaction.entries[index].preflight_passed = true;

        // A fresh link entry journals, before the checkpoint below, whether its
        // already-correct link existed beforehand, so Undo never removes a link
        // this transaction did not create (see `LINK_ADOPTED_KEY`).
        let catalogue =
            crate::database::organisation_paths::begin(&transaction.entries[index], false, false)
                .map_err(|error| ApplyError::Journal(error.to_string()))?;
        let prior_state = transaction.entries[index].state;
        if prior_state == EntryState::Planned {
            if let Err(reason) = record_link_adoption(&mut transaction.entries[index]) {
                transaction.entries[index].state = EntryState::ApplyFailed;
                transaction.entries[index].failure_reason = Some(reason);
                transaction.state = TransactionState::ApplyFailed;
                if super::exact_resume::has_envelope(transaction) {
                    super::exact_resume::set_state(
                        transaction,
                        super::model::ExactResumeState::Failed,
                    );
                }
                write_journal(&execution.journal_dir, transaction)
                    .map_err(|error| ApplyError::Journal(error.to_string()))?;
                break;
            }
        }

        let prepared_copy = match super::copy::prepare(&mut transaction.entries[index]) {
            Ok(prepared) => prepared,
            Err(reason) => {
                transaction.entries[index].state = EntryState::ApplyFailed;
                transaction.entries[index].failure_reason = Some(reason);
                transaction.state = TransactionState::ApplyFailed;
                write_journal(&execution.journal_dir, transaction)
                    .map_err(|e| ApplyError::Journal(e.to_string()))?;
                break;
            }
        };
        // Durable `Applying` checkpoint BEFORE the rename syscall. If this
        // write fails, no mutation may happen.
        transaction.entries[index].state = EntryState::Applying;
        #[cfg(target_os = "linux")]
        if let Some(result) = super::compact_recovery::fd_prepare_apply_entry(
            &execution.journal_dir,
            transaction,
            index,
        ) {
            result.map_err(|e| ApplyError::Journal(e.to_string()))?;
        }
        write_journal(&execution.journal_dir, transaction)
            .map_err(|error| ApplyError::Journal(error.to_string()))?;

        // Mutation window: the rename syscall plus the filesystem
        // confirmation. Nothing else runs while the file is in flight.
        #[cfg(target_os = "linux")]
        let mutation =
            super::compact_recovery::fd_apply_entry(&execution.journal_dir, transaction, index);
        #[cfg(not(target_os = "linux"))]
        let mutation: Option<Result<(), (EntryState, String)>> = None;
        // An entry already applied, or interrupted after its checkpoint (the
        // crash may have followed the link syscall), re-verifies its own
        // journalled link - the conclusion `reconcile_recovery` also reaches.
        let mutation = mutation.or_else(|| {
            match prior_state {
                EntryState::Applied => {
                    reverify_recorded_link(&transaction.entries[index], prior_state)
                }
                // Interrupted before the syscall: no link yet, so create it.
                // Unavailable evidence is kept, never treated as "no link".
                EntryState::Applying => {
                    reverify_recorded_link(&transaction.entries[index], prior_state)
                        .filter(|result| !matches!(result, Err((EntryState::ApplyFailed, _))))
                }
                _ => None,
            }
        });
        let mutation = prepared_copy.map(|copy| copy.publish()).or(mutation);
        match mutation
            .unwrap_or_else(|| apply_mutation_in(&transaction.entries[index], &execution.trusted))
        {
            Ok(()) => {
                if let TransactionOperation::CreateSymlink {
                    expected_target, ..
                } = &transaction.entries[index].operation
                {
                    if link_adoption(&transaction.entries[index]) == Some(false)
                        && transaction.entries[index].created_link_identity().is_none()
                    {
                        let proof = capture_identity(&transaction.entries[index].destination_path);
                        let target =
                            std::fs::read_link(&transaction.entries[index].destination_path);
                        match (proof, target) {
                            (Ok(proof), Ok(target))
                                if proof.kind == super::model::ObjectKind::Symlink
                                    && target == *expected_target =>
                            {
                                transaction.entries[index]
                                    .record_created_link(&proof)
                                    .map_err(|error| ApplyError::Journal(error.to_string()))?;
                            }
                            _ => {
                                transaction.entries[index].state = EntryState::ApplyFailed;
                                transaction.entries[index].failure_reason = Some(
                                    "created link could not be proved; review the journal".into(),
                                );
                                transaction.state = TransactionState::ApplyFailed;
                                write_journal(&execution.journal_dir, transaction)
                                    .map_err(|error| ApplyError::Journal(error.to_string()))?;
                                break;
                            }
                        }
                    }
                }
                if let Some(catalogue) = catalogue {
                    catalogue
                        .commit()
                        .map_err(|error| ApplyError::Journal(error.to_string()))?;
                }
                transaction.entries[index].state = EntryState::Applied;
                transaction.entries[index].applied_at_unix = Some(crate::dat::sources::now_unix());
            }
            Err((state, reason)) => {
                let uncertain_link = matches!(
                    &transaction.entries[index].operation,
                    TransactionOperation::CreateSymlink { expected_target, .. }
                        if link_adoption(&transaction.entries[index]) == Some(false)
                            && std::fs::read_link(&transaction.entries[index].destination_path)
                                .is_ok_and(|target| target == *expected_target)
                );
                transaction.entries[index].state = if uncertain_link {
                    EntryState::Applying
                } else {
                    state
                };
                transaction.entries[index].failure_reason = Some(if uncertain_link {
                    format!(
                        "{reason}; a link may have been created but ownership is unproved; recovery requires review"
                    )
                } else {
                    reason
                });
                transaction.state = TransactionState::ApplyFailed;
                if super::exact_resume::has_envelope(transaction) {
                    super::exact_resume::set_state(
                        transaction,
                        super::model::ExactResumeState::Failed,
                    );
                }
                write_journal(&execution.journal_dir, transaction)
                    .map_err(|error| ApplyError::Journal(error.to_string()))?;
                break;
            }
        }
        write_journal(&execution.journal_dir, transaction)
            .map_err(|error| ApplyError::Journal(error.to_string()))?;
    }

    if transaction.state == TransactionState::Applying {
        transaction.state = TransactionState::Applied;
        if super::exact_resume::has_envelope(transaction) {
            super::exact_resume::set_state(transaction, super::model::ExactResumeState::Completed);
        }
    }
    write_journal(&execution.journal_dir, transaction)
        .map_err(|error| ApplyError::Journal(error.to_string()))?;

    let summary = TransactionSummary::from_transaction(transaction);
    Ok(ApplyOutcome {
        transaction: transaction.clone(),
        summary,
    })
}

/// Rejects plans whose classification provenance is missing or differs from
/// the current classifier. This check is intentionally performed before any
/// journal write or filesystem mutation.
pub(crate) fn validate_classifier_version(plan_version: Option<&str>) -> Result<(), ApplyError> {
    let current = crate::dat::classification::CLASSIFIER_VERSION;
    if plan_version == Some(current) {
        Ok(())
    } else {
        Err(ApplyError::StaleClassifierVersion {
            plan: plan_version.map(str::to_string),
            current: current.to_string(),
        })
    }
}

/// The mutation window: the no-clobber rename and the filesystem confirmation.
///
/// Called only after the entry's `Applying` state has been durably persisted.
/// On success returns `Ok`; on any failure returns the entry state to record
/// (`ApplyFailed`) and the exact reason.
///
/// `pub(crate)` (rather than private): reused, unchanged, by
/// [`crate::repair::quarantine`]'s own per-entry apply loop, which needs to
/// interleave a domain-specific re-proof between entries and so cannot
/// delegate the whole batch to [`apply_transaction`]. Semantics here are not
/// altered by that reuse.
#[cfg(test)]
pub(crate) fn apply_mutation(entry: &TransactionEntry) -> Result<(), (EntryState, String)> {
    apply_mutation_in(entry, &crate::safe_read::TrustedRoots::none())
}

/// [`apply_mutation`] with the transaction's trusted roots: the rename's parent
/// directories are then resolved FROM those roots with per-component
/// `openat(O_NOFOLLOW)`, so an ancestor replaced after preflight is refused
/// rather than followed outside the root.
pub(crate) fn apply_mutation_in(
    entry: &TransactionEntry,
    trusted: &crate::safe_read::TrustedRoots,
) -> Result<(), (EntryState, String)> {
    if matches!(
        entry.operation,
        TransactionOperation::ReplaceExisting { .. }
    ) {
        return super::replacement::apply(entry);
    }
    if let TransactionOperation::CreateSymlink {
        expected_target,
        destination_root,
    } = &entry.operation
    {
        return apply_symlink_mutation(entry, expected_target, destination_root, trusted)
            .map(|_| ());
    }
    if let TransactionOperation::CreateHardlink {
        expected_source,
        destination_root,
    } = &entry.operation
    {
        return apply_hardlink_mutation(entry, expected_source, destination_root);
    }
    if let TransactionOperation::CreateCopy {
        expected_source,
        destination_root,
    } = &entry.operation
    {
        return apply_copy_mutation(entry, expected_source, destination_root);
    }
    // Bind BOTH parent directories (O_NOFOLLOW + identity checked) before anything
    // is touched. The rename below is issued against these descriptors, the source
    // is re-proved inside the bound source directory, and the same descriptors are
    // fsynced, so a directory replaced after this point cannot redirect the
    // rename or receive the durability sync in place of the real one.
    let dirs = super::dirsync::RenameDirs::open_in(&entry.source_path, &entry.destination_path, trusted)
        .map_err(|error| {
            (
                EntryState::ApplyFailed,
                format!("could not bind the parent directories before the rename: {error}; nothing was moved"),
            )
        })?;
    // Re-prove the source immediately before the rename. Preflight validated
    // an earlier moment; a changed or swapped source is refused without
    // touching it.
    match capture_identity(&dirs.source_entry()) {
        Ok(current) if super::identity::identity_matches(&entry.identity, &current) => {}
        Ok(_) => {
            return Err((
                EntryState::ApplyFailed,
                "the source changed after review; it was not moved".to_string(),
            ));
        }
        Err(error) => {
            return Err((
                EntryState::ApplyFailed,
                format!("could not re-verify the source before the rename: {error}"),
            ));
        }
    }
    run_test_before_rename(&entry.source_path, &entry.destination_path);
    match dirs.rename_noreplace(false) {
        Ok(()) => {
            // The filesystem must confirm the rename (inside the bound
            // directories) before Applied.
            match confirm_rename_bound(entry, &dirs) {
                Ok(()) => {
                    // The bound directories must be durable, and must still be
                    // the directories the paths name, before success is recorded.
                    // The rename has already happened, so a failure leaves the
                    // entry in flight (`Applying`), never `Applied` and never
                    // reported as unchanged; it is not retried.
                    dirs.sync_and_verify(false).map_err(|error| {
                        (
                            EntryState::Applying,
                            format!(
                                "the rename completed but its directory entries could not be \
                                 synchronised ({error}); the file is at the destination and the \
                                 completion was not recorded - review before any further action"
                            ),
                        )
                    })
                }
                Err(reason) => restore_failed_rename(entry, &dirs, reason),
            }
        }
        Err(NoClobberError::DestinationExists) => Err((
            EntryState::ApplyFailed,
            "the destination appeared during apply and was never overwritten".to_string(),
        )),
        Err(error) => Err((EntryState::ApplyFailed, error.to_string())),
    }
}

/// A swap landing between the source re-proof and the rename can still move a
/// replacement object. Never report that as success: if the object now at the
/// destination is not the reviewed one and the source name is free, put it back
/// (descriptor-bound, no overwrite). A restore failure is reported, not hidden.
fn restore_failed_rename(
    entry: &TransactionEntry,
    dirs: &super::dirsync::RenameDirs,
    reason: String,
) -> Result<(), (EntryState, String)> {
    let moved_foreign_object = std::fs::symlink_metadata(dirs.source_entry()).is_err()
        && matches!(
            capture_identity(&dirs.destination_entry()),
            Ok(current) if !super::identity::identity_matches(&entry.identity, &current)
        );
    if moved_foreign_object {
        return match dirs.rename_noreplace(true) {
            Ok(()) => {
                // The rename AND its reversal both happened; only a durable
                // reversal settles the entry as an unchanged failure. If the sync
                // fails the on-disk directory entries are not proven, so the entry
                // stays in flight (`Applying`): never `ApplyFailed`/settled, which
                // would let a crash resurface the moved replacement and a replay
                // proceed on an unproven state.
                match dirs.sync_and_verify(true) {
                    Ok(()) => Err((
                        EntryState::ApplyFailed,
                        format!("{reason}; the replacement object was restored to the source path"),
                    )),
                    Err(error) => Err((
                        EntryState::Applying,
                        format!(
                            "{reason}; the replacement object was restored to the source path \
                             but the directory entries could not be synchronised ({error}); the \
                             restore was not recorded as durable - review before any further action"
                        ),
                    )),
                }
            }
            Err(error) => Err((
                EntryState::ApplyFailed,
                format!("{reason}; could not restore the moved replacement: {error}"),
            )),
        };
    }
    Err((EntryState::ApplyFailed, reason))
}

#[cfg(test)]
thread_local! {
    static TEST_BEFORE_RENAME: std::cell::Cell<Option<fn(&std::path::Path)>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_before_rename_hook(hook: Option<fn(&std::path::Path)>) {
    TEST_BEFORE_RENAME.with(|cell| cell.set(hook));
}

#[cfg(test)]
fn run_test_before_rename(source: &std::path::Path, _destination: &std::path::Path) {
    TEST_BEFORE_RENAME.with(|cell| {
        if let Some(hook) = cell.take() {
            hook(source);
        }
    });
}

#[cfg(not(test))]
fn run_test_before_rename(_source: &std::path::Path, _destination: &std::path::Path) {}

fn apply_copy_mutation(
    _entry: &TransactionEntry,
    _expected_source: &std::path::Path,
    _destination_root: &std::path::Path,
) -> Result<(), (EntryState, String)> {
    Err((
        EntryState::ApplyFailed,
        "copy requires journalled exclusive staging through apply_transaction".into(),
    ))
}

fn apply_hardlink_mutation(
    entry: &TransactionEntry,
    expected_source: &std::path::Path,
    destination_root: &std::path::Path,
) -> Result<(), (EntryState, String)> {
    if !expected_source.is_absolute()
        || expected_source != entry.source_path
        || !super::preflight::destination_is_confined(&entry.destination_path, destination_root)
    {
        return Err((
            EntryState::ApplyFailed,
            "invalid journalled hardlink source or destination".to_string(),
        ));
    }
    let source = capture_identity(&entry.source_path).map_err(|_| {
        (
            EntryState::ApplyFailed,
            "hardlink source no longer exists".to_string(),
        )
    })?;
    if source.kind != super::model::ObjectKind::RegularFile
        || !super::identity::identity_matches(&entry.identity, &source)
    {
        return Err((
            EntryState::ApplyFailed,
            "hardlink source changed since review".to_string(),
        ));
    }
    match std::fs::symlink_metadata(&entry.destination_path) {
        Ok(_) => {
            return Err((
                EntryState::ApplyFailed,
                "destination already exists and was not replaced".to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err((EntryState::ApplyFailed, error.to_string())),
    }
    std::fs::hard_link(expected_source, &entry.destination_path)
        .map_err(|error| (EntryState::ApplyFailed, error.to_string()))?;
    let source_after = capture_identity(&entry.source_path).map_err(|_| {
        (
            EntryState::ApplyFailed,
            "hardlink source disappeared".to_string(),
        )
    })?;
    let destination_after = capture_identity(&entry.destination_path).map_err(|_| {
        (
            EntryState::ApplyFailed,
            "created hardlink destination disappeared".to_string(),
        )
    })?;
    if super::identity::identity_matches(&entry.identity, &source_after)
        && super::identity::identity_matches(&entry.identity, &destination_after)
    {
        Ok(())
    } else {
        Err((
            EntryState::ApplyFailed,
            "created hardlink failed identity verification".to_string(),
        ))
    }
}

/// Entry metadata recording, at the pre-mutation checkpoint, whether a
/// `CreateSymlink` entry found its exact link already present (`true`: the
/// link is adopted, never created, and Undo leaves it in place) or absent
/// (`false`: the link must be created by this entry). Absent on journals
/// written before this key existed, which keep the earlier behaviour.
pub(crate) const LINK_ADOPTED_KEY: &str = "emuwiz_link_adopted_v1";

/// The recorded adoption decision for a link entry, if any.
pub(crate) fn link_adoption(entry: &TransactionEntry) -> Option<bool> {
    match &entry.operation {
        TransactionOperation::CreateSymlink { .. } => entry
            .unknown
            .get(LINK_ADOPTED_KEY)
            .and_then(|v| v.as_bool()),
        _ => None,
    }
}

/// Whether the exact link is present. Only `NotFound` proves absence: any other
/// error is unavailable evidence, never "absent".
fn probe_exact_link(
    entry: &TransactionEntry,
    expected_target: &std::path::Path,
) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(&entry.destination_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
        Ok(metadata) if !metadata.file_type().is_symlink() => Ok(false),
        Ok(_) => Ok(std::fs::read_link(&entry.destination_path)? == expected_target),
    }
}

/// Journals the adoption decision; unavailable link evidence records nothing
/// and returns the reason, so the caller fails the entry before its checkpoint.
pub(crate) fn record_link_adoption(entry: &mut TransactionEntry) -> Result<(), String> {
    let TransactionOperation::CreateSymlink {
        expected_target, ..
    } = &entry.operation
    else {
        return Ok(());
    };
    let adopted = probe_exact_link(entry, expected_target)
        .map_err(|error| format!("link evidence unavailable: {error}"))?;
    entry.unknown.insert(
        LINK_ADOPTED_KEY.to_string(),
        serde_json::Value::Bool(adopted),
    );
    Ok(())
}

/// Repeating Apply on a link entry this transaction already applied (or was
/// applying when interrupted): its own recorded link must be exactly in place.
/// Nothing is created; an absent link fails the entry for review.
pub(crate) fn reverify_recorded_link(
    entry: &TransactionEntry,
    prior_state: EntryState,
) -> Option<Result<(), (EntryState, String)>> {
    link_adoption(entry)?;
    let TransactionOperation::CreateSymlink {
        expected_target, ..
    } = &entry.operation
    else {
        return None;
    };
    Some(match probe_exact_link(entry, expected_target) {
        Ok(true) => Ok(()),
        Ok(false) => Err((
            EntryState::ApplyFailed,
            "the link recorded by this transaction is no longer in place".to_string(),
        )),
        // Unavailable evidence keeps the entry's outcome (and its Undo
        // eligibility); only the transaction fails.
        Err(error) => Err((prior_state, format!("link evidence unavailable: {error}"))),
    })
}

fn apply_symlink_mutation(
    entry: &TransactionEntry,
    expected_target: &std::path::Path,
    destination_root: &std::path::Path,
    trusted: &crate::safe_read::TrustedRoots,
) -> Result<(), (EntryState, String)> {
    if !expected_target.is_absolute() || expected_target != entry.source_path {
        return Err((
            EntryState::ApplyFailed,
            "invalid journalled symlink target".to_string(),
        ));
    }
    if !super::preflight::destination_is_confined(&entry.destination_path, destination_root) {
        return Err((
            EntryState::ApplyFailed,
            "link destination is outside its approved root".to_string(),
        ));
    }
    let source = capture_identity(&entry.source_path).map_err(|_| {
        (
            EntryState::ApplyFailed,
            "link source no longer exists".to_string(),
        )
    })?;
    if source.kind != super::model::ObjectKind::RegularFile
        || !super::identity::identity_matches(&entry.identity, &source)
    {
        return Err((
            EntryState::ApplyFailed,
            "link source changed since review".to_string(),
        ));
    }
    let adoption = link_adoption(entry);
    match std::fs::symlink_metadata(&entry.destination_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            match std::fs::read_link(&entry.destination_path) {
                Ok(target) if target == expected_target && adoption != Some(false) => {
                    return Ok(());
                }
                Ok(target) if target == expected_target => {
                    return Err((
                        EntryState::ApplyFailed,
                        "a link appeared after this entry's checkpoint and was not created by \
                         this transaction"
                            .to_string(),
                    ));
                }
                _ => {
                    return Err((
                        EntryState::ApplyFailed,
                        "destination symlink points elsewhere".to_string(),
                    ));
                }
            }
        }
        Ok(_) => {
            return Err((
                EntryState::ApplyFailed,
                "destination already exists and was not replaced".to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if adoption == Some(true) {
                return Err((
                    EntryState::ApplyFailed,
                    "the adopted link disappeared before it could be confirmed".to_string(),
                ));
            }
        }
        Err(error) => return Err((EntryState::ApplyFailed, error.to_string())),
    }
    #[cfg(unix)]
    {
        let dirs = super::dirsync::RenameDirs::open_in(
            &entry.source_path,
            &entry.destination_path,
            trusted,
        )
        .map_err(|error| {
            (
                EntryState::ApplyFailed,
                format!("could not bind link directories: {error}"),
            )
        })?;
        let bound_source = capture_identity(&dirs.source_entry()).map_err(|error| {
            (
                EntryState::ApplyFailed,
                format!("bound link source is unavailable: {error}"),
            )
        })?;
        if !super::identity::identity_matches(&entry.identity, &bound_source) {
            return Err((
                EntryState::ApplyFailed,
                "bound link source changed since review".into(),
            ));
        }
        dirs.create_link(expected_target)
            .map_err(|error| (EntryState::ApplyFailed, error.to_string()))?;
        let source_after = capture_identity(&dirs.source_entry()).map_err(|_| {
            (
                EntryState::ApplyFailed,
                "link source disappeared".to_string(),
            )
        })?;
        if super::preflight::destination_is_confined(&entry.destination_path, destination_root)
            && super::identity::identity_matches(&entry.identity, &source_after)
        {
            return Ok(());
        }
        return Err((
            EntryState::ApplyFailed,
            "created link failed verification".into(),
        ));
    }
    #[cfg(not(unix))]
    return Err((
        EntryState::ApplyFailed,
        "symlink creation is unsupported on this platform".to_string(),
    ));
}

/// Confirms, from the filesystem and INSIDE the bound directories, that a rename
/// actually happened: the source is gone and the destination exists with the
/// recorded identity.
fn confirm_rename_bound(
    entry: &TransactionEntry,
    dirs: &super::dirsync::RenameDirs,
) -> Result<(), String> {
    if std::fs::symlink_metadata(dirs.source_entry()).is_ok() {
        return Err("the source still exists after the rename".to_string());
    }
    match capture_identity(&dirs.destination_entry()) {
        Err(_) => Err("the destination does not exist after the rename".to_string()),
        Ok(current) => {
            if super::identity::identity_matches(&entry.identity, &current) {
                Ok(())
            } else {
                Err("the destination identity does not match the recorded source".to_string())
            }
        }
    }
}

fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}
