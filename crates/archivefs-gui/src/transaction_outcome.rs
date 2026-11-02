//! One honest outcome badge for an Apply, read from the transaction journal.
//! Organisation, Playing Library and Publisher show the same badge, so a result
//! reads the same wherever it appears in Blue.
//!
//! Counts alone are not enough: a run that applied nothing may still have
//! created directories or left an entry in flight, and an Undo can end with
//! entries that were not reversed. The classifier therefore also reads the
//! journal: created directories, entries that block a full rollback, and
//! whether the journal proves a complete rollback.
//!
//! The badge never promises recovery. "Undo available" is raised only when the
//! page says it is offering Undo, which each page derives from the same
//! journal evidence that gates its Undo button.

use archivefs_core::dat::rename_apply::{RenameTransaction, TransactionEntry, TransactionSummary};
use eframe::egui;

use crate::ui::components::{self as widgets, StatusTone};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransactionOutcome {
    /// Every requested entry was applied; nothing skipped, failed or pending.
    Completed,
    /// Some entries were applied and others were skipped, failed or not attempted.
    Partial,
    /// Nothing was applied and the journal shows no possible change (all skipped,
    /// refused or empty).
    NothingChanged,
    /// Every applied entry was reversed and the journal proves it.
    Undone,
    /// Nothing was applied and at least one entry failed, with no possible change.
    Failed,
    /// The result is not settled: an entry is in flight, a rollback is incomplete,
    /// a directory was created, or an applied run cannot be rolled back.
    RecoveryRequired,
}

/// Whether the journal records a possible change to the destination: an entry
/// is applied or in flight, or the run created a directory.
pub(crate) fn destination_may_have_changed(transaction: &RenameTransaction) -> bool {
    !transaction.created_directories.is_empty()
        || transaction
            .entries
            .iter()
            .any(TransactionEntry::blocks_full_rollback)
}

impl TransactionOutcome {
    pub(crate) fn classify(summary: &TransactionSummary, transaction: &RenameTransaction) -> Self {
        if summary.unresolved > 0 {
            return Self::RecoveryRequired;
        }
        if summary.applied > 0 && summary.rolled_back > 0 {
            // Some entries were reversed and others were not: Undo is incomplete.
            return Self::RecoveryRequired;
        }
        if summary.applied > 0 {
            return if summary.failed > 0 || summary.pending > 0 || summary.skipped > 0 {
                Self::Partial
            } else if transaction.is_rollbackable() {
                Self::Completed
            } else {
                // Applied, yet the journal offers no rollback: not a clean success.
                Self::RecoveryRequired
            };
        }
        if summary.rolled_back > 0 {
            return if summary.failed == 0 && transaction.is_fully_rolled_back() {
                Self::Undone
            } else {
                // Partly reversed or reversed with leftovers: Undo is incomplete.
                Self::RecoveryRequired
            };
        }
        if destination_may_have_changed(transaction) {
            Self::RecoveryRequired
        } else if summary.failed > 0 {
            Self::Failed
        } else {
            Self::NothingChanged
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Partial => "Partially completed",
            Self::NothingChanged => "Nothing changed",
            Self::Undone => "Undone",
            Self::Failed => "Failed",
            Self::RecoveryRequired => "Recovery required",
        }
    }

    pub(crate) fn tone(self) -> StatusTone {
        match self {
            Self::Completed => StatusTone::Success,
            Self::Partial => StatusTone::Warning,
            Self::NothingChanged => StatusTone::Pending,
            Self::Undone => StatusTone::Info,
            Self::Failed | Self::RecoveryRequired => StatusTone::Blocked,
        }
    }

    /// Whether an "Undo available" cue can ever be honest for this outcome:
    /// there must be something recorded to reverse, or something to recover.
    fn can_offer_undo(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Partial | Self::RecoveryRequired
        )
    }
}

/// The counts that reconcile with the badge, e.g. `3 applied · 1 skipped`.
/// Zero buckets are omitted so the line stays short at 1024x600.
pub(crate) fn counts_line(summary: &TransactionSummary) -> String {
    let buckets = [
        (summary.applied, "applied"),
        (summary.skipped, "skipped"),
        (summary.failed, "failed"),
        (summary.pending, "not attempted"),
        (summary.unresolved, "in doubt"),
        (summary.rolled_back, "undone"),
    ];
    let parts: Vec<String> = buckets
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect();
    if parts.is_empty() {
        "0 requested".to_string()
    } else {
        parts.join(" · ")
    }
}

