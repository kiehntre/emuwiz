//! Projection of the authoritative SQLite receipt, never a second Undo engine.
use super::*;
use crate::{DiscoveryImportOperation, DiscoveryImportUndoPreview};

pub fn discovery_import_operation(
    op: &DiscoveryImportOperation,
    preview: Option<&DiscoveryImportUndoPreview>,
) -> OperationRecord {
    let current = preview.filter(|p| p.operation.operation_id == op.operation_id);
    let safe = current.is_some_and(|p| p.can_undo());
    let created =
        crate::attention::receipt_utc_seconds(&op.created_at).and_then(|v| u64::try_from(v).ok());
    let explanation = if op.undone {
        "This committed import has been undone. Its receipt is retained.".into()
    } else if let Some(p) = current {
        if p.blockers.is_empty() {
            "Only the exact created archive rows and their recorded platform assignments may be removed. Confirm revalidates the entire batch under SQLite's writer lock.".into()
        } else {
            p.blockers.join(" ")
        }
    } else {
        "Preview Undo to check current records and dependencies. Historical success alone does not establish current Undo safety.".into()
    };
    OperationRecord {
        schema_version: OPERATION_RECEIPT_SCHEMA_VERSION,
        operation_id: format!("discovery-import:{}", op.operation_id),
        kind: OperationKind::DiscoveryImport,
        state: if op.undone {
            OperationState::RolledBack
        } else {
            OperationState::Completed
        },
        created_at_unix: created.unwrap_or(0),
        started_at_unix: created,
        completed_at_unix: created,
        input: OperationInputSnapshot {
            source_root: Some(op.root.display().to_string()),
            plan_hash: Some(op.approval_sha256.clone()),
            freshness_evidence: op.catalogue_basis.clone(),
            ..Default::default()
        },
        destination: None,
        output: OperationOutputReceipt {
            outputs: op
                .items
                .iter()
                .map(|i| OperationOutput {
                    path: format!("database:archives/{}", i.archive_id),
                    changed: true,
                    verified: false,
                    before_state: Some("Absent before this import".into()),
                    after_state: Some(if op.undone {
                        "Removed by committed selective Undo".into()
                    } else {
                        "Created by this committed import".into()
                    }),
                })
                .collect(),
            journal_reference: Some(format!(
                "sqlite:discovery_import_operations/{}",
                op.operation_id
            )),
            summary: format!(
                "Discovery import: {} inserted, {} preexisting comparison records retained/skipped, {} refused during Apply, {} context exclusions. {}",
                op.items.len(),
                op.skipped,
                op.refused,
                op.excluded,
                if op.undone {
                    "Undone; media files preserved."
                } else {
                    "Committed; media files preserved."
                }
            ),
        },
        recovery: OperationRecoveryStatus {
            classification: if op.undone {
                RecoveryClassification::Unrecoverable
            } else if safe {
                RecoveryClassification::SafeToRollback
            } else {
                RecoveryClassification::RequiresReview
            },
            explanation,
            actions: OperationActionAvailability {
                rollback: if safe {
                    ActionAvailability::Available
                } else {
                    ActionAvailability::Unavailable
                },
                review: if op.undone {
                    ActionAvailability::Unavailable
                } else {
                    ActionAvailability::Available
                },
                resume: ActionAvailability::Unavailable,
            },
        },
        error: None,
    }
}
impl OperationRegistry {
    pub fn append_discovery_import_history(
        &mut self,
        db: &crate::Database,
        offset: usize,
        limit: usize,
    ) {
        match db.discovery_import_history(offset, limit) {
            Ok(ops) => self
                .records
                .extend(ops.iter().map(|op| discovery_import_operation(op, None))),
            Err(e) => self.problems.push(e),
        }
    }
}
