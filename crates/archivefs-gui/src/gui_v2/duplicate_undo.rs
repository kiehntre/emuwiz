//! Duplicate-quarantine Undo presentation. The core remains the sole rollback engine.
use std::sync::atomic::AtomicBool;

use archivefs_core::dat::rename_apply::{
    journal,
    model::{EntryState, RollbackResult, TransactionEntry, TransactionState},
};
use archivefs_core::repair::{QuarantineRollbackOutcome, rollback_quarantine_transaction};

use super::{App, activity, backend::DuplicateRepairRecord};

pub(super) struct DuplicateUndoResult {
    pub generation: u64,
    pub record: DuplicateRepairRecord,
    pub outcome: Result<QuarantineRollbackOutcome, String>,
}

pub(super) fn needs_restore(entry: &TransactionEntry) -> bool {
    matches!(
        entry.state,
        EntryState::Applied | EntryState::RollbackFailed | EntryState::RollingBack
    )
}

pub(super) fn undo(
    mut record: DuplicateRepairRecord,
    cancel: &AtomicBool,
    generation: u64,
) -> DuplicateUndoResult {
    // A failed move is not eligible for another rollback pass in the shared engine.
    // Re-enter its existing recovery reconciliation instead of blindly skipping it
    // (which could otherwise claim full success). Reconciliation proves whether the
    // recorded object is still quarantined, already restored, or needs manual review.
    if record.transaction.state != TransactionState::RolledBack {
        for entry in &mut record.transaction.entries {
            if entry.state == EntryState::RollbackFailed {
                entry.state = EntryState::RollingBack;
            }
        }
    }
    let outcome = rollback_quarantine_transaction(
        &mut record.transaction,
        &record.journal_dir,
        cancel,
        &record.trusted_root,
    );
    // An I/O error can follow progress, including an unpersisted terminal state.
    // Prefer the durable receipt; never claim completion from that failed write.
    if outcome.is_err() {
        if let Some(path) =
            journal::journal_path(&record.journal_dir, &record.transaction.transaction_id)
            && let Ok(persisted) = journal::read_journal(&path)
        {
            record.transaction = persisted;
        } else {
            record.transaction.state = TransactionState::RollbackFailed;
        }
    }
    DuplicateUndoResult {
        generation,
        record,
        outcome,
    }
}

pub(super) struct DuplicateUndoJob {
    pub id: u64,
    pub generation: u64,
    pub before: DuplicateRepairRecord,
}

fn same_receipt(a: &DuplicateRepairRecord, b: &DuplicateRepairRecord) -> bool {
    a.transaction == b.transaction
        && a.trusted_root == b.trusted_root
        && a.journal_dir == b.journal_dir
}

impl App {
    pub(super) fn undo_history_entry(&mut self, transaction_id: &str) {
        if self.undo_job.is_some() {
            return;
        }
        let mut records = self
            .repair_history
            .iter()
            .filter(|record| record.transaction.transaction_id == transaction_id);
        let Some(record) = records.next().cloned() else {
            return;
        };
        if records.next().is_some() {
            return;
        }
        self.undo_generation = self.undo_generation.wrapping_add(1);
        let generation = self.undo_generation;
        let id = self.activity.queue(
            "Undoing the duplicate quarantine",
            super::routes::Route::Section(super::routes::Section::History),
            false,
        );
        self.undo_job = Some(DuplicateUndoJob {
            id,
            generation,
            before: record.clone(),
        });
        self.undo_confirm = None;
        if !self.send(
            id,
            super::backend::Command::UndoDuplicateRepair {
                generation,
                record: Box::new(record),
                cancel: std::sync::Arc::new(AtomicBool::new(false)),
            },
        ) {
            self.undo_job = None;
        }
    }