/// The compact badge row: outcome, its counts and the Undo/recovery cue.
/// `undo_offered` must be the page's own Undo-button condition.
pub(crate) fn show(ui: &mut egui::Ui, transaction: &RenameTransaction, undo_offered: bool) {
    let summary = TransactionSummary::from_transaction(transaction);
    let summary = &summary;
    let outcome = TransactionOutcome::classify(summary, transaction);
    ui.horizontal_wrapped(|ui| {
        widgets::status_badge(ui, outcome.label(), outcome.tone());
        ui.label(counts_line(summary));
        if undo_offered && outcome.can_offer_undo() {
            // "now": it describes the action this page offers at this moment, not a
            // recovery promise that outlives a new preview or a restart.
            widgets::status_badge(ui, "Undo available now", StatusTone::Info);
        } else if outcome == TransactionOutcome::RecoveryRequired {
            widgets::status_badge(ui, "Review recovery", StatusTone::Warning);
        }
    });
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use archivefs_core::dat::rename_apply::model::{
        EntryState, ObjectIdentity, ObjectKind, TransactionState,
    };
    use std::path::PathBuf;

    pub(crate) fn entry(state: EntryState) -> TransactionEntry {
        TransactionEntry {
            source_path: PathBuf::from("/lib/a"),
            destination_path: PathBuf::from("/out/a"),
            original_basename: "a".into(),
            proposed_basename: "a".into(),
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
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        }
    }

    pub(crate) fn journal(
        state: TransactionState,
        entries: &[EntryState],
        directories: &[&str],
    ) -> RenameTransaction {
        RenameTransaction {
            transaction_id: "t".into(),
            plan_generation: 1,
            classifier_version: None,
            created_at_unix: 0,
            source_scan_root: "/lib".into(),
            state,
            entries: entries.iter().copied().map(entry).collect(),
            created_directories: directories.iter().map(PathBuf::from).collect(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: Default::default(),
        }
    }

    /// Every text run drawn in one frame, in paint order.
    pub(crate) fn rendered_text(output: &egui::FullOutput) -> String {
        fn collect(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(nested) => nested.iter().for_each(|shape| collect(shape, out)),
                _ => {}
            }
        }
        let mut text = String::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::journal;
    use super::*;
    use archivefs_core::dat::rename_apply::model::{EntryState, TransactionState};

    fn outcome(t: &RenameTransaction) -> TransactionOutcome {
        TransactionOutcome::classify(&TransactionSummary::from_transaction(t), t)
    }

    #[test]
    fn counts_and_journal_give_one_honest_outcome() {
        use EntryState::*;
        use TransactionOutcome as O;
        use TransactionState as S;
        let cases = [
            (journal(S::Applied, &[Applied, Applied], &[]), O::Completed),
            (journal(S::Applied, &[Applied, Skipped], &[]), O::Partial),
            (
                journal(S::ApplyFailed, &[Applied, ApplyFailed], &[]),
                O::Partial,
            ),
            (
                journal(S::ApplyFailed, &[Skipped, Skipped], &[]),
                O::NothingChanged,
            ),
            (journal(S::ApplyFailed, &[], &[]), O::NothingChanged),
            (journal(S::ApplyFailed, &[ApplyFailed], &[]), O::Failed),
            (
                journal(S::RolledBack, &[RolledBack, RolledBack], &[]),
                O::Undone,
            ),
        ];
        for (transaction, expected) in &cases {
            assert_eq!(outcome(transaction), *expected, "{:?}", transaction.entries);
        }
    }

    #[test]
    fn an_all_skipped_run_that_created_a_directory_is_not_nothing_changed() {
        use EntryState::*;
        use TransactionState as S;
        let t = journal(S::ApplyFailed, &[Skipped, Skipped], &["/out/new-dir"]);
        assert_eq!(outcome(&t), TransactionOutcome::RecoveryRequired);
        let failed = journal(S::ApplyFailed, &[ApplyFailed], &["/out/new-dir"]);
        assert_eq!(outcome(&failed), TransactionOutcome::RecoveryRequired);
    }

    #[test]
    fn incomplete_undo_is_never_shown_as_undone() {
        use EntryState::*;
        use TransactionState as S;
        // Some entries reversed, one still applied.
        let partly = journal(S::RollbackFailed, &[RolledBack, Applied], &[]);
        assert_eq!(outcome(&partly), TransactionOutcome::RecoveryRequired);
        // Reversed entries but a rollback failure.
        let failed = journal(S::RollbackFailed, &[RolledBack, RollbackFailed], &[]);
        assert_eq!(outcome(&failed), TransactionOutcome::RecoveryRequired);
        // Transaction says RolledBack but an entry contradicts it.
        let contradicted = journal(S::RolledBack, &[RolledBack, RollingBack], &[]);
        assert_eq!(outcome(&contradicted), TransactionOutcome::RecoveryRequired);
        // In flight is never settled.
        let in_flight = journal(S::Applying, &[Applying], &[]);
        assert_eq!(outcome(&in_flight), TransactionOutcome::RecoveryRequired);
    }

    #[test]
    fn applied_but_not_rollbackable_is_not_a_clean_completion() {
        use EntryState::*;
        use TransactionState as S;
        // Undo surrendered (Finalising): applied entries, no rollback offered.
        let t = journal(S::Finalising, &[Applied], &[]);
        assert_eq!(outcome(&t), TransactionOutcome::RecoveryRequired);
    }

    #[test]
    fn only_completed_is_success_toned() {
        use TransactionOutcome::*;
        assert_eq!(Completed.tone(), StatusTone::Success);
        for other in [Partial, NothingChanged, Undone, Failed, RecoveryRequired] {
            assert_ne!(other.tone(), StatusTone::Success, "{}", other.label());
        }
    }

    #[test]
    fn undo_cue_needs_both_the_page_offer_and_a_recoverable_outcome() {
        use TransactionOutcome::*;
        for o in [NothingChanged, Undone, Failed] {
            assert!(!o.can_offer_undo(), "{}", o.label());
        }
        for o in [Completed, Partial, RecoveryRequired] {
            assert!(o.can_offer_undo(), "{}", o.label());
        }
    }

    #[test]
    fn counts_line_reconciles_and_omits_empty_buckets() {
        use EntryState::*;
        use TransactionState as S;
        let t = journal(S::Applied, &[Applied, Applied, Applied, Skipped], &[]);
        assert_eq!(
            counts_line(&TransactionSummary::from_transaction(&t)),
            "3 applied · 1 skipped"
        );
        let empty = journal(S::ApplyFailed, &[], &[]);
        assert_eq!(
            counts_line(&TransactionSummary::from_transaction(&empty)),
            "0 requested"
        );
    }

    fn row_size(blue: bool, t: &RenameTransaction, undo: bool) -> egui::Vec2 {
        let context = egui::Context::default();
        crate::ui::theme::apply(&context);
        if blue {
            context.style_mut(|style| {
                style.visuals.faint_bg_color = crate::ui::theme::blue::CARD_SURFACE
            });
        }
        let mut size = egui::Vec2::ZERO;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            ..egui::RawInput::default()
        };
        let _ = context.run(input, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let before = ui.cursor().min;
                show(ui, t, undo);
                size = egui::vec2(ui.min_rect().width(), ui.cursor().min.y - before.y);
            });
        });
        size
    }

    #[test]
    fn the_badge_row_stays_compact_at_1024_by_600_in_both_profiles() {
        use EntryState::*;
        use TransactionState as S;
        let rows = [
            (journal(S::Applied, &[Applied, Applied], &[]), true),
            (journal(S::Applied, &[Applied, Skipped], &[]), true),
            (journal(S::ApplyFailed, &[Skipped], &[]), false),
            (
                journal(
                    S::RollbackFailed,
                    &[RolledBack, RollbackFailed, Applied, Skipped],
                    &["/d"],
                ),
                true,
            ),
        ];
        for blue in [false, true] {
            for (t, undo) in &rows {
                let size = row_size(blue, t, *undo);
                assert!(size.x <= 1024.0, "row overflows (blue={blue}): {size:?}");
                assert!(size.y <= 56.0, "row too tall (blue={blue}): {size:?}");
            }
        }
    }
}
