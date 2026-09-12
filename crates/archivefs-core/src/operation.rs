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
use crate::patch_manager::{
    PreviewAdapter, SharedApplyJournal, SharedApplyOutcome, SharedApplyStatus,
    SharedRollbackOutcome, SharedRollbackPreview, discover_shared_apply_history,
    preview_shared_rollback,
};
use crate::repair::quarantine::QUARANTINE_DIRECTORY_NAME;

pub const OPERATION_RECEIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    DatRename,
    PlayingLibrary,
    DuplicateQuarantine,
    RepairApply,
    CheatApply,
    ModApply,
}

impl OperationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::DatRename => "DAT rename",
            Self::PlayingLibrary => "Playing Library",
            Self::DuplicateQuarantine => "Duplicate quarantine",
            Self::RepairApply => "Repair apply",
            Self::CheatApply => "Cheat apply",
            Self::ModApply => "Mod apply",
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
                } else if transaction.entries.iter().any(|entry| {
                    entry
                        .destination_path
                        .components()
                        .any(|component| component.as_os_str() == QUARANTINE_DIRECTORY_NAME)
                }) {
                    duplicate_quarantine_operation(transaction, journal_reference)
                } else {
                    dat_rename_operation(transaction, journal_reference)
                }
            })
            .collect();
        Self { records, problems }
    }

    /// Adds shared cheat/mod journals to this index. The existing shared
    /// history reader remains authoritative; this only supplies a common
    /// read-only projection and rollback capability check.
    pub fn append_shared_apply_history(&mut self, history_root: &Path, backup_root: &Path) {
        let report = discover_shared_apply_history(history_root);
        self.problems.extend(
            report
                .warnings
                .iter()
                .map(|warning| format!("{}: {}", warning.path.display, warning.failure.detail)),
        );
        self.records
            .extend(report.journals.iter().map(|(path, journal)| {
                let rollback = path.to_path_buf().ok().and_then(|journal_path| {
                    journal
                        .destination_root
                        .to_path_buf()
                        .ok()
                        .map(|destination_root| {
                            preview_shared_rollback(&journal_path, &destination_root, backup_root)
                        })
                });
                shared_apply_operation(journal, Some(path.display.clone()), rollback.as_ref())
            }));
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

pub fn duplicate_quarantine_operation(
    transaction: &RenameTransaction,
    journal_reference: Option<String>,
) -> OperationRecord {
    operation_from_transaction(
        transaction,
        OperationKind::DuplicateQuarantine,
        journal_reference,
    )
}

pub fn repair_operation(
    transaction: &RenameTransaction,
    journal_reference: Option<String>,
) -> OperationRecord {
    operation_from_transaction(transaction, OperationKind::RepairApply, journal_reference)
}

fn shared_apply_operation(
    journal: &SharedApplyJournal,
    journal_reference: Option<String>,
    rollback: Option<&SharedRollbackPreview>,
) -> OperationRecord {
    let kind = match journal.context.adapter {
        PreviewAdapter::LocalModPackage => OperationKind::ModApply,
        PreviewAdapter::RetroArch
        | PreviewAdapter::Pcsx2
        | PreviewAdapter::Dolphin
        | PreviewAdapter::Xenia => OperationKind::CheatApply,
    };
    let state = match journal.status {
        SharedApplyStatus::DryRun => OperationState::Planned,
        SharedApplyStatus::Success => OperationState::Completed,
        SharedApplyStatus::PartialFailure => OperationState::Partial,
        SharedApplyStatus::Failed => OperationState::Failed,
    };
    let written = journal.entries.iter().filter(|entry| {
        matches!(
            entry.outcome,
            SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
        )
    });
    let outputs = written
        .map(|entry| OperationOutput {
            path: format!(
                "{}/{}",
                entry.plan_entry.destination_root.display,
                entry.plan_entry.destination_relative_path.display
            ),
            changed: true,
            verified: entry.verification_succeeded,
            before_state: Some(format!("{:?}", entry.plan_entry.destination_pre_state)),
            after_state: Some(format!("{:?}", entry.outcome)),
        })
        .collect::<Vec<_>>();
    let recovery = shared_recovery(journal, rollback);
    OperationRecord {
        schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
        operation_id: journal.operation_id.clone(),
        kind,
        state,
        created_at_unix: journal.timestamp_unix_seconds,
        started_at_unix: Some(journal.timestamp_unix_seconds),
        completed_at_unix: if matches!(
            state,
            OperationState::Completed | OperationState::Partial | OperationState::Failed
        ) {
            Some(journal.timestamp_unix_seconds)
        } else {
            None
        },
        input: OperationInputSnapshot {
            source_root: Some(journal.approved_source_root.display.clone()),
            plan_generation: None,
            plan_hash: Some(journal.plan_id.clone()),
            freshness_evidence: journal
                .entries
                .iter()
                .map(|entry| {
                    format!(
                        "source_digest:{}",
                        entry
                            .observed_source_digest
                            .as_deref()
                            .unwrap_or(&entry.plan_entry.source_digest)
                    )
                })
                .collect(),
            destination_occupancy_checked: journal.entries.iter().all(|entry| {
                entry.observed_destination_digest.is_some()
                    || entry.destination_existed_before_apply.is_some()
            }),
        },
        destination: Some(journal.destination_root.display.clone()),
        output: OperationOutputReceipt {
            outputs,
            journal_reference,
            summary: shared_summary(journal),
        },
        recovery,
        error: journal
            .entries
            .iter()
            .flat_map(|entry| entry.failures.iter())
            .next()
            .map(|failure| failure.detail.clone()),
    }
}

fn shared_recovery(
    journal: &SharedApplyJournal,
    rollback: Option<&SharedRollbackPreview>,
) -> OperationRecoveryStatus {
    let writes = journal.entries.iter().any(|entry| {
        matches!(
            entry.outcome,
            SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
        )
    });
    let stale = rollback.is_some_and(|preview| {
        preview.entries.iter().any(|entry| {
            matches!(
                entry.outcome,
                SharedRollbackOutcome::DestinationChanged
                    | SharedRollbackOutcome::DestinationUnsafe
                    | SharedRollbackOutcome::BackupChanged
                    | SharedRollbackOutcome::RootMismatch
            )
        })
    });
    let classification = if stale {
        RecoveryClassification::Stale
    } else if rollback.is_some_and(|preview| preview.available) {
        RecoveryClassification::SafeToRollback
    } else if writes {
        RecoveryClassification::RequiresReview
    } else {
        RecoveryClassification::Unrecoverable
    };
    let explanation = match classification {
        RecoveryClassification::SafeToRollback => {
            "Rollback is available after the existing safety checks.".into()
        }
        RecoveryClassification::Stale => {
            "The destination or backup no longer matches the recorded operation.".into()
        }
        RecoveryClassification::RequiresReview => {
            "The operation changed files but no safe automatic recovery action is available.".into()
        }
        RecoveryClassification::Unrecoverable => {
            "No recovery action is advertised by this adapter.".into()
        }
        RecoveryClassification::SafeToResume => {
            "An exact resume implementation is available for this operation.".into()
        }
    };
    OperationRecoveryStatus {
        classification,
        explanation,
        actions: OperationActionAvailability {
            resume: ActionAvailability::Unavailable,
            rollback: if rollback.is_some_and(|preview| preview.available) {
                ActionAvailability::Available
            } else {
                ActionAvailability::Unavailable
            },
            review: if matches!(
                classification,
                RecoveryClassification::RequiresReview | RecoveryClassification::Stale
            ) {
                ActionAvailability::Available
            } else {
                ActionAvailability::Unavailable
            },
        },
    }
}

fn shared_summary(journal: &SharedApplyJournal) -> String {
    let written = journal
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.outcome,
                SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
            )
        })
        .count();
    let skipped = journal.entries.len().saturating_sub(written);
    format!("Applied {written} file(s); {skipped} skipped or refused")
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
    use crate::patch_manager::{
        PreviewDestinationState, PreviewProposedAction, SharedApplyContext, SharedApplyEntry,
        SharedApplyJournal, SharedApplyStatus, SharedPlanEntry, SharedRollbackPreview,
        SharedTransactionPath,
    };
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

    fn shared_journal(adapter: PreviewAdapter, status: SharedApplyStatus) -> SharedApplyJournal {
        let source = SharedTransactionPath::from_path(Path::new("/source/cheat.txt"));
        let destination_root = SharedTransactionPath::from_path(Path::new("/emulator"));
        SharedApplyJournal {
            schema_version: 1,
            operation_id: "shared-operation".into(),
            plan_id: "plan-7".into(),
            timestamp_unix_seconds: 100,
            context: SharedApplyContext {
                adapter,
                selected_archive: source.clone(),
                verified_game_identity: "game-identity".into(),
                profile_id: "profile".into(),
                source_mode: "local".into(),
            },
            approved_source_root: SharedTransactionPath::from_path(Path::new("/source")),
            destination_root: destination_root.clone(),
            created_root_directories: Vec::new(),
            dry_run: false,
            entries: vec![SharedApplyEntry {
                plan_entry: SharedPlanEntry {
                    adapter,
                    selected_archive: source.clone(),
                    verified_game_identity: "game-identity".into(),
                    source_path: source,
                    source_digest: "source-digest".into(),
                    destination_root,
                    destination_relative_path: SharedTransactionPath::from_path(Path::new(
                        "game.txt",
                    )),
                    destination_pre_state: PreviewDestinationState::Missing,
                    destination_pre_digest: None,
                    proposed_action: PreviewProposedAction::Install,
                    backup_required: true,
                    parent_creation_approved: true,
                    content_verification: None,
                },
                destination_existed_before_apply: Some(false),
                destination_parent_existed_before_apply: Some(true),
                observed_source_digest: Some("source-digest".into()),
                observed_destination_digest: None,
                backup_path: Some(SharedTransactionPath::from_path(Path::new("/backup/game"))),
                backup_digest: Some("backup-digest".into()),
                temporary_path: None,
                final_destination_digest: Some("installed-digest".into()),
                created_directories: Vec::new(),
                replacement_approved: false,
                verification_succeeded: true,
                outcome: SharedApplyOutcome::InstalledNew,
                stages: Vec::new(),
                warnings: Vec::new(),
                failures: Vec::new(),
            }],
            status,
            rollback_operation_id: None,
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

    #[test]
    fn shared_cheat_apply_preserves_identity_backup_and_rollback_capability() {
        let journal = shared_journal(PreviewAdapter::RetroArch, SharedApplyStatus::Success);
        let preview = SharedRollbackPreview {
            schema_version: 1,
            preview_id: "rollback-preview".into(),
            journal_path: SharedTransactionPath::from_path(Path::new(
                "/history/shared-operation.json",
            )),
            original_operation_id: journal.operation_id.clone(),
            destination_root: journal.destination_root.clone(),
            entries: Vec::new(),
            available: true,
        };
        let record = shared_apply_operation(&journal, None, Some(&preview));
        assert_eq!(record.kind, OperationKind::CheatApply);
        assert_eq!(record.state, OperationState::Completed);
        assert_eq!(record.input.plan_hash.as_deref(), Some("plan-7"));
        assert_eq!(record.output.outputs.len(), 1);
        assert_eq!(
            record.recovery.actions.rollback,
            ActionAvailability::Available
        );
    }

    #[test]
    fn shared_local_mod_apply_is_distinct_and_partial_failures_need_review() {
        let journal = shared_journal(
            PreviewAdapter::LocalModPackage,
            SharedApplyStatus::PartialFailure,
        );
        let record = shared_apply_operation(&journal, Some("mod.json".into()), None);
        assert_eq!(record.kind, OperationKind::ModApply);
        assert_eq!(record.state, OperationState::Partial);
        assert_eq!(
            record.recovery.classification,
            RecoveryClassification::RequiresReview
        );
        assert_eq!(
            record.recovery.actions.resume,
            ActionAvailability::Unavailable
        );
        assert_eq!(record.output.journal_reference.as_deref(), Some("mod.json"));
    }
}