    pub(super) fn duplicate_undo_done(&mut self, id: u64, mut result: DuplicateUndoResult) {
        let Some(job) = self.undo_job.as_ref() else {
            self.activity.supersede(
                id,
                "Obsolete Undo completion ignored; History was preserved.".into(),
            );
            return;
        };
        if job.id != id {
            self.activity.supersede(
                id,
                "Obsolete Undo completion ignored; the newer job remains active.".into(),
            );
            return;
        }
        if job.generation != result.generation
            || self.undo_generation != result.generation
            || job.before.transaction.transaction_id != result.record.transaction.transaction_id
            || job.before.trusted_root != result.record.trusted_root
            || job.before.journal_dir != result.record.journal_dir
            || !self.activity.jobs.get(&id).is_some_and(|row| row.active())
        {
            return;
        }
        let matching = self
            .repair_history
            .iter()
            .filter(|record| {
                record.transaction.transaction_id == job.before.transaction.transaction_id
            })
            .collect::<Vec<_>>();
        if matching.len() != 1 || !same_receipt(matching[0], &job.before) {
            self.undo_job = None;
            self.activity.supersede(
                id,
                "Undo completion requires review: newer recovery evidence was preserved.".into(),
            );
            return;
        }
        self.undo_job = None;
        if result.outcome.is_err() && result.record.transaction.is_fully_rolled_back() {
            // Readback cannot acknowledge a failed write/sync. This is only the
            // displayed recovery snapshot: retain the actual journal unchanged
            // for explicit review/reload rather than painting it as successful.
            result.record.transaction.state = TransactionState::RollbackFailed;
        }
        let (summary, settled) = match &result.outcome {
            Ok(outcome) => match &outcome.rollback.result {
                RollbackResult::FullyRolledBack if result.record.transaction.is_fully_rolled_back() => (
                    "The quarantined file was restored safely.".to_string(), activity::Settled::Done,
                ),
                RollbackResult::FullyRolledBack => (
                    "Undo could not confirm restoration; the recovery receipt is inconsistent. Review History.".into(),
                    activity::Settled::Failed("Rollback outcome and transaction receipt disagree; retained for review.".into()),
                ),
                rollback => {
                    let failed = rollback.failed();
                    let stopped = failed.iter().any(|(_, reason)| reason == "rollback cancelled while entries were still applied");
                    let restored = result.record.transaction.entries.iter()
                        .filter(|entry| entry.state == EntryState::RolledBack).count();
                    let summary = if stopped {
                        format!("Undo was stopped before restoration completed. {restored} file(s) restored. Review History to retry the remaining files.")
                    } else if matches!(rollback, RollbackResult::PartiallyRolledBack { .. }) {
                        format!("Undo only partially restored the quarantined files. {restored} file(s) restored. Review History to resolve the remaining problems and retry.")
                    } else if result.record.transaction.entries.iter().any(|entry|
                        matches!(entry.state, EntryState::Applying | EntryState::RollingBack)) {
                        "Undo remains uncertain; restoration could not be confirmed. Review History and the recovery journal.".to_string()
                    } else {
                        "Undo failed to restore the remaining quarantined files. Review History to resolve the problems and retry.".to_string()
                    };
                    let detail = failed.iter().map(|(path, reason)| format!("{}: {reason}", path.display()))
                        .chain(std::iter::once(format!("Recovery journal: {}", result.record.journal_dir.display())))
                        .collect::<Vec<_>>().join("\n");
                    let settled = if stopped { activity::Settled::Stopped } else { activity::Settled::Failed(detail) };
                    (summary, settled)
                }
            },
            Err(error) => (
                "Undo could not confirm restoration. Review History and the recovery journal before retrying.".to_string(),
                activity::Settled::Failed(format!("{error}\nRecovery journal: {}", result.record.journal_dir.display())),
            ),
        };
        let transaction_id = result.record.transaction.transaction_id.clone();
        if let Some(existing) = self.repair_history.iter_mut().find(|item| {
            item.transaction.transaction_id == result.record.transaction.transaction_id
        }) {
            *existing = result.record;
        }
        // A retry must preview the new receipt, never reuse an earlier safety check.
        self.history_view.built_for = None;
        self.history_view.previews.remove(&transaction_id);
        if self.history_view.confirm_undo.as_ref() == Some(&transaction_id) {
            self.history_view.confirm_undo = None;
        }
        self.repair_result = Some(summary.clone());
        self.activity.settle(id, summary, settled);
    }
}
