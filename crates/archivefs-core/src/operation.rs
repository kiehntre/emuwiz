//! Provider-neutral operation receipts and recovery capabilities.
//!
//! This is an additive projection layer. Existing workflow journals remain
//! authoritative; adapters translate them into this stable, read-only shape
//! for history and recovery surfaces. No executor is called from this module.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dat::rename_apply::{
    EntryState, RenameTransaction, TransactionEntry, TransactionOperation, TransactionState,
    journal_path, list_journals,
};
use crate::launch::es_de_publish::EsDeGamelistPublication;
use crate::patch_manager::{
    PreviewAdapter, SharedApplyJournal, SharedApplyOutcome, SharedApplyStatus,
    SharedRollbackOutcome, SharedRollbackPreview, discover_shared_apply_history,
    preview_shared_rollback,
};
use crate::repair::optical_conversion::{
    DISC_CONVERSION_QUARANTINE_SUBDIR, DISC_CONVERSION_STAGING_PREFIX,
};
use crate::repair::quarantine::QUARANTINE_DIRECTORY_NAME;
use crate::{LibraryViewHistoryOperation, LibraryViewHistoryRecord};

pub const OPERATION_RECEIPT_SCHEMA_VERSION: u32 = 1;

mod attention;
pub use attention::{AttentionReceiptPaths, attention_receipt_snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    DatRename,
    PlayingLibrary,
    DuplicateQuarantine,
    RepairApply,
    CheatApply,
    ModApply,
    LibraryViewPublish,
    RommPublish,
    EsDePublish,
    DatabaseRecovery,
    DatabaseBackup,
    DiscConversion,
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
            Self::LibraryViewPublish => "Library View publication",
            Self::RommPublish => "RomM publication",
            Self::EsDePublish => "ES-DE publication",
            Self::DatabaseRecovery => "Database recovery",
            Self::DatabaseBackup => "Database backup",
            Self::DiscConversion => "Disc conversion",
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
                } else if let Some(role) = disc_conversion_role(transaction) {
                    disc_conversion_operation(transaction, role, journal_reference)
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

    /// Adds durable Library View history records. Malformed legacy records
    /// remain visible as registry problems; no recovery capability is
    /// invented for history that does not contain it.
    pub fn append_library_view_history(&mut self, directory: &Path) {
        for entry in crate::list_library_view_history_at(directory, 200) {
            match entry {
                crate::LibraryViewHistoryEntry::Record { path, record } => self.records.push(
                    library_view_history_operation(&record, Some(path.display().to_string())),
                ),
                crate::LibraryViewHistoryEntry::Malformed { path, error } => {
                    self.problems.push(format!("{}: {error}", path.display()))
                }
            }
        }
    }

    /// Enumerates retained database backup files as historical operations.
    /// Their presence is useful history, but their original hash and
    /// pre-operation state are not persisted by the legacy naming format, so
    /// rollback is never advertised.
    pub fn append_database_backup_history(&mut self, database_path: &Path) {
        let Some(parent) = database_path.parent() else {
            return;
        };
        let Some(database_name) = database_path.file_name() else {
            return;
        };
        let prefix = format!("{}.schema-", database_name.to_string_lossy());
        let Ok(entries) = std::fs::read_dir(parent) else {
            return;
        };
        for entry in entries.flatten().take(200) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.starts_with(&prefix) || !name.contains(".before-") || !name.contains(".backup")
            {
                continue;
            }
            let hash = sha256_path(&path).unwrap_or_else(|| "unavailable".into());
            let record = OperationRecord {
                schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
                operation_id: format!("database-backup:{hash}"),
                kind: OperationKind::DatabaseBackup,
                state: OperationState::Completed,
                created_at_unix: 0,
                started_at_unix: None,
                completed_at_unix: None,
                input: OperationInputSnapshot {
                    source_root: Some(database_path.to_string_lossy().into_owned()),
                    plan_generation: None,
                    plan_hash: None,
                    freshness_evidence: vec!["legacy_backup_filename".into()],
                    destination_occupancy_checked: false,
                },
                destination: Some(path.to_string_lossy().into_owned()),
                output: OperationOutputReceipt {
                    outputs: vec![OperationOutput {
                        path: path.to_string_lossy().into_owned(),
                        changed: true,
                        verified: hash != "unavailable",
                        before_state: None,
                        after_state: Some(format!("sha256:{hash}")),
                    }],
                    journal_reference: None,
                    summary: "Historical database backup retained; recovery evidence is incomplete".into(),
                },
                recovery: OperationRecoveryStatus {
                    classification: RecoveryClassification::Unrecoverable,
                    explanation: "Historical database operation; recovery unavailable because the legacy record lacks the original hash and before-state evidence.".into(),
                    actions: OperationActionAvailability::default(),
                },
                error: None,
            };
            self.records.push(record);
        }
    }

    /// Reads durable database-restore receipts. Unlike legacy backup names,
    /// these records contain the selected hash, live-database freshness, and
    /// the mandatory emergency-backup evidence needed for reviewed recovery.
    pub fn append_database_restore_history(&mut self, database_path: &Path) {
        let Some(parent) = database_path.parent() else {
            return;
        };
        let Some(database_name) = database_path.file_name().and_then(|v| v.to_str()) else {
            return;
        };
        let prefix = format!("{database_name}.restore-");
        let Ok(entries) = fs::read_dir(parent) else {
            return;
        };
        for entry in entries.flatten().take(200) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
                continue;
            };
            if !name.starts_with(&prefix) || !name.ends_with(".json") {
                continue;
            }
            match fs::read(&path).ok().and_then(|bytes| {
                serde_json::from_slice::<crate::DatabaseRestoreReceipt>(&bytes).ok()
            }) {
                Some(receipt) => self.records.push(database_restore_receipt_operation(
                    &receipt,
                    Some(path.display().to_string()),
                )),
                None => self.problems.push(format!(
                    "{}: malformed database restore receipt",
                    path.display()
                )),
            }
        }
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

