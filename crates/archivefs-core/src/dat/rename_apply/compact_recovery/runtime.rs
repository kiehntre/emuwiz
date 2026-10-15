//! Read-only consumer projection. No serialized evidence is mutation permission.
use super::*;
use crate::dat::rename_apply::model::{ObjectKind, TransactionOperation, TransactionState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    AwaitingApproval,
    Applied,
    PartiallyApplied,
    ApplyFailed,
    UndoPartiallyCompleted,
    RollbackFailed,
    FullyRolledBack,
    RecoveryBlocked,
    EvidenceUnknownOrConflicting,
    UnsupportedOperation,
    LegacyMigrationRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextAction {
    ReviewApply,
    ReviewUndo,
    InspectEvidence,
    OfflineMigration,
    None,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoAvailability {
    /// Journal records alone never make Undo available.
    RequiresFilesystemInspection,
    /// Current synthetic lease/filesystem inspection succeeded. Still requires
    /// explicit confirmation and validation again before execution.
    AvailableAfterConfirmation,
    Blocked,
    NotNeeded,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReport {
    pub state: RuntimeState,
    pub next_action: NextAction,
    pub undo: UndoAvailability,
    pub outstanding_operations: Vec<usize>,
}
impl RuntimeReport {
    pub fn blocked(state: RuntimeState) -> Self {
        Self {
            state,
            next_action: if state == RuntimeState::LegacyMigrationRequired {
                NextAction::OfflineMigration
            } else {
                NextAction::InspectEvidence
            },
            undo: UndoAvailability::Blocked,
            outstanding_operations: Vec::new(),
        }
    }
}

/// Legacy detection is deliberately separate from V2 state reconstruction.
/// A readable legacy receipt cannot be offered V2 Undo or silently migrated.
pub fn legacy(bytes: &[u8], limit: usize) -> RuntimeReport {
    RuntimeReport::blocked(if Authority::decode_legacy(bytes, limit).is_ok() {
        RuntimeState::LegacyMigrationRequired
    } else {
        RuntimeState::EvidenceUnknownOrConflicting
    })
}

pub fn project(base: &Authority, stream: &[u8], limit: usize) -> RuntimeReport {
    let Ok(obs) = materialize(base, stream, limit) else {
        return RuntimeReport::blocked(RuntimeState::EvidenceUnknownOrConflicting);
    };
    if base.transaction.entries.is_empty()
        || !base.transaction.created_directories.is_empty()
        || base.transaction.recovery_resolution.is_some()
        || crate::dat::rename_apply::exact_resume::has_envelope(&base.transaction)
        || base.transaction.entries.iter().any(|e| {
            e.operation != TransactionOperation::RenameMove
                || e.identity.kind != ObjectKind::RegularFile
        })
    {
        return RuntimeReport::blocked(RuntimeState::UnsupportedOperation);
    }
    if base.transaction.state != TransactionState::Planned
        || base
            .transaction
            .entries
            .iter()
            .any(|e| e.state != EntryState::Planned)
    {
        return RuntimeReport::blocked(RuntimeState::LegacyMigrationRequired);
    }
    let failure = stream
        .chunks_exact(FRAME_BYTES)
        .any(|f| f[46] == Fact::ApplyFailure as u8);
    let undo_failure = stream
        .chunks_exact(FRAME_BYTES)
        .any(|f| f[46] == Fact::UndoFailure as u8);
    let any = |s| obs.entries.contains(&s);
    let state = if obs.rollback_recorded {
        RuntimeState::FullyRolledBack
    } else if any(ObservedEntry::UndoUnresolved) || any(ObservedEntry::Undone) {
        RuntimeState::UndoPartiallyCompleted
    } else if failure || obs.cancelled {
        RuntimeState::ApplyFailed
    } else if obs.entries.iter().all(|s| *s == ObservedEntry::Applied) {
        RuntimeState::Applied
    } else if any(ObservedEntry::Applied) || any(ObservedEntry::ApplyUnresolved) {
        RuntimeState::PartiallyApplied
    } else if any(ObservedEntry::Unchanged) {
        RuntimeState::ApplyFailed
    } else {
        RuntimeState::AwaitingApproval
    };
    let next_action = if undo_failure {
        NextAction::InspectEvidence
    } else {
        match state {
            RuntimeState::FullyRolledBack => NextAction::None,
            RuntimeState::AwaitingApproval => NextAction::ReviewApply,
            _ => NextAction::ReviewUndo,
        }
    };
    RuntimeReport {
        state,
        next_action,
        undo: if undo_failure {
            UndoAvailability::Blocked
        } else if matches!(
            state,
            RuntimeState::AwaitingApproval | RuntimeState::FullyRolledBack
        ) {
            UndoAvailability::NotNeeded
        } else {
            UndoAvailability::RequiresFilesystemInspection
        },
        outstanding_operations: obs
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                (!matches!(s, ObservedEntry::Undone | ObservedEntry::Unchanged)).then_some(i)
            })
            .collect(),
    }
}
