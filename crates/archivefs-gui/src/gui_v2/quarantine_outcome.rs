//! What a duplicate-quarantine Apply actually did, read from the recorded
//! transaction instead of from the fact that the worker returned at all.
//!
//! The core executor (`repair::quarantine::apply_quarantine_transaction`)
//! returns `Ok` with the transaction in `ApplyFailed` when it stopped part
//! way: a cancel observed between entries, or one entry whose re-proof,
//! preflight or move failed. Earlier entries may already be in quarantine.
//! Blue used to word every `Ok` as "moved to quarantine" and settle Activity
//! as Finished (Night Watch S04-03 / S06-05); this module makes the shown
//! outcome agree with the receipt, and History's own wording.
use super::activity::Settled;
use super::backend::DuplicateRepairRecord;
use archivefs_core::dat::rename_apply::model::{EntryState, RenameTransaction, TransactionState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QuarantineOutcome {
    /// Every entry was moved.
    Moved { count: usize },
    /// Some entries were moved before the batch stopped; the rest stayed.
    StoppedPartway {
        applied: usize,
        total: usize,
        reason: Option<String>,
        cancelled: bool,
    },
    /// Nothing was moved.
    NothingMoved {
        reason: Option<String>,
        cancelled: bool,
    },
}

pub(super) fn classify(transaction: &RenameTransaction, cancelled: bool) -> QuarantineOutcome {
    let applied = transaction.applied_count();
    let total = transaction.entries.len();
    let reason = transaction
        .entries
        .iter()
        .find(|entry| entry.state != EntryState::Applied)
        .and_then(|entry| entry.failure_reason.clone());
    match transaction.state {
        TransactionState::Applied | TransactionState::Finalised if applied == total => {
            QuarantineOutcome::Moved { count: applied }
        }
        _ if applied == 0 => QuarantineOutcome::NothingMoved { reason, cancelled },
        _ => QuarantineOutcome::StoppedPartway {
            applied,
            total,
            reason,
            cancelled,
        },
    }
}

impl QuarantineOutcome {
    pub(super) fn is_complete(&self) -> bool {
        matches!(self, QuarantineOutcome::Moved { .. })
    }

    /// The sentence shown on the page that started the Apply.
    pub(super) fn message(&self) -> String {
        match self {
            QuarantineOutcome::Moved { count } => format!(
                "Moved {count} file(s) to EmuWiz quarantine. Nothing was deleted, and this change can be undone from History."
            ),
            QuarantineOutcome::StoppedPartway {
                applied,
                total,
                reason,
                cancelled,
            } => {
                let why = if *cancelled {
                    "you stopped it".to_string()
                } else {
                    reason
                        .clone()
                        .unwrap_or_else(|| "a file no longer matched its check".into())
                };
                format!(
                    "Stopped part way: moved {applied} of {total} file(s) to EmuWiz quarantine, then stopped because {why}. Nothing was deleted. The moved files can be put back from History; the others were not touched."
                )
            }
            QuarantineOutcome::NothingMoved { reason, cancelled } => {
                let why = if *cancelled {
                    "you stopped it".to_string()
                } else {
                    reason
                        .clone()
                        .unwrap_or_else(|| "a file no longer matched its check".into())
                };
                format!("Nothing was moved: {why}. Check the duplicates again before retrying.")
            }
        }
    }

    /// How Activity should settle the job that produced this receipt.
    pub(super) fn settled(&self) -> Settled {
        match self {
            QuarantineOutcome::Moved { .. } => Settled::Done,
            QuarantineOutcome::StoppedPartway {
                cancelled: true, ..
            }
            | QuarantineOutcome::NothingMoved {
                cancelled: true, ..
            } => Settled::Stopped,
            other => Settled::Failed(other.message()),
        }
    }
}

impl super::App {
    /// Settles an exact-duplicate quarantine Apply from its receipt.
    pub(super) fn duplicate_applied(&mut self, id: u64, record: DuplicateRepairRecord) {
        let cancelled = self.job_was_cancelled(id);
        let outcome = classify(&record.transaction, cancelled);
        self.repair_job = None;
        self.repair_history.push(record);
        self.repair_preview = None;
        self.repair_confirm = false;
        // The duplicate report and Problems summary described files that have
        // now moved (or were proven not to match); both must be rebuilt
        // before another preview is offered (Night Watch S06-07).
        self.duplicate_report = None;
        self.problem_summary = None;
        self.problem_selected = None;
        self.repair_result = Some(outcome.message());
        self.activity
            .settle(id, outcome.message(), outcome.settled());
    }

    fn job_was_cancelled(&self, id: u64) -> bool {
        self.activity
            .jobs
            .get(&id)
            .and_then(|job| job.cancel.as_ref())
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use archivefs_core::dat::rename_apply::model::{ObjectIdentity, ObjectKind, TransactionEntry};
    use std::path::PathBuf;

    fn entry(file: &str, state: EntryState, reason: Option<&str>) -> TransactionEntry {
        TransactionEntry {
            source_path: PathBuf::from(format!("/games/{file}")),
            destination_path: PathBuf::from(format!("/games/.emuwiz-quarantine/{file}")),
            original_basename: file.into(),
            proposed_basename: file.into(),
            identity: ObjectIdentity {
                size_bytes: 1,
                modified_unix: 1,
                kind: ObjectKind::RegularFile,
                ino: 1,
                dev: 1,
                freshness: None,
            },
            operation: Default::default(),
            preflight_passed: true,
            preflight_failures: Vec::new(),
            state,
            failure_reason: reason.map(str::to_string),
            applied_at_unix: (state == EntryState::Applied).then_some(1),
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }
    }

    pub(in crate::gui_v2) fn transaction(
        state: TransactionState,
        entries: Vec<TransactionEntry>,
    ) -> RenameTransaction {
        RenameTransaction {
            transaction_id: "dup-test".into(),
            plan_generation: 1,
            classifier_version: None,
            created_at_unix: 1,
            source_scan_root: "/games".into(),
            state,
            entries,
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    pub(in crate::gui_v2) fn partial_failure() -> RenameTransaction {
        transaction(
            TransactionState::ApplyFailed,
            vec![
                entry("a.bin", EntryState::Applied, None),
                entry(
                    "b.bin",
                    EntryState::ApplyFailed,
                    Some("'/games/b.bin' changed since the check"),
                ),
                entry("c.bin", EntryState::Planned, None),
            ],
        )
    }

    #[test]
    fn a_fully_applied_receipt_is_a_move() {
        let t = transaction(
            TransactionState::Applied,
            vec![
                entry("a.bin", EntryState::Applied, None),
                entry("b.bin", EntryState::Applied, None),
            ],
        );
        let outcome = classify(&t, false);
        assert_eq!(outcome, QuarantineOutcome::Moved { count: 2 });
        assert!(outcome.is_complete());
        assert!(matches!(outcome.settled(), Settled::Done));
        assert!(outcome.message().starts_with("Moved 2 file(s)"));
    }

    #[test]
    fn a_partial_failure_names_the_counts_and_the_reason() {
        let outcome = classify(&partial_failure(), false);
        assert_eq!(
            outcome,
            QuarantineOutcome::StoppedPartway {
                applied: 1,
                total: 3,
                reason: Some("'/games/b.bin' changed since the check".into()),
                cancelled: false,
            }
        );
        let message = outcome.message();
        assert!(message.contains("moved 1 of 3"), "{message}");
        assert!(message.contains("changed since the check"), "{message}");
        assert!(!message.starts_with("Moved"), "{message}");
        assert!(matches!(outcome.settled(), Settled::Failed(_)));
    }

    #[test]
    fn a_failure_before_any_move_says_nothing_was_moved() {
        let t = transaction(
            TransactionState::ApplyFailed,
            vec![entry(
                "a.bin",
                EntryState::ApplyFailed,
                Some("destination exists"),
            )],
        );
        let outcome = classify(&t, false);
        assert!(matches!(outcome, QuarantineOutcome::NothingMoved { .. }));
        assert!(
            outcome
                .message()
                .starts_with("Nothing was moved: destination exists")
        );
        assert!(matches!(outcome.settled(), Settled::Failed(_)));
    }

    #[test]
    fn a_cancel_between_entries_is_stopped_not_failed() {
        let t = transaction(
            TransactionState::ApplyFailed,
            vec![
                entry("a.bin", EntryState::Applied, None),
                entry("b.bin", EntryState::Planned, None),
            ],
        );
        let outcome = classify(&t, true);
        assert!(matches!(outcome.settled(), Settled::Stopped));
        assert!(outcome.message().contains("you stopped it"));
    }

    #[test]
    fn an_applied_state_with_an_unapplied_entry_is_not_trusted_as_complete() {
        // A receipt whose state and entries disagree never reads as "Moved".
        let t = transaction(
            TransactionState::Applied,
            vec![
                entry("a.bin", EntryState::Applied, None),
                entry("b.bin", EntryState::Planned, None),
            ],
        );
        assert!(!classify(&t, false).is_complete());
    }
}