fn database_restore_receipt_operation(
    receipt: &crate::DatabaseRestoreReceipt,
    journal_reference: Option<String>,
) -> OperationRecord {
    let state = match receipt.state {
        crate::DatabaseRestoreState::Completed => OperationState::Completed,
        crate::DatabaseRestoreState::RolledBack => OperationState::RolledBack,
        crate::DatabaseRestoreState::Failed | crate::DatabaseRestoreState::RollbackFailed => {
            OperationState::Failed
        }
        _ => OperationState::Stale,
    };
    let recoverable =
        receipt.emergency_backup_path.is_some() && receipt.emergency_backup_sha256.is_some();
    OperationRecord {
        schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
        operation_id: receipt.operation_id.clone(),
        kind: OperationKind::DatabaseRecovery,
        state,
        created_at_unix: receipt.created_at_unix,
        started_at_unix: Some(receipt.created_at_unix),
        completed_at_unix: Some(receipt.updated_at_unix),
        input: OperationInputSnapshot {
            source_root: Some(
                receipt
                    .plan
                    .selected_backup_path
                    .to_string_lossy()
                    .into_owned(),
            ),
            plan_generation: None,
            plan_hash: Some(format!("sha256:{}", receipt.plan.selected_backup_sha256)),
            freshness_evidence: vec![
                format!("live_size:{}", receipt.plan.expected_live_size_bytes),
                format!("live_sha256:{}", receipt.plan.expected_live_sha256),
                format!("backup_schema:{}", receipt.plan.backup_schema_version),
            ],
            destination_occupancy_checked: true,
        },
        destination: Some(
            receipt
                .plan
                .live_database_path
                .to_string_lossy()
                .into_owned(),
        ),
        output: OperationOutputReceipt {
            outputs: receipt
                .emergency_backup_path
                .iter()
                .map(|path| OperationOutput {
                    path: path.to_string_lossy().into_owned(),
                    changed: true,
                    verified: recoverable,
                    before_state: Some("live database".into()),
                    after_state: Some(receipt.state.label().into()),
                })
                .collect(),
            journal_reference,
            summary: receipt.message.clone(),
        },
        recovery: OperationRecoveryStatus {
            classification: if recoverable {
                RecoveryClassification::RequiresReview
            } else {
                RecoveryClassification::Unrecoverable
            },
            explanation: if recoverable {
                "Review this database restore receipt before any further recovery action. A rollback is only allowed after fresh validation.".into()
            } else {
                "Restore evidence is incomplete; no automatic recovery action is available.".into()
            },
            actions: OperationActionAvailability {
                review: if recoverable {
                    ActionAvailability::Available
                } else {
                    ActionAvailability::Unavailable
                },
                ..Default::default()
            },
        },
        error: matches!(state, OperationState::Failed).then_some(receipt.message.clone()),
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

/// Which half of a disc-conversion attempt a saved transaction represents.
/// Disc conversion runs the shared Repair engine twice per attempt: once to
/// finalize the fingerprint-verified CHD output, and, only when the user
/// chose [`crate::repair::optical_conversion::ChdConversionSourceMode::QuarantineSource`],
/// a second time to relocate the original CUE/BIN. Both are ordinary
/// `RenameTransaction` journals; this distinguishes them from every other
/// adapter that shares the same journal directory, and from each other,
/// purely from stable path shapes already produced by
/// `crate::repair::optical_conversion` -- no new journal field, no change to
/// what that module writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscConversionRole {
    /// Finalizes the staged, fingerprint-verified CHD at its real destination.
    Output,
    /// Relocates the original CUE/BIN into quarantine after verification.
    SourceQuarantine,
}

fn disc_conversion_role(transaction: &RenameTransaction) -> Option<DiscConversionRole> {
    let is_staged_output = transaction.entries.iter().any(|entry| {
        entry.source_path.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| name.starts_with(DISC_CONVERSION_STAGING_PREFIX))
        })
    });
    if is_staged_output {
        return Some(DiscConversionRole::Output);
    }
    let is_source_quarantine = transaction.entries.iter().any(|entry| {
        entry
            .destination_path
            .components()
            .any(|component| component.as_os_str() == DISC_CONVERSION_QUARANTINE_SUBDIR)
    });
    is_source_quarantine.then_some(DiscConversionRole::SourceQuarantine)
}

