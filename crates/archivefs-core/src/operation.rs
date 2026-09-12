//! Provider-neutral operation receipts and recovery capabilities.
//!
//! This is an additive projection layer. Existing workflow journals remain
//! authoritative; adapters translate them into this stable, read-only shape
//! for history and recovery surfaces. No executor is called from this module.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::dat::rename_apply::{
    EntryState, RenameTransaction, TransactionOperation, TransactionState, journal_path,
    list_journals,
};

pub const OPERATION_RECEIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    DatRename,
    PlayingLibrary,
}

impl OperationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::DatRename => "DAT rename",
            Self::PlayingLibrary => "Playing Library",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Planned,
    Ready,
    Running,
    Completed,
    Partial,
    Interrupted,
    Failed,
    RolledBack,
    RollbackBlocked,
    Stale,
}

impl OperationState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Planned => "Planned",
            Self::Ready => "Ready",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Partial => "Partial",
            Self::Interrupted => "Interrupted",
            Self::Failed => "Failed",
            Self::RolledBack => "Rolled back",
            Self::RollbackBlocked => "Rollback blocked",
            Self::Stale => "Stale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryClassification {
    SafeToResume,
    SafeToRollback,
    RequiresReview,
    Stale,
    Unrecoverable,
}

impl RecoveryClassification {
    pub fn label(self) -> &'static str {
        match self {
            Self::SafeToResume => "Resume available",
            Self::SafeToRollback => "Rollback available",
            Self::RequiresReview => "Review required",
            Self::Stale => "Stale",
            Self::Unrecoverable => "No recovery action",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActionAvailability {
    #[default]
    Unavailable,
    Available,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OperationActionAvailability {
    pub resume: ActionAvailability,
    pub rollback: ActionAvailability,
    pub review: ActionAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OperationInputSnapshot {
    pub source_root: Option<String>,
    pub plan_generation: Option<u64>,
    pub plan_hash: Option<String>,
    pub freshness_evidence: Vec<String>,
    pub destination_occupancy_checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationOutput {
    pub path: String,
    pub changed: bool,
    pub verified: bool,
    pub before_state: Option<String>,
    pub after_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OperationOutputReceipt {
    pub outputs: Vec<OperationOutput>,
    pub journal_reference: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecoveryStatus {
    pub classification: RecoveryClassification,
    pub explanation: String,
    pub actions: OperationActionAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub schema_version: u32,
    pub operation_id: String,
    pub kind: OperationKind,
    pub state: OperationState,
    pub created_at_unix: u64,
    pub started_at_unix: Option<u64>,
    pub completed_at_unix: Option<u64>,
    pub input: OperationInputSnapshot,
    pub destination: Option<String>,
    pub output: OperationOutputReceipt,
    pub recovery: OperationRecoveryStatus,
    pub error: Option<String>,
}

impl OperationRecord {
    pub fn is_actionable(&self) -> bool {
        self.recovery.actions.resume == ActionAvailability::Available
            || self.recovery.actions.rollback == ActionAvailability::Available
            || self.recovery.actions.review == ActionAvailability::Available
    }
}

/// A read-only index over existing rename journals. It intentionally stores
/// projections rather than a second mutable source of truth.
#[derive(Debug, Default)]
pub struct OperationRegistry {
    pub records: Vec<OperationRecord>,
    pub problems: Vec<String>,
}

impl OperationRegistry {
    pub fn from_rename_journals(dir: &Path) -> Self {
        let (transactions, problems) = list_journals(dir);
        let records = transactions
            .iter()
            .map(|transaction| {
                let journal_reference = journal_path(dir, &transaction.transaction_id)
                    .map(|path| path.to_string_lossy().into_owned());
                if transaction.entries.iter().any(|entry| {
                    matches!(entry.operation, TransactionOperation::CreateSymlink { .. })
                }) {
                    playing_library_operation(transaction, journal_reference)
                } else {
                    dat_rename_operation(transaction, journal_reference)
                }
            })
            .collect();
        Self { records, problems }
    }

    pub fn active(&self) -> impl Iterator<Item = &OperationRecord> {
        self.records.iter().filter(|record| {
            !matches!(
                record.state,
                OperationState::Completed | OperationState::RolledBack
            )
        })
    }
}

pub fn dat_rename_operation(
    transaction: &RenameTransaction,
    journal_reference: Option<String>,
) -> OperationRecord {
    operation_from_transaction(transaction, OperationKind::DatRename, journal_reference)
}

pub fn playing_library_operation(
    transaction: &RenameTransaction,
    journal_reference: Option<String>,
) -> OperationRecord {
    operation_from_transaction(
        transaction,
        OperationKind::PlayingLibrary,
        journal_reference,
    )
}

fn operation_from_transaction(
    transaction: &RenameTransaction,
    kind: OperationKind,
    journal_reference: Option<String>,
) -> OperationRecord {
    let state = map_state(transaction.state, transaction);
    let recovery = recovery_status(transaction);
    let outputs = transaction
        .entries
        .iter()
        .map(|entry| OperationOutput {
            path: entry.destination_path.to_string_lossy().into_owned(),
            changed: matches!(
                entry.state,
                EntryState::Applied | EntryState::RolledBack | EntryState::RollbackFailed
            ),
            verified: matches!(entry.state, EntryState::Applied | EntryState::RolledBack),
            before_state: Some(entry.identity.kind.label().to_string()),
            after_state: Some(entry.state.label().to_string()),
        })
        .collect();

    OperationRecord {
        schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
        operation_id: transaction.transaction_id.clone(),
        kind,
        state,
        created_at_unix: transaction.created_at_unix,
        started_at_unix: None,
        completed_at_unix: if matches!(
            state,
            OperationState::Completed
                | OperationState::Partial
                | OperationState::Failed
                | OperationState::RolledBack
                | OperationState::RollbackBlocked
        ) {
            transaction
                .entries
                .iter()
                .filter_map(|entry| entry.applied_at_unix)
                .max()
        } else {
            None
        },
        input: OperationInputSnapshot {
            source_root: Some(transaction.source_scan_root.clone()),
            plan_generation: Some(transaction.plan_generation),
            plan_hash: None,
            freshness_evidence: transaction
                .entries
                .iter()
                .filter_map(|entry| entry.identity.freshness.as_ref())
                .map(|freshness| {
                    format!(
                        "sha256:{}",
                        freshness
                            .sha256
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()
                    )
                })
                .collect(),
            destination_occupancy_checked: transaction
                .entries
                .iter()
                .all(|entry| entry.preflight_passed),
        },
        destination: transaction
            .entries
            .first()
            .and_then(|entry| entry.destination_path.parent())
            .map(|path| path.to_string_lossy().into_owned()),
        output: OperationOutputReceipt {
            outputs,
            journal_reference,
            summary: summary(transaction),
        },
        recovery,
        error: transaction
            .entries
            .iter()
            .find_map(|entry| entry.failure_reason.clone()),
    }
}

fn map_state(state: TransactionState, transaction: &RenameTransaction) -> OperationState {
    match state {
        TransactionState::Planned => OperationState::Planned,
        TransactionState::Applying | TransactionState::RollingBack => OperationState::Running,
        TransactionState::Applied => OperationState::Completed,
        TransactionState::ApplyFailed if transaction.applied_count() > 0 => OperationState::Partial,
        TransactionState::ApplyFailed => OperationState::Failed,
        TransactionState::RolledBack => OperationState::RolledBack,
        TransactionState::RollbackFailed => OperationState::RollbackBlocked,
    }
}

fn recovery_status(transaction: &RenameTransaction) -> OperationRecoveryStatus {
    let rollback = transaction.has_applied_entries();
    let interrupted = transaction.state.needs_recovery();
    let classification = if transaction.state == TransactionState::RolledBack {
        RecoveryClassification::Unrecoverable
    } else if transaction.state == TransactionState::RollbackFailed {
        RecoveryClassification::RequiresReview
    } else if rollback {
        RecoveryClassification::SafeToRollback
    } else if interrupted {
        RecoveryClassification::RequiresReview
    } else {
        RecoveryClassification::Unrecoverable
    };
    let explanation = match classification {
        RecoveryClassification::SafeToRollback => {
            "The journal records applied steps that may be rolled back after fresh checks.".into()
        }
        RecoveryClassification::RequiresReview => {
            "The operation is unfinished or rollback failed; review the journal before acting."
                .into()
        }
        RecoveryClassification::Unrecoverable => {
            "No generic recovery action is advertised by this adapter.".into()
        }
        RecoveryClassification::SafeToResume => {
            "An exact resume implementation is available for this operation.".into()
        }
        RecoveryClassification::Stale => "The recorded plan or input evidence is stale.".into(),
    };
    OperationRecoveryStatus {
        classification,
        explanation,
        actions: OperationActionAvailability {
            resume: ActionAvailability::Unavailable,
            rollback: if rollback {
                ActionAvailability::Available
            } else {
                ActionAvailability::Unavailable
            },
            review: if interrupted {
                ActionAvailability::Available
            } else {
                ActionAvailability::Unavailable
            },
        },
    }
}

fn summary(transaction: &RenameTransaction) -> String {
    format!(
        "{} requested, {} applied, {} skipped, {} failed",
        transaction.entries.len(),
        transaction.applied_count(),
        transaction.skipped_count(),
        transaction.failed_count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::rename_apply::{ObjectIdentity, ObjectKind, TransactionEntry};
    use std::path::PathBuf;

    fn transaction(state: TransactionState) -> RenameTransaction {
        RenameTransaction {
            transaction_id: "operation-test".into(),
            plan_generation: 7,
            classifier_version: Some("classifier-v1".into()),
            created_at_unix: 100,
            source_scan_root: "/library".into(),
            state,
            entries: vec![TransactionEntry {
                source_path: PathBuf::from("/library/a.zip"),
                destination_path: PathBuf::from("/output/a.zip"),
                original_basename: "a.zip".into(),
                proposed_basename: "a.zip".into(),
                identity: ObjectIdentity {
                    size_bytes: 4,
                    modified_unix: 100,
                    kind: ObjectKind::RegularFile,
                    #[cfg(unix)]
                    ino: 1,
                    #[cfg(unix)]
                    dev: 1,
                    freshness: None,
                },
                operation: Default::default(),
                preflight_passed: true,
                preflight_failures: Vec::new(),
                state: if state == TransactionState::Applied {
                    EntryState::Applied
                } else {
                    EntryState::Planned
                },
                failure_reason: None,
                applied_at_unix: Some(101),
                rolled_back_at_unix: None,
                unknown: Default::default(),
            }],
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    #[test]
    fn completed_dat_receipt_contains_plan_and_output_evidence() {
        let record = dat_rename_operation(&transaction(TransactionState::Applied), None);
        assert_eq!(record.kind, OperationKind::DatRename);
        assert_eq!(record.state, OperationState::Completed);
        assert_eq!(record.input.plan_generation, Some(7));
        assert_eq!(record.output.outputs.len(), 1);
        assert!(record.output.summary.contains("1 applied"));
    }

    #[test]
    fn interrupted_operation_is_reviewable_and_rollback_capability_is_explicit() {
        let mut transaction = transaction(TransactionState::Applying);
        transaction.entries[0].state = EntryState::Applied;
        let record = dat_rename_operation(&transaction, None);
        assert_eq!(record.state, OperationState::Running);
        assert_eq!(
            record.recovery.classification,
            RecoveryClassification::SafeToRollback
        );
        assert_eq!(
            record.recovery.actions.resume,
            ActionAvailability::Unavailable
        );
        assert_eq!(
            record.recovery.actions.rollback,
            ActionAvailability::Available
        );
    }

    #[test]
    fn playing_library_is_a_distinct_adapter_kind() {
        let record = playing_library_operation(&transaction(TransactionState::Planned), None);
        assert_eq!(record.kind, OperationKind::PlayingLibrary);
        assert_eq!(record.state, OperationState::Planned);
    }

    #[test]
    fn receipt_round_trips_and_old_transaction_fields_are_not_required() {
        let record =
            dat_rename_operation(&transaction(TransactionState::Applied), Some("x".into()));
        let json = serde_json::to_string(&record).unwrap();
        let reloaded: OperationRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, reloaded);
    }

    #[test]
    fn completed_operation_does_not_advertise_generic_resume() {
        let record = dat_rename_operation(&transaction(TransactionState::RolledBack), None);
        assert!(!record.is_actionable());
        assert_eq!(
            record.recovery.actions.resume,
            ActionAvailability::Unavailable
        );
    }

    #[test]
    fn planned_operation_requires_review_without_claiming_rollback_safety() {
        let record = dat_rename_operation(&transaction(TransactionState::Planned), None);
        assert_eq!(
            record.recovery.classification,
            RecoveryClassification::RequiresReview
        );
        assert_eq!(
            record.recovery.actions.rollback,
            ActionAvailability::Unavailable
        );
        assert_eq!(
            record.recovery.actions.review,
            ActionAvailability::Available
        );
    }

    #[test]
    fn failed_operation_with_applied_steps_is_partial_and_rollbackable() {
        let mut transaction = transaction(TransactionState::ApplyFailed);
        transaction.entries[0].state = EntryState::Applied;
        let record = dat_rename_operation(&transaction, None);
        assert_eq!(record.state, OperationState::Partial);
        assert_eq!(
            record.recovery.classification,
            RecoveryClassification::SafeToRollback
        );
        assert_eq!(
            record.recovery.actions.rollback,
            ActionAvailability::Available
        );
    }

    #[test]
    fn rollback_failure_never_advertises_resume() {
        let mut transaction = transaction(TransactionState::RollbackFailed);
        transaction.entries[0].state = EntryState::ApplyFailed;
        let record = dat_rename_operation(&transaction, None);
        assert_eq!(record.state, OperationState::RollbackBlocked);
        assert_eq!(
            record.recovery.classification,
            RecoveryClassification::RequiresReview
        );
        assert_eq!(
            record.recovery.actions.resume,
            ActionAvailability::Unavailable
        );
    }

    #[test]
    fn registry_projects_existing_journals_and_preserves_parse_problems() {
        let temp = tempfile::tempdir().unwrap();
        let valid = transaction(TransactionState::Applied);
        crate::dat::rename_apply::write_journal(temp.path(), &valid).unwrap();
        std::fs::write(temp.path().join("broken.json"), "not json").unwrap();
        let registry = OperationRegistry::from_rename_journals(temp.path());
        assert_eq!(registry.records.len(), 1);
        assert_eq!(registry.problems.len(), 1);
        assert_eq!(registry.records[0].operation_id, "operation-test");
        assert!(registry.records[0].output.journal_reference.is_some());
    }

    #[test]
    fn old_transaction_shape_projects_without_new_receipt_fields() {
        let old = r#"{
            "transaction_id":"old",
            "plan_generation":1,
            "created_at_unix":2,
            "source_scan_root":"/old",
            "state":"applied",
            "entries":[]
        }"#;
        let transaction: RenameTransaction = serde_json::from_str(old).unwrap();
        let record = dat_rename_operation(&transaction, None);
        assert_eq!(record.operation_id, "old");
        assert_eq!(record.state, OperationState::Completed);
        assert_eq!(record.schema_version, OPERATION_RECEIPT_SCHEMA_VERSION);
    }
}
