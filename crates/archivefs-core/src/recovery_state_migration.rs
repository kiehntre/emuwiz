//! Read-only projection of rename/recovery state into source-root migration.
//!
//! Transaction journals contain both paths that may still participate in a
//! filesystem mutation and paths that are only historical evidence.  This
//! adapter keeps that distinction explicit.  Only live source paths are sent
//! to the generic exact-containment planner.  Destinations, symlink targets,
//! and exact-resume approval evidence are deliberately review-only: a path
//! being under a source root is not proof that its transaction semantics may
//! be rebased.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::dat::rename_apply::journal::list_journals;
use crate::dat::rename_apply::model::{
    EntryState, ExactResumeEnvelope, RenameTransaction, TransactionOperation, TransactionState,
};
use crate::source_root_migration::{
    MigrationClassification, MigrationProposal, MigrationReference, PathSemantics,
    SourceRootMigrationPlan, SubsystemSnapshot, plan_rebase, plan_source_root_migration,
};

const EXACT_RESUME_ENVELOPE_KEY: &str = "emuwiz_exact_resume_envelope";
const SUBSYSTEM: &str = "rename-recovery-state";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMigrationRole {
    LiveSource,
    LiveDestination,
    ExactResumeSource,
    ExactResumeDestination,
    SymlinkExpectedTarget,
    CreatedDirectory,
    TransactionScanRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMigrationDisposition {
    Planner,
    HistoricalOnly,
    DestinationOwned,
    External,
    RollbackPathChangedReviewRequired,
    InvalidTechnicalReview,
}

/// One bounded path finding tied back to its durable transaction and entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryMigrationReference {
    pub transaction_id: String,
    pub entry_index: Option<usize>,
    pub role: RecoveryMigrationRole,
    pub path: PathBuf,
    pub disposition: RecoveryMigrationDisposition,
    pub proposal: MigrationProposal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct RecoveryMigrationTotals {
    pub journals: usize,
    pub parse_problems: usize,
    pub settled_historical: usize,
    pub live_actionable: usize,
    pub exact_resume: usize,
    pub legacy: usize,
    pub invalid_technical_review: usize,
    pub source_candidates: usize,
    pub source_already_current: usize,
    pub source_manual_review: usize,
    pub destination_owned_review: usize,
    pub external_references: usize,
    pub rollback_path_review: usize,
    pub conflicts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryMigrationReport {
    pub source_root_plan: SourceRootMigrationPlan,
    pub references: Vec<RecoveryMigrationReference>,
    pub exact_resume: Vec<MigrationProposal>,
    pub totals: RecoveryMigrationTotals,
    pub journal_problems: Vec<String>,
}

/// Reads and plans every JSON journal without opening a journal for writing.
pub fn plan_recovery_state_migration_from_dir(
    old_root: impl AsRef<Path>,
    new_root: impl AsRef<Path>,
    journal_dir: impl AsRef<Path>,
) -> RecoveryMigrationReport {
    let (transactions, problems) = list_journals(journal_dir.as_ref());
    let mut report = plan_recovery_state_migration(old_root, new_root, &transactions);
    report.totals.journals = transactions.len() + problems.len();
    report.totals.parse_problems = problems.len();
    report.journal_problems = problems;
    report
}

/// Projects parsed transactions into the generic source-root planner.
///
/// This function is pure with respect to journals and the filesystem: the
/// generic planner may inspect candidate existence, but no transaction,
/// journal, approval envelope, or file is changed.  A source candidate is
/// safe only when the existing planner proves exact root containment,
/// suffix-preserving rebasing, target existence, and symlink safety.
pub fn plan_recovery_state_migration(
    old_root: impl AsRef<Path>,
    new_root: impl AsRef<Path>,
    transactions: &[RenameTransaction],
) -> RecoveryMigrationReport {
    let old_root = old_root.as_ref();
    let new_root = new_root.as_ref();
    let mut source_references = Vec::new();
    let mut references = Vec::new();
    let mut exact_resume = Vec::new();
    let mut totals = RecoveryMigrationTotals {
        journals: transactions.len(),
        ..Default::default()
    };

    for transaction in transactions {
        let invalid = technically_invalid(transaction);
        let exact = read_exact_envelope(transaction);
        if exact.is_some() {
            totals.exact_resume += 1;
        } else {
            totals.legacy += 1;
        }

        if invalid {
            totals.invalid_technical_review += 1;
            add_transaction_review_references(
                transaction,
                old_root,
                new_root,
                RecoveryMigrationDisposition::InvalidTechnicalReview,
                &mut references,
            );
            continue;
        }
        if is_historical(transaction) {
            totals.settled_historical += 1;
            for (index, entry) in transaction.entries.iter().enumerate() {
                for (role, path) in [
                    (RecoveryMigrationRole::LiveSource, &entry.source_path),
                    (
                        RecoveryMigrationRole::LiveDestination,
                        &entry.destination_path,
                    ),
                ] {
                    references.push(historical_reference(
                        &transaction.transaction_id,
                        Some(index),
                        role,
                        path,
                        old_root,
                        new_root,
                    ));
                }
                if let TransactionOperation::CreateSymlink {
                    destination_root, ..
                } = &entry.operation
                {
                    references.push(historical_reference(
                        &transaction.transaction_id,
                        Some(index),
                        RecoveryMigrationRole::CreatedDirectory,
                        destination_root,
                        old_root,
                        new_root,
                    ));
                }
            }
            references.push(historical_reference(
                &transaction.transaction_id,
                None,
                RecoveryMigrationRole::TransactionScanRoot,
                Path::new(&transaction.source_scan_root),
                old_root,
                new_root,
            ));
            for path in &transaction.created_directories {
                references.push(historical_reference(
                    &transaction.transaction_id,
                    None,
                    RecoveryMigrationRole::CreatedDirectory,
                    path,
                    old_root,
                    new_root,
                ));
            }
            continue;
        }

        totals.live_actionable += 1;
        for (index, entry) in transaction.entries.iter().enumerate() {
            let source_id = format!("{}:entry:{index}:source", transaction.transaction_id);
            source_references.push(MigrationReference {
                id: source_id,
                subsystem: SUBSYSTEM.to_string(),
                path: entry.source_path.clone(),
                semantics: PathSemantics::SourceRootAbsolute,
                migratable: true,
                requires_existence: true,
                historical: false,
                live: true,
                claimed_destination: None,
            });
            references.push(RecoveryMigrationReference {
                transaction_id: transaction.transaction_id.clone(),
                entry_index: Some(index),
                role: RecoveryMigrationRole::LiveSource,
                path: entry.source_path.clone(),
                disposition: RecoveryMigrationDisposition::Planner,
                proposal: placeholder_proposal(&source_references[source_references.len() - 1]),
            });
            let disposition = if entry.state == EntryState::Applied {
                totals.rollback_path_review += 1;
                RecoveryMigrationDisposition::RollbackPathChangedReviewRequired
            } else {
                RecoveryMigrationDisposition::DestinationOwned
            };
            totals.destination_owned_review += 1;
            references.push(review_reference(
                &transaction.transaction_id,
                Some(index),
                RecoveryMigrationRole::LiveDestination,
                &entry.destination_path,
                disposition,
                old_root,
                new_root,
            ));
            if let TransactionOperation::CreateSymlink {
                expected_target,
                destination_root,
            } = &entry.operation
            {
                let disposition = if expected_target.is_absolute()
                    && !expected_target.starts_with(old_root)
                    && !expected_target.starts_with(new_root)
                {
                    totals.external_references += 1;
                    RecoveryMigrationDisposition::External
                } else {
                    totals.destination_owned_review += 1;
                    RecoveryMigrationDisposition::DestinationOwned
                };
                references.push(review_reference(
                    &transaction.transaction_id,
                    Some(index),
                    RecoveryMigrationRole::SymlinkExpectedTarget,
                    expected_target,
                    disposition,
                    old_root,
                    new_root,
                ));
                references.push(review_reference(
                    &transaction.transaction_id,
                    Some(index),
                    RecoveryMigrationRole::CreatedDirectory,
                    destination_root,
                    RecoveryMigrationDisposition::DestinationOwned,
                    old_root,
                    new_root,
                ));
            }
        }
        references.push(review_reference(
            &transaction.transaction_id,
            None,
            RecoveryMigrationRole::TransactionScanRoot,
            Path::new(&transaction.source_scan_root),
            RecoveryMigrationDisposition::DestinationOwned,
            old_root,
            new_root,
        ));
        for path in &transaction.created_directories {
            references.push(review_reference(
                &transaction.transaction_id,
                None,
                RecoveryMigrationRole::CreatedDirectory,
                path,
                RecoveryMigrationDisposition::DestinationOwned,
                old_root,
                new_root,
            ));
        }
        if let Some(envelope) = exact {
            let envelope_root = Path::new(&envelope.source_scan_root);
            exact_resume.push(exact_resume_reference(
                &transaction.transaction_id,
                usize::MAX,
                RecoveryMigrationRole::TransactionScanRoot,
                envelope_root,
                old_root,
                new_root,
            ));
            for path in &envelope.approved_source_paths {
                exact_resume.push(exact_resume_reference(
                    &transaction.transaction_id,
                    usize::MAX,
                    RecoveryMigrationRole::ExactResumeSource,
                    Path::new(path),
                    old_root,
                    new_root,
                ));
            }
            for operation in envelope.operations {
                let source = exact_resume_reference(
                    &transaction.transaction_id,
                    operation.index,
                    RecoveryMigrationRole::ExactResumeSource,
                    &operation.source_path,
                    old_root,
                    new_root,
                );
                let destination = exact_resume_reference(
                    &transaction.transaction_id,
                    operation.index,
                    RecoveryMigrationRole::ExactResumeDestination,
                    &operation.destination_path,
                    old_root,
                    new_root,
                );
                exact_resume.push(source);
                exact_resume.push(destination);
            }
        }
    }

    let source_plan = plan_source_root_migration(
        old_root,
        new_root,
        &[SubsystemSnapshot {
            subsystem: SUBSYSTEM.to_string(),
            references: source_references,
        }],
    );
    for reference in &mut references {
        if reference.disposition == RecoveryMigrationDisposition::Planner {
            if let Some(proposal) = source_plan.migration.proposals.iter().find(|proposal| {
                proposal.reference_id.ends_with(&format!(
                    ":entry:{}:source",
                    reference.entry_index.unwrap_or_default()
                )) && proposal.reference_id.starts_with(&reference.transaction_id)
            }) {
                reference.proposal = proposal.clone();
            }
        }
    }
    for reference in &references {
        if reference.disposition == RecoveryMigrationDisposition::Planner {
            match reference.proposal.classification {
                MigrationClassification::SafeRebase => totals.source_candidates += 1,
                MigrationClassification::AlreadyCurrent => totals.source_already_current += 1,
                MigrationClassification::Ambiguous
                | MigrationClassification::TargetMissing
                | MigrationClassification::OutsideNewRoot
                | MigrationClassification::ManualReview => totals.source_manual_review += 1,
                _ => {}
            }
        }
    }
    totals.conflicts = source_plan.totals.conflicts;
    let mut all_references = references;
    all_references.sort_by(|a, b| {
        a.transaction_id
            .cmp(&b.transaction_id)
            .then(a.entry_index.cmp(&b.entry_index))
            .then((a.role as u8).cmp(&(b.role as u8)))
    });
    RecoveryMigrationReport {
        source_root_plan: source_plan,
        references: all_references,
        exact_resume,
        totals,
        journal_problems: Vec::new(),
    }
}

fn placeholder_proposal(reference: &MigrationReference) -> MigrationProposal {
    MigrationProposal {
        reference_id: reference.id.clone(),
        subsystem: reference.subsystem.clone(),
        old_path: reference.path.clone(),
        candidate_path: None,
        classification: MigrationClassification::ManualReview,
        reason: "Awaiting the aggregate exact source-root planner result.".to_string(),
    }
}

fn review_reference(
    transaction_id: &str,
    entry_index: Option<usize>,
    role: RecoveryMigrationRole,
    path: &Path,
    disposition: RecoveryMigrationDisposition,
    old_root: &Path,
    new_root: &Path,
) -> RecoveryMigrationReference {
    let generic = MigrationReference {
        id: format!("{transaction_id}:review:{role:?}"),
        subsystem: SUBSYSTEM.to_string(),
        path: path.to_path_buf(),
        semantics: PathSemantics::DestinationRootAbsolute,
        migratable: false,
        requires_existence: false,
        historical: false,
        live: true,
        claimed_destination: None,
    };
    let mut proposal = plan_rebase(old_root, new_root, &generic);
    proposal.classification = MigrationClassification::ManualReview;
    proposal.reason = match disposition {
        RecoveryMigrationDisposition::RollbackPathChangedReviewRequired => "Applied-entry rollback paths are approval-bound; a root move requires explicit rollback-path review.".to_string(),
        RecoveryMigrationDisposition::External => "The transaction reference is outside both source-root candidates and remains external.".to_string(),
        _ => "Transaction destination semantics are not safe to rebase automatically.".to_string(),
    };
    RecoveryMigrationReference {
        transaction_id: transaction_id.to_string(),
        entry_index,
        role,
        path: path.to_path_buf(),
        disposition,
        proposal,
    }
}

fn historical_reference(
    transaction_id: &str,
    entry_index: Option<usize>,
    role: RecoveryMigrationRole,
    path: &Path,
    old_root: &Path,
    new_root: &Path,
) -> RecoveryMigrationReference {
    let generic = MigrationReference {
        id: format!("{transaction_id}:historical:{entry_index:?}:{role:?}"),
        subsystem: SUBSYSTEM.to_string(),
        path: path.to_path_buf(),
        semantics: PathSemantics::TransactionHistorical,
        migratable: false,
        requires_existence: false,
        historical: true,
        live: false,
        claimed_destination: None,
    };
    RecoveryMigrationReference {
        transaction_id: transaction_id.to_string(),
        entry_index,
        role,
        path: path.to_path_buf(),
        disposition: RecoveryMigrationDisposition::HistoricalOnly,
        proposal: plan_rebase(old_root, new_root, &generic),
    }
}

fn exact_resume_reference(
    transaction_id: &str,
    entry_index: usize,
    role: RecoveryMigrationRole,
    path: &Path,
    old_root: &Path,
    new_root: &Path,
) -> MigrationProposal {
    let generic = MigrationReference {
        id: format!("{transaction_id}:exact:{entry_index}:{role:?}"),
        subsystem: SUBSYSTEM.to_string(),
        path: path.to_path_buf(),
        semantics: PathSemantics::TransactionHistorical,
        migratable: false,
        requires_existence: false,
        historical: true,
        live: false,
        claimed_destination: None,
    };
    let mut proposal = plan_rebase(old_root, new_root, &generic);
    proposal.reason = "Exact-resume approval remains bound to its original path; migration never rewrites or revalidates the envelope.".to_string();
    proposal
}

fn read_exact_envelope(transaction: &RenameTransaction) -> Option<ExactResumeEnvelope> {
    transaction
        .unknown
        .get(EXACT_RESUME_ENVELOPE_KEY)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn is_historical(transaction: &RenameTransaction) -> bool {
    if transaction.recovery_resolution.is_some() {
        return true;
    }
    if transaction.state == TransactionState::RolledBack {
        return true;
    }
    transaction.state == TransactionState::Applied
        && transaction
            .entries
            .iter()
            .all(|entry| matches!(entry.state, EntryState::Skipped | EntryState::RolledBack))
}

fn technically_invalid(transaction: &RenameTransaction) -> bool {
    if transaction.state == TransactionState::RolledBack
        && transaction
            .entries
            .iter()
            .any(|entry| !matches!(entry.state, EntryState::RolledBack | EntryState::Skipped))
    {
        return true;
    }
    transaction.state == TransactionState::Applied
        && transaction
            .entries
            .iter()
            .any(|entry| !matches!(entry.state, EntryState::Applied | EntryState::Skipped))
}

fn add_transaction_review_references(
    transaction: &RenameTransaction,
    old_root: &Path,
    new_root: &Path,
    disposition: RecoveryMigrationDisposition,
    references: &mut Vec<RecoveryMigrationReference>,
) {
    for (index, entry) in transaction.entries.iter().enumerate() {
        references.push(review_reference(
            &transaction.transaction_id,
            Some(index),
            RecoveryMigrationRole::LiveSource,
            &entry.source_path,
            disposition,
            old_root,
            new_root,
        ));
        references.push(review_reference(
            &transaction.transaction_id,
            Some(index),
            RecoveryMigrationRole::LiveDestination,
            &entry.destination_path,
            disposition,
            old_root,
            new_root,
        ));
    }
}

#[cfg(test)]
mod tests;