/// Projects a disc-conversion transaction (either role) into a receipt with
/// disc-conversion-specific wording. State and recovery classification reuse
/// the exact same, already-proven `RenameTransaction` mapping every other
/// rename-engine adapter uses (`map_state` / `recovery_status`) -- this adds
/// presentation only, never a new safety rule.
///
/// The only additional check beyond that shared mapping is a cheap liveness
/// check on a completed [`DiscConversionRole::Output`] transaction's
/// destination: an output that has since been deleted or resized is
/// reported `Stale` rather than `Completed`, the same principle
/// `es_de_publication_operation` already applies to a live gamelist. This
/// never re-reads or re-hashes the (potentially multi-gigabyte) CHD content;
/// only a `stat` is performed.
pub fn disc_conversion_operation(
    transaction: &RenameTransaction,
    role: DiscConversionRole,
    journal_reference: Option<String>,
) -> OperationRecord {
    let mut record = operation_from_transaction(
        transaction,
        OperationKind::DiscConversion,
        journal_reference,
    );
    let format_label = "CUE/BIN to CHD (chdman)";
    let hash_of = |entry: &TransactionEntry| -> String {
        entry
            .identity
            .freshness
            .as_ref()
            .map(|freshness| {
                format!(
                    "sha256:{}",
                    freshness
                        .sha256
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                )
            })
            .unwrap_or_else(|| "hash unavailable (legacy record)".into())
    };
    match role {
        DiscConversionRole::Output => {
            let applied = transaction
                .entries
                .iter()
                .find(|entry| matches!(entry.state, EntryState::Applied));
            // Only a transaction that itself reached a genuinely completed
            // state is a candidate for "stale" (i.e. was true, may no longer
            // be) -- an interrupted/partial transaction's destination may
            // legitimately not exist yet, and that is already correctly
            // described by the shared recovery_status() above, not by this
            // liveness check.
            let outcome = if record.state == OperationState::Completed {
                applied.map(|entry| {
                    let live = fs::symlink_metadata(&entry.destination_path).ok();
                    let stale = live.as_ref().is_none_or(|metadata| {
                        !metadata.is_file() || metadata.len() != entry.identity.size_bytes
                    });
                    (entry, stale)
                })
            } else {
                applied.map(|entry| (entry, false))
            };
            if let Some((entry, stale)) = outcome {
                if stale {
                    record.state = OperationState::Stale;
                    record.recovery = OperationRecoveryStatus {
                        classification: RecoveryClassification::Stale,
                        explanation: "The converted CHD no longer matches the recorded output (moved, resized, or deleted); review before relying on this receipt.".into(),
                        actions: OperationActionAvailability {
                            review: ActionAvailability::Available,
                            ..Default::default()
                        },
                    };
                }
                record.output.summary = format!(
                    "Converted {format_label} and verified output ({}). Source preserved: this operation never reads, moves, or deletes the original CUE/BIN.",
                    hash_of(entry)
                );
            } else {
                record.output.summary = format!(
                    "Disc conversion output ({format_label}) not completed. Source preserved: this operation never reads, moves, or deletes the original CUE/BIN."
                );
            }
            // This transaction only ever moves the fingerprint-verified staged
            // file to its destination; it never reads, moves, or deletes the
            // user's original CUE/BIN, regardless of outcome. A companion
            // DiscConversionRole::SourceQuarantine record, if one exists,
            // reports the source's actual disposition -- this record does
            // not, and must never be read as proof the source still exists
            // (a separate quarantine transaction may have relocated it).
            record
                .input
                .freshness_evidence
                .push("source_preservation:not_modified_by_this_transaction".into());
            record.error = record.error.map(|error| format!("{format_label}: {error}"));
        }
        DiscConversionRole::SourceQuarantine => {
            let moved = transaction
                .entries
                .iter()
                .filter(|entry| matches!(entry.state, EntryState::Applied))
                .count();
            record.output.summary = if moved > 0 {
                format!(
                    "Source replacement requested: original CUE/BIN moved to quarantine after verified conversion ({moved} file(s), recoverable, not deleted)"
                )
            } else {
                "Source replacement requested but not completed; original CUE/BIN unchanged".into()
            };
            record
                .input
                .freshness_evidence
                .push("source_preservation:replacement_requested".into());
        }
    }
    record
}

/// Projects the existing migration/upgrade result. The retained backup is
/// verified again by hash, while the live database is diagnosed read-only.
/// No restore action is advertised because the current database API has no
/// operation-safe restore executor.
pub fn database_recovery_operation(
    report: &crate::DatabaseUpgradeReport,
    journal_reference: Option<String>,
) -> OperationRecord {
    let backup_hash = sha256_path(&report.backup_path);
    let health = crate::diagnose_database(&report.database_path);
    let backup_valid = backup_hash.as_deref() == Some(report.backup_sha256.as_str());
    let live_valid = health.database_present
        && health.open_outcome == crate::DatabaseOpenOutcome::OpenedReadOnly
        && health.quick_check.status == crate::DatabaseCheckStatus::Ok
        && health.schema_version == Some(report.to_version);
    let verified = backup_valid && live_valid;
    let state = if verified {
        OperationState::Completed
    } else {
        OperationState::Failed
    };
    let reason = if !backup_valid {
        "The retained backup no longer matches its recorded SHA-256."
    } else if !live_valid {
        "The recovered database did not pass the recorded read-only verification."
    } else {
        "Database upgrade completed; the retained pre-upgrade backup remains available for reviewed recovery."
    };
    OperationRecord {
        schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
        operation_id: format!("database-recovery:{}", report.backup_sha256),
        kind: OperationKind::DatabaseRecovery,
        state,
        created_at_unix: unix_now(),
        started_at_unix: None,
        completed_at_unix: Some(unix_now()),
        input: OperationInputSnapshot {
            source_root: Some(report.database_path.to_string_lossy().into_owned()),
            plan_generation: None,
            plan_hash: Some(format!("sha256:{}", report.backup_sha256)),
            freshness_evidence: vec![
                format!("source_size_before:{}", report.source_size_bytes_before),
                format!(
                    "source_mtime_before:{}",
                    report
                        .source_modified_unix_seconds_before
                        .map_or_else(|| "unknown".into(), |value| value.to_string())
                ),
                format!("schema:{}->{}", report.from_version, report.to_version),
            ],
            destination_occupancy_checked: true,
        },
        destination: Some(report.database_path.to_string_lossy().into_owned()),
        output: OperationOutputReceipt {
            outputs: vec![
                OperationOutput {
                    path: report.backup_path.to_string_lossy().into_owned(),
                    changed: true,
                    verified: backup_valid,
                    before_state: Some(format!("schema {}", report.from_version)),
                    after_state: Some(format!("sha256:{}", report.backup_sha256)),
                },
                OperationOutput {
                    path: report.database_path.to_string_lossy().into_owned(),
                    changed: true,
                    verified: live_valid,
                    before_state: Some(format!("schema {}", report.from_version)),
                    after_state: Some(format!("schema {}", report.to_version)),
                },
            ],
            journal_reference,
            summary: if verified {
                format!(
                    "Database recovery completed and verified; backup retained at {}",
                    report.backup_path.display()
                )
            } else {
                format!(
                    "Database recovery verification failed; review {}",
                    report.backup_path.display()
                )
            },
        },
        recovery: OperationRecoveryStatus {
            classification: if verified {
                RecoveryClassification::RequiresReview
            } else {
                RecoveryClassification::Stale
            },
            explanation: reason.into(),
            actions: OperationActionAvailability {
                review: ActionAvailability::Available,
                ..Default::default()
            },
        },
        error: (!verified).then_some(reason.into()),
    }
}

fn sha256_path(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Some(
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Projects the durable Library View receipt without replacing its history
/// format. Library View history has no before-state for safe rollback, so
/// this adapter deliberately exposes review only for partial operations.
pub fn library_view_history_operation(
    record: &LibraryViewHistoryRecord,
    journal_reference: Option<String>,
) -> OperationRecord {
    let state = if record.success {
        OperationState::Completed
    } else if record.created + record.repaired + record.removed > 0 {
        OperationState::Partial
    } else {
        OperationState::Failed
    };
    let changed = record.created + record.repaired + record.removed;
    let summary = match record.operation {
        LibraryViewHistoryOperation::Apply => format!(
            "Library View completed; {changed} links created or repaired, {} skipped, {} failed",
            record.skipped_or_collision.unwrap_or(0),
            record.failed
        ),
        LibraryViewHistoryOperation::Remove => format!(
            "Library View removed; {} links removed, {} failed",
            record.removed, record.failed
        ),
    };
    let partial = state == OperationState::Partial;
    OperationRecord {
        schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
        operation_id: format!("library-view:{}:{}", record.view_id, record.timestamp),
        kind: OperationKind::LibraryViewPublish,
        state,
        created_at_unix: 0,
        started_at_unix: None,
        completed_at_unix: None,
        input: OperationInputSnapshot {
            source_root: Some(record.manifest_path.clone()),
            plan_generation: None,
            plan_hash: None,
            freshness_evidence: vec![format!("history_timestamp:{}", record.timestamp)],
            destination_occupancy_checked: true,
        },
        destination: Some(record.destination_root.clone()),
        output: OperationOutputReceipt {
            outputs: vec![OperationOutput {
                path: record.manifest_path.clone(),
                changed: changed > 0,
                verified: record.success,
                before_state: None,
                after_state: Some(format!("{} planned entries", record.planned_count)),
            }],
            journal_reference,
            summary,
        },
        recovery: OperationRecoveryStatus {
            classification: if partial {
                RecoveryClassification::RequiresReview
            } else {
                RecoveryClassification::Unrecoverable
            },
            explanation: if partial {
                "Some view entries failed; inspect the saved diagnostics.".into()
            } else {
                "Historical record has no safe rollback or resume evidence.".into()
            },
            actions: OperationActionAvailability {
                review: if partial {
                    ActionAvailability::Available
                } else {
                    ActionAvailability::Unavailable
                },
                ..Default::default()
            },
        },
        error: record.warnings.first().cloned(),
    }
}

/// Explicitly projects a RomM transaction. RomM shares the rename executor,
/// whose legacy journal cannot identify the publication target afterwards,
/// so callers must supply this semantic context.
pub fn romm_publication_operation(
    transaction: &RenameTransaction,
    journal_reference: Option<String>,
) -> OperationRecord {
    operation_from_transaction(transaction, OperationKind::RommPublish, journal_reference)
}

/// Projects an ES-DE gamelist publication and checks the live output before
/// advertising rollback. A changed gamelist is stale/review-required rather
/// than rollback-capable, even when the in-memory plan is otherwise valid.
pub fn es_de_publication_operation(
    publication: &EsDeGamelistPublication,
    completed: bool,
    journal_reference: Option<String>,
) -> OperationRecord {
    let output_hash = digest_bytes(publication.new_content.as_bytes());
    let operation_id = format!(
        "es-de:{}:{output_hash}",
        publication.gamelist_path.display()
    );
    let current = std::fs::read_to_string(&publication.gamelist_path).ok();
    let matches_output = current.as_deref() == Some(publication.new_content.as_str());
    let stale = completed && !publication.is_unchanged() && !matches_output;
    let rollback = completed && !publication.is_unchanged() && matches_output;
    let state = if !completed {
        OperationState::Planned
    } else if stale {
        OperationState::Stale
    } else {
        OperationState::Completed
    };
    let classification = if stale {
        RecoveryClassification::Stale
    } else if rollback {
        RecoveryClassification::SafeToRollback
    } else {
        RecoveryClassification::Unrecoverable
    };
    OperationRecord {
        schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
        operation_id,
        kind: OperationKind::EsDePublish,
        state,
        created_at_unix: 0,
        started_at_unix: None,
        completed_at_unix: completed.then_some(0),
        input: OperationInputSnapshot {
            source_root: None,
            plan_generation: None,
            plan_hash: Some(format!("sha256:{output_hash}")),
            freshness_evidence: vec![format!("system:{}", publication.es_de_system)],
            destination_occupancy_checked: true,
        },
        destination: Some(publication.gamelist_path.to_string_lossy().into_owned()),
        output: OperationOutputReceipt {
            outputs: vec![OperationOutput {
                path: publication.gamelist_path.to_string_lossy().into_owned(),
                changed: !publication.is_unchanged(),
                verified: !completed || matches_output,
                before_state: Some(format!(
                    "{} existing entries",
                    publication.already_present.len()
                )),
                after_state: Some(format!("{} entries added", publication.added.len())),
            }],
            journal_reference,
            summary: format!(
                "ES-DE publication planned {} and added {}",
                publication.added.len() + publication.already_present.len(),
                publication.added.len()
            ),
        },
        recovery: OperationRecoveryStatus {
            classification,
            explanation: if stale {
                "The gamelist changed after publication; rollback requires review.".into()
            } else if rollback {
                "The gamelist still matches this publication and can be rolled back safely.".into()
            } else if !completed {
                "Preview only; nothing has been published.".into()
            } else {
                "No changed output requires recovery.".into()
            },
            actions: OperationActionAvailability {
                rollback: if rollback {
                    ActionAvailability::Available
                } else {
                    ActionAvailability::Unavailable
                },
                review: if stale {
                    ActionAvailability::Available
                } else {
                    ActionAvailability::Unavailable
                },
                ..Default::default()
            },
        },
        error: None,
    }
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn shared_apply_operation(
    journal: &SharedApplyJournal,
    journal_reference: Option<String>,
    rollback: Option<&SharedRollbackPreview>,
) -> OperationRecord {
    let kind = match journal.context.adapter {
        PreviewAdapter::LocalModPackage
        | PreviewAdapter::CemuGraphicPack
        | PreviewAdapter::Rpcs3OrdinaryMod => OperationKind::ModApply,
        PreviewAdapter::RetroArch
        | PreviewAdapter::Pcsx2
        | PreviewAdapter::Dolphin
        | PreviewAdapter::Ppsspp
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

/// A persisted successful rollback is stronger than the original apply's
/// outcome. The shared executor writes this marker only after all entries are
/// restored/removed; a preview, failed marker, or another destination cannot
/// resolve an operation. No fresh recovery capability is advertised here.
fn project_completed_shared_rollback(
    record: &mut OperationRecord,
    receipt: &SharedRollbackPreview,
) {
    if receipt.schema_version != 1
        || receipt.original_operation_id != record.operation_id
        || record.destination.as_deref() != Some(&receipt.destination_root.display)
        || !receipt.entries.iter().all(|entry| {
            matches!(
                entry.outcome,
                SharedRollbackOutcome::RemovedInstalledFile
                    | SharedRollbackOutcome::RestoredBackup
                    | SharedRollbackOutcome::NoChangeRequired
            )
        })
    {
        return;
    }
    let restored: std::collections::BTreeSet<_> = receipt
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.outcome,
                SharedRollbackOutcome::RemovedInstalledFile | SharedRollbackOutcome::RestoredBackup
            )
        })
        .filter_map(|entry| entry.destination.as_ref().map(|path| path.display.as_str()))
        .collect();
    if !record
        .output
        .outputs
        .iter()
        .filter(|output| output.changed)
        .all(|output| restored.contains(output.path.as_str()))
    {
        // A syntactically valid but truncated marker is not proof that every
        // changed output was restored. Keep the original unresolved state.
        return;
    }
    record.state = OperationState::RolledBack;
    record.error = None;
    record.output.summary =
        "The saved rollback receipt records all changes restored or removed.".into();
    record.recovery = OperationRecoveryStatus {
        classification: RecoveryClassification::Unrecoverable,
        explanation: "Rollback completed according to its durable receipt; no further recovery action is required.".into(),
        actions: OperationActionAvailability::default(),
    };
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
    use crate::dat::rename_apply::{ObjectFreshness, ObjectIdentity, ObjectKind, TransactionEntry};
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

    pub(super) fn shared_journal(
        adapter: PreviewAdapter,
        status: SharedApplyStatus,
    ) -> SharedApplyJournal {
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

    #[test]
    fn library_view_history_projects_completed_and_partial_receipts() {
        let mut record = LibraryViewHistoryRecord {
            schema_version: crate::LIBRARY_VIEW_HISTORY_SCHEMA_VERSION,
            timestamp: "2026-01-02T03:04:05Z".into(),
            operation: LibraryViewHistoryOperation::Apply,
            view_id: "view-1".into(),
            view_name: "Main".into(),
            profile_kind: crate::library_views::FrontendProfileKind::Generic,
            destination_root: "/views/main".into(),
            manifest_path: "/views/main/manifest.json".into(),
            planned_count: 3,
            created: 3,
            repaired: 0,
            removed: 0,
            unchanged: 0,
            failed: 0,
            skipped_or_collision: Some(0),
            success: true,
            warnings: Vec::new(),
        };
        let complete = library_view_history_operation(&record, Some("history.json".into()));
        assert_eq!(complete.kind, OperationKind::LibraryViewPublish);
        assert_eq!(complete.state, OperationState::Completed);
        assert_eq!(
            complete.recovery.actions.rollback,
            ActionAvailability::Unavailable
        );
        assert!(complete.output.summary.contains("3 links"));

        record.created = 1;
        record.failed = 2;
        record.success = false;
        record.warnings = vec!["missing source".into()];
        let partial = library_view_history_operation(&record, None);
        assert_eq!(partial.state, OperationState::Partial);
        assert_eq!(
            partial.recovery.classification,
            RecoveryClassification::RequiresReview
        );
    }

    #[test]
    fn romm_publication_is_explicitly_distinguished_from_legacy_playing_library() {
        let record = romm_publication_operation(&transaction(TransactionState::Applied), None);
        assert_eq!(record.kind, OperationKind::RommPublish);
        assert_eq!(record.state, OperationState::Completed);
    }

    #[test]
    fn es_de_publication_only_advertises_rollback_for_unchanged_output() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("gamelist.xml");
        let publication = EsDeGamelistPublication {
            es_de_system: "nes",
            gamelist_path: path.clone(),
            previous_content: None,
            new_content: "<gameList/>".into(),
            added: vec![crate::launch::es_de_publish::EsDePublicationEntry {
                dat_entry_name: "Game".into(),
                destination_path: PathBuf::from("/library/Game.zip"),
            }],
            already_present: Vec::new(),
        };
        std::fs::write(&path, &publication.new_content).unwrap();
        let safe = es_de_publication_operation(&publication, true, None);
        assert_eq!(safe.kind, OperationKind::EsDePublish);
        assert_eq!(
            safe.recovery.classification,
            RecoveryClassification::SafeToRollback
        );
        assert_eq!(
            safe.recovery.actions.rollback,
            ActionAvailability::Available
        );

        std::fs::write(&path, "<gameList><game/></gameList>").unwrap();
        let stale = es_de_publication_operation(&publication, true, None);
        assert_eq!(stale.state, OperationState::Stale);
        assert_eq!(stale.recovery.classification, RecoveryClassification::Stale);
        assert_eq!(
            stale.recovery.actions.rollback,
            ActionAvailability::Unavailable
        );
        assert_eq!(stale.recovery.actions.review, ActionAvailability::Available);
    }

    #[test]
    fn database_recovery_requires_intact_backup_and_verified_live_database() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("library.sqlite3");
        let backup_path = temp.path().join("library.sqlite3.backup");
        let database = crate::Database::open_or_create(&database_path).unwrap();
        database.close().unwrap();
        std::fs::copy(&database_path, &backup_path).unwrap();
        let bytes = std::fs::read(&backup_path).unwrap();
        let mut digest = Sha256::new();
        digest.update(bytes);
        let backup_sha256 = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let report = crate::DatabaseUpgradeReport {
            database_path: database_path.clone(),
            backup_path: backup_path.clone(),
            backup_sha256,
            source_size_bytes_before: std::fs::metadata(&database_path).unwrap().len(),
            source_modified_unix_seconds_before: None,
            from_version: crate::latest_schema_version(),
            to_version: crate::latest_schema_version(),
            applied_versions: Vec::new(),
        };
        let record = database_recovery_operation(&report, Some("db-upgrade".into()));
        assert_eq!(record.kind, OperationKind::DatabaseRecovery);
        assert_eq!(record.state, OperationState::Completed);
        assert_eq!(
            record.recovery.classification,
            RecoveryClassification::RequiresReview
        );
        assert_eq!(
            record.recovery.actions.rollback,
            ActionAvailability::Unavailable
        );
        assert!(record.output.summary.contains("completed and verified"));

        std::fs::write(&backup_path, b"changed").unwrap();
        let stale = database_recovery_operation(&report, None);
        assert_eq!(stale.state, OperationState::Failed);
        assert_eq!(stale.recovery.classification, RecoveryClassification::Stale);
        assert_eq!(stale.recovery.actions.review, ActionAvailability::Available);
    }

    // -- Disc conversion --------------------------------------------------
    //
    // Fixtures are hand-built RenameTransaction values shaped exactly like
    // what crate::repair::optical_conversion actually produces (verified by
    // reading that module directly, not guessed): a single-entry "output"
    // transaction whose source_path sits under a
    // DISC_CONVERSION_STAGING_PREFIX-named staging directory, and a
    // "source quarantine" transaction whose destination_path sits under a
    // DISC_CONVERSION_QUARANTINE_SUBDIR component. No real chdman/tempdir
    // execution is needed to test the projection layer, matching every
    // other adapter's tests in this module.

    fn disc_conversion_output_entry(state: EntryState, size_bytes: u64) -> TransactionEntry {
        TransactionEntry {
            source_path: PathBuf::from(format!(
                "/library/{DISC_CONVERSION_STAGING_PREFIX}4242-0/output.chd"
            )),
            destination_path: PathBuf::from("/library/Disc.chd"),
            original_basename: "output.chd".into(),
            proposed_basename: "Disc.chd".into(),
            identity: ObjectIdentity {
                size_bytes,
                modified_unix: 100,
                kind: ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 1,
                #[cfg(unix)]
                dev: 1,
                freshness: Some(ObjectFreshness {
                    version: 1,
                    modified: std::time::UNIX_EPOCH,
                    sha256: [0xab; 32],
                }),
            },
            operation: Default::default(),
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state,
            failure_reason: None,
            applied_at_unix: (state == EntryState::Applied).then_some(101),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }
    }

    fn disc_conversion_output_transaction(state: TransactionState) -> RenameTransaction {
        let entry_state = if state == TransactionState::Applied {
            EntryState::Applied
        } else {
            EntryState::Planned
        };
        RenameTransaction {
            transaction_id: "disc-conversion-output-test".into(),
            plan_generation: 1,
            classifier_version: Some("classifier-v1".into()),
            created_at_unix: 100,
            source_scan_root: "/library".into(),
            state,
            entries: vec![disc_conversion_output_entry(entry_state, 2048 * 16)],
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    fn disc_conversion_quarantine_transaction(state: TransactionState) -> RenameTransaction {
        let entry_state = if state == TransactionState::Applied {
            EntryState::Applied
        } else {
            EntryState::Planned
        };
        let entry = |name: &str| TransactionEntry {
            source_path: PathBuf::from(format!("/library/{name}")),
            destination_path: PathBuf::from(format!(
                "/library/{DISC_CONVERSION_QUARANTINE_SUBDIR}/abcd1234/{name}"
            )),
            original_basename: name.into(),
            proposed_basename: name.into(),
            identity: ObjectIdentity {
                size_bytes: 10,
                modified_unix: 100,
                kind: ObjectKind::RegularFile,
                #[cfg(unix)]
                ino: 2,
                #[cfg(unix)]
                dev: 1,
                freshness: None,
            },
            operation: Default::default(),
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state: entry_state,
            failure_reason: None,
            applied_at_unix: (entry_state == EntryState::Applied).then_some(101),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        };
        RenameTransaction {
            transaction_id: "disc-conversion-quarantine-test".into(),
            plan_generation: 1,
            classifier_version: Some("classifier-v1".into()),
            created_at_unix: 100,
            source_scan_root: "/library".into(),
            state,
            entries: vec![entry("Disc.cue"), entry("Disc.bin")],
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    #[test]
    fn disc_conversion_role_is_detected_from_stable_path_shapes_only() {
        assert_eq!(
            disc_conversion_role(&disc_conversion_output_transaction(
                TransactionState::Applied
            )),
            Some(DiscConversionRole::Output)
        );
        assert_eq!(
            disc_conversion_role(&disc_conversion_quarantine_transaction(
                TransactionState::Applied
            )),
            Some(DiscConversionRole::SourceQuarantine)
        );
        assert_eq!(
            disc_conversion_role(&transaction(TransactionState::Applied)),
            None
        );
    }

    #[test]
    fn registry_classifies_disc_conversion_transactions_ahead_of_the_dat_rename_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let output = disc_conversion_output_transaction(TransactionState::Applied);
        crate::dat::rename_apply::write_journal(temp.path(), &output).unwrap();
        let registry = OperationRegistry::from_rename_journals(temp.path());
        assert_eq!(registry.records.len(), 1);
        assert_eq!(registry.records[0].kind, OperationKind::DiscConversion);
    }

    #[test]
    fn completed_output_receipt_reports_verified_hash_and_never_claims_source_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let mut transaction = disc_conversion_output_transaction(TransactionState::Applied);
        let destination = temp.path().join("Disc.chd");
        std::fs::write(&destination, vec![0_u8; 2048 * 16]).unwrap();
        transaction.entries[0].destination_path = destination;
        let record = disc_conversion_operation(&transaction, DiscConversionRole::Output, None);
        assert_eq!(record.kind, OperationKind::DiscConversion);
        assert_eq!(record.state, OperationState::Completed);
        assert!(record.output.summary.contains("verified output"));
        assert!(record.output.summary.contains("sha256:"));
        assert!(record.output.summary.to_lowercase().contains("preserved"));
        // The summary may reassure that the source was NOT deleted (as it
        // does here); it must never claim deletion occurred.
        assert!(
            !record
                .output
                .summary
                .to_lowercase()
                .contains("source deleted")
        );
        assert!(!record.output.summary.to_lowercase().contains("deleted the"));
    }

    #[test]
    fn source_quarantine_receipt_states_replacement_requested_never_deletion() {
        let record = disc_conversion_operation(
            &disc_conversion_quarantine_transaction(TransactionState::Applied),
            DiscConversionRole::SourceQuarantine,
            None,
        );
        assert!(
            record
                .output
                .summary
                .contains("Source replacement requested")
        );
        assert!(record.output.summary.contains("recoverable"));
        assert!(
            !record
                .output
                .summary
                .to_lowercase()
                .contains("deleted from disk")
        );
        assert!(
            record
                .input
                .freshness_evidence
                .iter()
                .any(|entry| entry.contains("replacement_requested"))
        );
    }

    #[test]
    fn stale_destination_overrides_completed_state_without_rehashing_content() {
        let temp = tempfile::tempdir().unwrap();
        let mut transaction = disc_conversion_output_transaction(TransactionState::Applied);
        let destination = temp.path().join("Disc.chd");
        transaction.entries[0].destination_path = destination.clone();
        // Recorded size (2048*16) does not match what is actually on disk.
        std::fs::write(&destination, b"only a few bytes").unwrap();
        let record = disc_conversion_operation(&transaction, DiscConversionRole::Output, None);
        assert_eq!(record.state, OperationState::Stale);
        assert_eq!(
            record.recovery.classification,
            RecoveryClassification::Stale
        );
        assert_eq!(
            record.recovery.actions.review,
            ActionAvailability::Available
        );
    }

    #[test]
    fn matching_destination_stays_completed() {
        let temp = tempfile::tempdir().unwrap();
        let mut transaction = disc_conversion_output_transaction(TransactionState::Applied);
        let destination = temp.path().join("Disc.chd");
        let bytes = vec![0_u8; 2048 * 16];
        std::fs::write(&destination, &bytes).unwrap();
        transaction.entries[0].destination_path = destination;
        let record = disc_conversion_operation(&transaction, DiscConversionRole::Output, None);
        assert_eq!(record.state, OperationState::Completed);
    }

    #[test]
    fn missing_destination_is_stale_not_completed() {
        let temp = tempfile::tempdir().unwrap();
        let mut transaction = disc_conversion_output_transaction(TransactionState::Applied);
        transaction.entries[0].destination_path = temp.path().join("never-written.chd");
        let record = disc_conversion_operation(&transaction, DiscConversionRole::Output, None);
        assert_eq!(record.state, OperationState::Stale);
    }

    #[test]
    fn partial_output_is_reviewable_and_never_advertises_resume() {
        let mut transaction = disc_conversion_output_transaction(TransactionState::ApplyFailed);
        transaction.entries[0].state = EntryState::Applied;
        let record = disc_conversion_operation(&transaction, DiscConversionRole::Output, None);
        assert_eq!(record.state, OperationState::Partial);
        assert_eq!(
            record.recovery.actions.resume,
            ActionAvailability::Unavailable
        );
    }

    #[test]
    fn interrupted_output_transaction_is_rollback_capable_via_the_shared_engine() {
        let mut transaction = disc_conversion_output_transaction(TransactionState::Applying);
        transaction.entries[0].state = EntryState::Applied;
        let record = disc_conversion_operation(&transaction, DiscConversionRole::Output, None);
        assert_eq!(record.state, OperationState::Running);
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
    fn rollback_failure_requires_review_and_blocks_resume() {
        let mut transaction = disc_conversion_output_transaction(TransactionState::RollbackFailed);
        transaction.entries[0].state = EntryState::ApplyFailed;
        let record = disc_conversion_operation(&transaction, DiscConversionRole::Output, None);
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
    fn legacy_output_record_without_a_freshness_proof_reports_hash_unavailable_not_a_fabricated_one()
     {
        let mut transaction = disc_conversion_output_transaction(TransactionState::Applied);
        transaction.entries[0].identity.freshness = None;
        transaction.entries[0].destination_path = PathBuf::from("/does/not/exist/on/this/host.chd");
        let record = disc_conversion_operation(&transaction, DiscConversionRole::Output, None);
        assert!(
            record
                .output
                .summary
                .contains("hash unavailable (legacy record)")
        );
    }

    #[test]
    fn no_specialist_dreamcast_or_multitrack_conversion_path_exists_today() {
        // Documents real, current behavior rather than aspirational support:
        // build_chd_conversion_plan only accepts a single MODE1/2048 data
        // track (see crate::ingestion::cue_bin::CueLayout::supported_single_mode1_2048),
        // so a multi-track (e.g. Dreamcast GD-ROM-style) CUE is refused
        // before any staging, journal, or partial state is ever created --
        // there is nothing for this projection layer to recover, and this
        // test exists so a future specialist path is added deliberately,
        // not accidentally assumed to already work.
        let dir = tempfile::tempdir().unwrap();
        let cue = dir.path().join("multitrack.cue");
        std::fs::write(
            &cue,
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("data.bin"), [0_u8; 2048]).unwrap();
        std::fs::write(dir.path().join("audio.bin"), [0_u8; 2352]).unwrap();
        let error = crate::repair::optical_conversion::build_chd_conversion_plan(
            &cue,
            &dir.path().join("out.chd"),
            crate::repair::optical_conversion::ChdConversionSourceMode::KeepSource,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            crate::repair::optical_conversion::ChdConversionError::InvalidSource(_)
                | crate::repair::optical_conversion::ChdConversionError::ChdmanUnavailable(_)
        ));
        // No journal directory was ever created for this refused attempt.
        assert!(!dir.path().join("journal").exists());
    }
}
