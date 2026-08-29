//! The Repair History page.
//!
//! Answers, for every rename transaction EmuWiz has ever journaled through
//! [`archivefs_core::dat::rename_apply`] (which the Repair Center's executor
//! and journal reuse verbatim - see [`archivefs_core::repair::execute`]):
//! what changed, whether it was verified, whether anything was rolled back,
//! and whether it can still be safely undone.
//!
//! # No second rollback implementation
//!
//! This module never calls `std::fs::rename`, `std::fs::remove_file`, or
//! `std::fs::copy`, and never re-implements identity checking, no-clobber
//! renaming, or crash reconciliation. Every read comes from
//! [`archivefs_core::dat::rename_apply::list_journals`] (re-read from disk on
//! every refresh, never trusted from a prior in-memory copy) and every
//! mutation goes through the exact same
//! [`archivefs_core::dat::rename_apply::rollback_transaction`] the DAT
//! Sources page's own recovery UI already uses. "Can I undo this safely?" is
//! answered by [`archivefs_core::dat::rename_apply::RenameTransaction::is_rollbackable`],
//! a core predicate rather than a GUI guess, and every fail-closed check
//! (changed identity, an occupied original path, an ambiguous in-flight
//! entry) is enforced inside `rollback_transaction` itself; this page only
//! renders whatever it reports.
//!
//! # Every journal in the directory, not just Repair Center's
//!
//! The journal format carries no "which feature produced this" tag - a
//! transaction from Repair Center's `apply_saved_plan_selected` and one from
//! the DAT Sources page's own rename apply are the same
//! [`RenameTransaction`] type in the same directory. This page shows all of
//! them truthfully rather than inventing a provenance field to filter by.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, TryRecvError};

use archivefs_core::dat::rename_apply::{
    EntryState, RenameTransaction, RollbackOutcome, RollbackResult, TransactionSummary,
    journal_path, list_journals, read_journal, reconcile_recovery, remove_journal,
    rollback_transaction,
};
use archivefs_core::repair::execute::{
    RepairReverifyEntry, RepairReverifyOutcome, reverify_transaction,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// One pending "Undo" confirmation, frozen when the dialog opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UndoConfirmation {
    pub(crate) transaction_id: String,
    pub(crate) applied_count: usize,
}

/// The terminal message the background undo worker sends back.
enum UndoMessage {
    /// `rollback_transaction` returned - which does not by itself mean full
    /// success; see [`RollbackOutcome::result`].
    Done(Box<RollbackOutcome>),
    /// `rollback_transaction` itself could not complete (e.g. the journal
    /// could not be written).
    Failed(String),
}

/// The running background undo job.
struct UndoJob {
    messages: Receiver<UndoMessage>,
}

/// The page's authoritative state.
pub(crate) struct RepairHistoryPageState {
    /// Where transaction journals live. Never the saved/loaded snapshot in
    /// `transactions` - every refresh re-reads this directory.
    journal_dir: PathBuf,
    /// The journal directory's transactions, most recent first. Reloaded
    /// wholesale by [`Self::refresh`]; never mutated piecemeal in memory.
    pub(crate) transactions: Vec<RenameTransaction>,
    /// Journals that exist but could not be parsed. Surfaced, never hidden or
    /// silently dropped.
    pub(crate) load_problems: Vec<String>,
    /// The transaction id whose details panel is open, if any.
    pub(crate) details_id: Option<String>,
    /// A pending "Undo" confirmation, frozen when the dialog opens.
    pub(crate) undo_confirm: Option<UndoConfirmation>,
    undo_confirm_focus_cancel: bool,
    undo_job: Option<UndoJob>,
    pub(crate) undo_running: bool,
    /// The last undo attempt's result, when the worker actually ran.
    pub(crate) undo_outcome: Option<RollbackOutcome>,
    /// The last undo attempt's error, when the worker itself could not
    /// complete (distinct from a reported partial/failed rollback, which
    /// lands in `undo_outcome` instead - see [`RollbackResult`]).
    pub(crate) undo_error: Option<String>,
    /// Set when the user asked to clear completed history; holds the exact
    /// transaction ids that will be removed, frozen at the moment the
    /// confirmation opened so a concurrent refresh can never silently widen
    /// what gets deleted.
    pub(crate) clear_confirm: Option<Vec<String>>,
    /// The outcome of the last "Clear completed history" run: how many
    /// journals were actually removed, and any that failed to delete.
    pub(crate) clear_outcome: Option<ClearHistoryOutcome>,
    /// When true (the default), transactions with nothing left to undo -
    /// [`RenameTransaction::is_rollbackable`] is false - are hidden from the
    /// list. Purely a display filter: `transactions` itself is never
    /// touched, so "Clear completed history" and "Undo" keep working on
    /// hidden rows exactly as before.
    pub(crate) hide_settled: bool,
    /// Free-text filter matched against the transaction id and each entry's
    /// original/proposed basenames. Empty matches everything.
    pub(crate) search_query: String,
}

/// The result of removing every provably-safe-to-remove transaction's
/// journal.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClearHistoryOutcome {
    pub(crate) removed: usize,
    pub(crate) failed: Vec<String>,
}

impl RepairHistoryPageState {
    /// Loads history from the default journal directory (the same one the
    /// Repair Center executor and the DAT Sources page's rollback both use).
    pub(crate) fn load() -> Self {
        let journal_dir = archivefs_core::dat::rename_apply::default_rename_transaction_dir()
            .unwrap_or_else(|_| PathBuf::from("rename-transactions"));
        Self::load_with_journal_dir(journal_dir)
    }

    /// Loads history from an explicit journal directory. Exists so tests
    /// (and any future caller that already knows the directory) never
    /// depend on resolving the real user data directory.
    pub(crate) fn load_with_journal_dir(journal_dir: PathBuf) -> Self {
        let mut state = Self {
            journal_dir,
            transactions: Vec::new(),
            load_problems: Vec::new(),
            details_id: None,
            undo_confirm: None,
            undo_confirm_focus_cancel: false,
            undo_job: None,
            undo_running: false,
            undo_outcome: None,
            undo_error: None,
            clear_confirm: None,
            clear_outcome: None,
            hide_settled: true,
            search_query: String::new(),
        };
        state.refresh();
        state
    }

    /// Re-reads every journal from disk. Never relies on the in-memory
    /// `transactions` from a prior call - the whole point of "history" is
    /// that it reflects what actually happened, including from another
    /// process or a previous session.
    ///
    /// Any transaction left with an in-flight (`Applying`/`RollingBack`)
    /// entry by a crash is reconciled against the filesystem first, via the
    /// exact same read-only-to-files [`reconcile_recovery`] the DAT Sources
    /// page's own recovery pass already calls - never a second
    /// implementation of that classification.
    pub(crate) fn refresh(&mut self) {
        let (mut transactions, problems) = list_journals(&self.journal_dir);
        for transaction in &mut transactions {
            let needs_reconciliation = transaction
                .entries
                .iter()
                .any(|entry| matches!(entry.state, EntryState::Applying | EntryState::RollingBack));
            if needs_reconciliation {
                let _ = reconcile_recovery(transaction, &self.journal_dir);
            }
        }
        transactions.sort_by(|a, b| {
            b.created_at_unix
                .cmp(&a.created_at_unix)
                .then_with(|| b.transaction_id.cmp(&a.transaction_id))
        });
        self.transactions = transactions;
        self.load_problems = problems;
    }

    pub(crate) fn transaction_by_id(&self, transaction_id: &str) -> Option<&RenameTransaction> {
        self.transactions
            .iter()
            .find(|transaction| transaction.transaction_id == transaction_id)
    }

    pub(crate) fn set_details(&mut self, transaction_id: Option<String>) {
        self.details_id = transaction_id;
    }

    /// Whether "Undo" may be invoked right now for this transaction: it is
    /// still in the loaded history, the core proves it is reversible
    /// ([`RenameTransaction::is_rollbackable`]), and no undo is already
    /// running.
    pub(crate) fn can_undo(&self, transaction_id: &str) -> bool {
        !self.undo_running
            && self
                .transaction_by_id(transaction_id)
                .is_some_and(RenameTransaction::is_rollbackable)
    }

    /// Whether a background undo job is in flight.
    pub(crate) fn is_undo_running(&self) -> bool {
        self.undo_job.is_some()
    }

    /// Opens the confirmation dialog, freezing the transaction id and its
    /// applied count. A no-op when `can_undo` does not hold for it.
    pub(crate) fn open_undo_confirmation(&mut self, transaction_id: &str) {
        if !self.can_undo(transaction_id) {
            return;
        }
        let Some(transaction) = self.transaction_by_id(transaction_id) else {
            return;
        };
        self.undo_confirm = Some(UndoConfirmation {
            transaction_id: transaction.transaction_id.clone(),
            applied_count: transaction.applied_count(),
        });
        self.undo_confirm_focus_cancel = true;
    }

    /// Dismisses the confirmation dialog without undoing anything.
    pub(crate) fn cancel_undo_confirmation(&mut self) {
        self.undo_confirm = None;
    }

    /// Confirms the pending undo: spawns the background worker for exactly
    /// the frozen transaction id, then closes the dialog.
    pub(crate) fn confirm_undo(&mut self) {
        let Some(confirmation) = self.undo_confirm.take() else {
            return;
        };
        if self.undo_running {
            return;
        }
        self.spawn_undo(confirmation.transaction_id);
    }

    /// Spawns the background undo worker. The GUI never mutates the
    /// filesystem itself: this re-reads the journal fresh from disk (never
    /// trusting the in-memory row a stale frame might still be showing) and
    /// hands it to [`rollback_transaction`] on a dedicated thread, relaying
    /// only the result back.
    fn spawn_undo(&mut self, transaction_id: String) {
        if self.undo_job.is_some() {
            return;
        }
        let journal_dir = self.journal_dir.clone();
        let Some(path) = journal_path(&journal_dir, &transaction_id) else {
            self.undo_error = Some(format!(
                "transaction id '{transaction_id}' cannot name a journal file"
            ));
            return;
        };
        let mut transaction = match read_journal(&path) {
            Ok(transaction) => transaction,
            Err(error) => {
                self.undo_error = Some(format!("journal unreadable: {error}"));
                return;
            }
        };

        let (sender, messages) = std::sync::mpsc::channel();
        // Never exposed to cancellation in this slice; still required by
        // `rollback_transaction`'s signature.
        let cancel = AtomicBool::new(false);

        std::thread::spawn(move || {
            let result = rollback_transaction(&mut transaction, &journal_dir, &cancel);
            let message = match result {
                Ok(outcome) => UndoMessage::Done(Box::new(outcome)),
                Err(error) => UndoMessage::Failed(error),
            };
            let _ = sender.send(message);
        });

        self.undo_job = Some(UndoJob { messages });
        self.undo_running = true;
        self.undo_outcome = None;
        self.undo_error = None;
    }

    /// Drains the background undo job's channel, if one is running. Returns
    /// whether anything changed (so the caller can request a repaint).
    ///
    /// History is refreshed from disk whenever the job settles, regardless
    /// of the outcome: even a partial or failed rollback attempt durably
    /// updates the journal, so the safest default is to always reflect
    /// exactly what is on disk afterward.
    pub(crate) fn poll_undo(&mut self) -> bool {
        let Some(job) = self.undo_job.as_mut() else {
            return false;
        };
        match job.messages.try_recv() {
            Ok(message) => {
                self.undo_job = None;
                self.undo_running = false;
                match message {
                    UndoMessage::Done(outcome) => {
                        self.undo_outcome = Some(*outcome);
                        self.undo_error = None;
                    }
                    UndoMessage::Failed(error) => {
                        self.undo_error = Some(error);
                        self.undo_outcome = None;
                    }
                }
                self.refresh();
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.undo_job = None;
                self.undo_running = false;
                true
            }
        }
    }

    /// The transactions "Clear completed history" would remove: every one
    /// [`RenameTransaction::is_rollbackable`] reports as *not* rollbackable
    /// - the exact same core predicate `can_undo` already trusts, just
    /// inverted. This never touches a transaction that still has applied
    /// entries awaiting rollback, or one left interrupted by a crash and
    /// needing recovery; both remain `is_rollbackable() == true` and are
    /// never included here (2026-08-22, live-QA Phase 8).
    pub(crate) fn clearable_transaction_ids(&self) -> Vec<String> {
        self.transactions
            .iter()
            .filter(|transaction| !transaction.is_rollbackable())
            .map(|transaction| transaction.transaction_id.clone())
            .collect()
    }

    /// Opens the "Clear completed history" confirmation, freezing exactly
    /// which transaction ids will be removed. A no-op when there is nothing
    /// clearable.
    pub(crate) fn open_clear_confirmation(&mut self) {
        let ids = self.clearable_transaction_ids();
        if ids.is_empty() {
            return;
        }
        self.clear_confirm = Some(ids);
    }

    /// Dismisses the "Clear completed history" confirmation without
    /// removing anything.
    pub(crate) fn cancel_clear_confirmation(&mut self) {
        self.clear_confirm = None;
    }

    /// Removes exactly the frozen set of journals from the confirmation,
    /// re-checking each one is still not rollbackable immediately before
    /// deleting it (in case something changed the on-disk state since the
    /// dialog opened - e.g. another process). Always refreshes from disk
    /// afterward.
    pub(crate) fn confirm_clear(&mut self) {
        let Some(ids) = self.clear_confirm.take() else {
            return;
        };
        let mut outcome = ClearHistoryOutcome::default();
        for transaction_id in &ids {
            let still_safe = self
                .transaction_by_id(transaction_id)
                .is_none_or(|transaction| !transaction.is_rollbackable());
            if !still_safe {
                outcome
                    .failed
                    .push(format!("{transaction_id}: no longer safe to remove"));
                continue;
            }
            match remove_journal(&self.journal_dir, transaction_id) {
                Ok(()) => outcome.removed += 1,
                Err(error) => outcome.failed.push(format!("{transaction_id}: {error}")),
            }
        }
        self.clear_outcome = Some(outcome);
        self.refresh();
    }
}

// ---------------------------------------------------------------------------
// Filtering (display-only - never mutates `transactions`)
// ---------------------------------------------------------------------------

/// Whether a transaction has nothing left to undo, by the exact same
/// predicate [`RepairHistoryPageState::can_undo`] and "Clear completed
/// history" already trust.
fn is_settled(transaction: &RenameTransaction) -> bool {
    !transaction.is_rollbackable()
}

/// Whether `query` (already expected lowercase, empty meaning "match
/// everything") appears in the transaction id or any entry's basenames.
/// Never inspects full paths, so this stays cheap even for a transaction
/// with many entries.
fn transaction_matches_search(transaction: &RenameTransaction, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    if transaction
        .transaction_id
        .to_lowercase()
        .contains(query_lower)
    {
        return true;
    }
    transaction.entries.iter().any(|entry| {
        entry.original_basename.to_lowercase().contains(query_lower)
            || entry.proposed_basename.to_lowercase().contains(query_lower)
    })
}

/// The transaction ids to actually draw, in their existing (most-recent
/// first) order, after applying "hide settled" and the search box. A pure
/// read over `transactions` - it never reorders or mutates the underlying
/// list, so Undo/Details/Clear keep acting on the real data regardless of
/// what the filter currently hides.
fn visible_transaction_ids(
    transactions: &[RenameTransaction],
    hide_settled: bool,
    search_query: &str,
) -> Vec<String> {
    let query_lower = search_query.to_lowercase();
    transactions
        .iter()
        .filter(|transaction| !(hide_settled && is_settled(transaction)))
        .filter(|transaction| transaction_matches_search(transaction, &query_lower))
        .map(|transaction| transaction.transaction_id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// What [`reverify_transaction`] found for a transaction's still-applied
/// entries, in wording that cannot be misread as "the original source is
/// gone, which is expected after a rename". `reverify_transaction` only
/// ever inspects each entry's *destination* - the file the rename
/// produced - never the source, so "missing" here always means the
/// destination itself could not be found after having been marked
/// `Applied`: a genuine post-apply problem, never the normal, expected
/// disappearance of the pre-rename source path.
struct ReverifySummary {
    /// The one-line headline, safe to show without further context.
    headline: String,
    /// Present only when there is something to explain beyond the
    /// headline (a missing or changed destination); spells out that this
    /// is about the destination and is not a normal rename outcome.
    explanation: Option<String>,
    tone: widgets::StatusTone,
}

fn reverify_summary(entries: &[RepairReverifyEntry]) -> ReverifySummary {
    if entries.is_empty() {
        return ReverifySummary {
            headline: "Not applicable".to_string(),
            explanation: Some(
                "No entries in this transaction are currently applied, so there is nothing to \
                 re-check right now."
                    .to_string(),
            ),
            tone: widgets::StatusTone::Info,
        };
    }
    let total = entries.len();
    let verified = entries
        .iter()
        .filter(|entry| entry.outcome == RepairReverifyOutcome::Verified)
        .count();
    let missing = entries
        .iter()
        .filter(|entry| entry.outcome == RepairReverifyOutcome::Missing)
        .count();
    let changed = entries
        .iter()
        .filter(|entry| entry.outcome == RepairReverifyOutcome::Changed)
        .count();

    if verified == total {
        return ReverifySummary {
            headline: format!("{verified} of {total} destination file(s) verified"),
            explanation: None,
            tone: widgets::StatusTone::Success,
        };
    }

    let mut explanation_parts = Vec::new();
    if missing > 0 {
        explanation_parts.push(format!(
            "{missing} destination file(s) could not be found. This is not the pre-rename \
             source path (which is expected to be gone) - it is the file the rename itself \
             created, and its absence means the repair did not hold."
        ));
    }
    if changed > 0 {
        explanation_parts.push(format!(
            "{changed} destination file(s) have changed since the rename and no longer match \
             what was recorded."
        ));
    }
    ReverifySummary {
        headline: format!("{verified} of {total} destination file(s) verified"),
        explanation: Some(explanation_parts.join(" ")),
        tone: widgets::StatusTone::Blocked,
    }
}

/// The badge text for one entry's reverify outcome. Always names
/// "destination", never bare "missing"/"changed", so a single entry read in
/// isolation cannot be misread as describing the pre-rename source.
fn reverify_outcome_badge_label(outcome: RepairReverifyOutcome) -> &'static str {
    match outcome {
        RepairReverifyOutcome::Verified => "destination verified",
        RepairReverifyOutcome::Missing => "destination missing",
        RepairReverifyOutcome::Changed => "destination changed",
    }
}

/// The card's headline: the file(s) this transaction actually changed,
/// named directly rather than left behind an opaque id and counts. A
/// single-entry transaction names that one file; a multi-entry transaction
/// names the first entry and counts the rest - "Details" reveals every one.
fn transaction_headline(transaction: &RenameTransaction) -> String {
    let Some(first) = transaction.entries.first() else {
        return "No entries recorded".to_string();
    };
    let headline = format!("{} → {}", first.original_basename, first.proposed_basename);
    match transaction.entries.len() - 1 {
        0 => headline,
        remaining => format!("{headline}  ·  + {remaining} more"),
    }
}

/// Full source/destination paths for the headline's hover text - the
/// basenames shown on the card are the readable summary; the full paths
/// are one hover (or "Details") away, never lost.
fn transaction_headline_hover(transaction: &RenameTransaction) -> Option<String> {
    let first = transaction.entries.first()?;
    Some(format!(
        "{} → {}",
        first.source_path.display(),
        first.destination_path.display()
    ))
}

fn rollback_status_tone(summary: &TransactionSummary) -> widgets::StatusTone {
    match summary.rollback {
        archivefs_core::dat::rename_apply::RollbackStatus::NotRequested => {
            widgets::StatusTone::Info
        }
        archivefs_core::dat::rename_apply::RollbackStatus::FullyRolledBack => {
            widgets::StatusTone::Success
        }
        archivefs_core::dat::rename_apply::RollbackStatus::PartiallyRolledBack => {
            widgets::StatusTone::Warning
        }
        archivefs_core::dat::rename_apply::RollbackStatus::RollbackFailed => {
            widgets::StatusTone::Blocked
        }
    }
}

/// Draws the page.
pub(crate) fn show_repair_history_page(
    ui: &mut egui::Ui,
    state: &mut RepairHistoryPageState,
    clipboard: &mut dyn crate::ClipboardBackend,
) {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::HISTORY,
        "Repair History",
        "What EmuWiz changed on disk through the Repair Center, whether it was verified, and \
         whether it can still be undone.",
    );

    show_undo_confirmation_dialog(ui, state);
    show_clear_confirmation_dialog(ui, state);

    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                state.refresh();
            }
            let clearable = state.clearable_transaction_ids();
            if widgets::action_button(
                ui,
                "Clear completed history",
                widgets::ActionStyle::Secondary,
                !clearable.is_empty(),
            )
            .on_hover_text(
                "Removes only transactions with nothing left to undo - already rolled back, or \
                 never actually changed anything on disk. A transaction with applied changes \
                 still awaiting rollback is never touched.",
            )
            .clicked()
            {
                state.open_clear_confirmation();
            }
            ui.label(
                egui::RichText::new(format!("{} transaction(s)", state.transactions.len()))
                    .color(theme::muted(ui)),
            );
        });
        ui.label(
            egui::RichText::new(
                "Always read fresh from the transaction journal on disk, never only from this \
                 session's memory.",
            )
            .color(theme::muted(ui)),
        );
    });

    if let Some(outcome) = &state.clear_outcome {
        ui.add_space(6.0);
        if outcome.failed.is_empty() {
            widgets::banner(
                ui,
                "History cleared",
                &format!(
                    "{} transaction{} with nothing left to undo {} removed.",
                    outcome.removed,
                    if outcome.removed == 1 { "" } else { "s" },
                    if outcome.removed == 1 { "was" } else { "were" }
                ),
                widgets::StatusTone::Success,
            );
        } else {
            widgets::failure_summary(
                ui,
                "clear_history_result",
                "Some history could not be cleared",
                Some("Anything not removed is left exactly as it was."),
                &outcome.failed.join("\n"),
            );
        }
    }

    if !state.load_problems.is_empty() {
        ui.add_space(6.0);
        widgets::banner(
            ui,
            "Some journals could not be read",
            &format!(
                "{} journal file(s) could not be parsed. They are left on disk untouched; other \
                 history remains available.",
                state.load_problems.len()
            ),
            widgets::StatusTone::Warning,
        );
    }

    show_undo_result(ui, state);

    ui.add_space(8.0);
    if state.transactions.is_empty() {
        widgets::empty_state(
            ui,
            "No repair transactions found",
            "Nothing has been journaled in this directory yet.",
            None,
        );
        return;
    }

    // Filtering only earns its keep once there is actually a list to sift
    // through; a handful of transactions reads fine without it.
    if state.transactions.len() > 5 {
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Search:");
                ui.add(
                    egui::TextEdit::singleline(&mut state.search_query)
                        .hint_text("file name or transaction id")
                        .desired_width(220.0),
                );
                ui.checkbox(
                    &mut state.hide_settled,
                    "Hide completed (nothing left to undo)",
                );
            });
        });
    }

    let ids = visible_transaction_ids(&state.transactions, state.hide_settled, &state.search_query);
    if ids.len() != state.transactions.len() {
        ui.label(
            egui::RichText::new(format!(
                "Showing {} of {} transaction(s).",
                ids.len(),
                state.transactions.len()
            ))
            .color(theme::muted(ui))
            .small(),
        );
    }
    if ids.is_empty() {
        widgets::empty_state(
            ui,
            "No matching transactions",
            "Nothing matches the current search and filter. Clear them to see everything.",
            None,
        );
        return;
    }
    for transaction_id in &ids {
        let Some(transaction) = state.transaction_by_id(transaction_id) else {
            continue;
        };
        let transaction = transaction.clone();
        show_transaction_row(ui, &transaction, state, clipboard);
    }
}

fn show_transaction_row(
    ui: &mut egui::Ui,
    transaction: &RenameTransaction,
    state: &mut RepairHistoryPageState,
    clipboard: &mut dyn crate::ClipboardBackend,
) {
    let summary = TransactionSummary::from_transaction(transaction);
    let reverify = reverify_transaction(transaction);
    let reverify_summary = reverify_summary(&reverify);

    ui.add_space(6.0);
    widgets::card(ui, |ui| {
        // Primary line: what actually changed - source/destination - not
        // the transaction id. A single-entry transaction names its one
        // file directly; a multi-entry one names the first and counts the
        // rest ("Details" below reveals every entry).
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                transaction.state.label(),
                rollback_status_tone(&summary),
            );
            let headline_label = ui.label(
                egui::RichText::new(transaction_headline(transaction))
                    .strong()
                    .size(15.0),
            );
            if let Some(hover) = transaction_headline_hover(transaction) {
                headline_label.on_hover_text(hover);
            }
        });

        // Secondary technical metadata: transaction id and timestamp, kept
        // small and muted rather than leading the card.
        ui.label(
            egui::RichText::new(format!(
                "{} · {}",
                transaction.transaction_id,
                archivefs_core::format_unix_timestamp_utc(transaction.created_at_unix as i64)
            ))
            .small()
            .monospace()
            .color(theme::muted(ui)),
        );

        ui.label(
            egui::RichText::new(format!(
                "Requested {} · Applied {} · Failed {} · Skipped {}",
                summary.requested, summary.applied, summary.failed, summary.skipped
            ))
            .small()
            .color(theme::muted(ui)),
        );
        ui.label(
            egui::RichText::new(format!("Rollback: {}", summary.rollback.label()))
                .small()
                .color(theme::muted(ui)),
        );
        // The status badge above stays exactly what `state` truthfully says
        // (never rewritten to "Applied" or anything else) - this line adds
        // the user's own decision about the recovery prompt alongside it,
        // never in place of it.
        if let Some(resolution) = transaction.recovery_resolution {
            ui.label(
                egui::RichText::new(resolution.label())
                    .small()
                    .color(theme::SUCCESS),
            );
        }

        // Reverify: the headline never says just "missing" - see
        // `reverify_summary`'s doc comment for why that reads as
        // ambiguous in a destructive/undo-capable history UI.
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Reverify:").small());
            widgets::status_badge(ui, &reverify_summary.headline, reverify_summary.tone);
        });
        if let Some(explanation) = &reverify_summary.explanation {
            ui.label(
                egui::RichText::new(explanation)
                    .small()
                    .color(theme::muted(ui)),
            );
        }

        ui.horizontal(|ui| {
            let details_open = state.details_id.as_deref() == Some(&transaction.transaction_id);
            if widgets::action_button(
                ui,
                if details_open {
                    "Hide details"
                } else {
                    "Details"
                },
                widgets::ActionStyle::Quiet,
                true,
            )
            .clicked()
            {
                state.set_details(if details_open {
                    None
                } else {
                    Some(transaction.transaction_id.clone())
                });
            }

            let undoable = state.can_undo(&transaction.transaction_id);
            let undo = widgets::action_button(
                ui,
                if state.undo_running {
                    "Undoing…"
                } else {
                    "Undo"
                },
                widgets::ActionStyle::Destructive,
                undoable,
            );
            let undo_clicked = undo.clicked();
            if !undoable {
                let hover = if state.undo_running {
                    "An undo is already running."
                } else {
                    "This transaction cannot be safely reversed (nothing applied, or it was \
                     already rolled back)."
                };
                undo.on_disabled_hover_text(hover);
            }
            if undo_clicked {
                state.open_undo_confirmation(&transaction.transaction_id);
            }
        });

        if state.details_id.as_deref() == Some(transaction.transaction_id.as_str()) {
            ui.separator();
            show_transaction_details(ui, transaction, &reverify, clipboard);
        }
    });
}

/// The details panel for one transaction: every entry's full source and
/// destination path (wrapped, never truncated, each with its own Copy
/// button), final state, reverify result, and any recorded failure/recovery
/// evidence. Concise technical detail (generation, classifier version) is
/// collapsed rather than presented as the primary UI.
fn show_transaction_details(
    ui: &mut egui::Ui,
    transaction: &RenameTransaction,
    reverify: &[RepairReverifyEntry],
    clipboard: &mut dyn crate::ClipboardBackend,
) {
    for entry in &transaction.entries {
        ui.add_space(4.0);
        let tone = match entry.state {
            EntryState::Applied => widgets::StatusTone::Success,
            EntryState::RolledBack => widgets::StatusTone::Info,
            EntryState::Skipped => widgets::StatusTone::Pending,
            EntryState::ApplyFailed | EntryState::RollbackFailed => widgets::StatusTone::Blocked,
            EntryState::Planned | EntryState::PreflightPassed | EntryState::Applying => {
                widgets::StatusTone::Pending
            }
            EntryState::RollingBack => widgets::StatusTone::Warning,
        };
        widgets::status_badge(ui, entry.state.label(), tone);
        // Full paths, wrapped rather than truncated: this is the panel
        // where "which exact file" must be fully readable, with a Copy
        // button for pasting into a shell/file manager.
        detail_path_row(ui, clipboard, "Source", &entry.source_path);
        detail_path_row(ui, clipboard, "Destination", &entry.destination_path);
        if let Some(reverify_entry) = reverify
            .iter()
            .find(|candidate| candidate.source_path == entry.source_path)
        {
            let tone = match reverify_entry.outcome {
                RepairReverifyOutcome::Verified => widgets::StatusTone::Success,
                RepairReverifyOutcome::Missing | RepairReverifyOutcome::Changed => {
                    widgets::StatusTone::Blocked
                }
            };
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                widgets::status_badge(
                    ui,
                    reverify_outcome_badge_label(reverify_entry.outcome),
                    tone,
                );
                ui.label(
                    egui::RichText::new(&reverify_entry.detail)
                        .small()
                        .color(theme::muted(ui)),
                );
            });
        }
        if let Some(reason) = &entry.failure_reason {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label(
                    egui::RichText::new(format!("Recovery/failure evidence: {reason}"))
                        .small()
                        .color(theme::muted(ui)),
                );
            });
        }
    }

    ui.collapsing("Technical details", |ui| {
        detail_label(
            ui,
            "Plan generation",
            &transaction.plan_generation.to_string(),
        );
        detail_label(
            ui,
            "Classifier version",
            transaction
                .classifier_version
                .as_deref()
                .unwrap_or("(none)"),
        );
        detail_label(ui, "Source scan root", &transaction.source_scan_root);
    });
}

/// One fully readable, copyable path row for the details panel: the label,
/// the whole path wrapped (never `.truncate()`d - this is the one place a
/// user needs to read the exact path without hovering), and a Copy button.
/// Deliberately not [`widgets::path_value`]/[`widgets::copyable_value`],
/// which truncate with hover text - the right shape for a compact summary
/// row, not for "source/destination must be fully readable" in Details.
fn detail_path_row(
    ui: &mut egui::Ui,
    clipboard: &mut dyn crate::ClipboardBackend,
    label: &str,
    path: &std::path::Path,
) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(24.0);
        ui.add_sized(
            [88.0, 0.0],
            egui::Label::new(egui::RichText::new(label).strong().small()),
        );
        let text = path.display().to_string();
        ui.add(egui::Label::new(egui::RichText::new(&text).monospace().small()).wrap());
        if widgets::action_button(ui, "Copy", widgets::ActionStyle::Quiet, true).clicked() {
            let _ = clipboard.set_text(text);
        }
    });
}

/// The "Undo" confirmation dialog. A no-op draw when nothing is pending.
/// Cancel is favoured: its button claims focus the first frame the dialog
/// appears, and closing the window (Esc/✕) is wired to Cancel, not Undo.
fn show_undo_confirmation_dialog(ui: &mut egui::Ui, state: &mut RepairHistoryPageState) {
    let Some(confirmation) = state.undo_confirm.clone() else {
        return;
    };
    let mut focus_cancel = state.undo_confirm_focus_cancel;
    let mut cancel_clicked = false;
    let mut undo_clicked = false;
    let mut open = true;

    egui::Window::new("Undo this repair?")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Transaction {} · {} applied change(s)",
                    confirmation.transaction_id, confirmation.applied_count
                ))
                .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "This will rename the affected files back to their original paths on disk. \
                     Each file is re-verified against its recorded identity immediately before \
                     the reverse rename; if anything has changed, nothing is touched.",
                )
                .color(theme::muted(ui)),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let cancel = ui.add(egui::Button::new("Cancel"));
                if focus_cancel {
                    cancel.request_focus();
                    focus_cancel = false;
                }
                if cancel.clicked() {
                    cancel_clicked = true;
                }
                if ui
                    .add(egui::Button::new("Undo").fill(theme::DANGER))
                    .clicked()
                {
                    undo_clicked = true;
                }
            });
        });

    state.undo_confirm_focus_cancel = focus_cancel;
    if cancel_clicked || !open {
        state.cancel_undo_confirmation();
    } else if undo_clicked {
        state.confirm_undo();
    }
}

/// The "Clear completed history" confirmation dialog. A no-op draw when
/// nothing is pending. Names exactly which transactions will be removed -
/// the frozen ids from [`RepairHistoryPageState::open_clear_confirmation`] -
/// and states plainly what "safe to remove" means here, since "clear
/// history" could otherwise be misread as touching anything still
/// recoverable.
fn show_clear_confirmation_dialog(ui: &mut egui::Ui, state: &mut RepairHistoryPageState) {
    let Some(ids) = state.clear_confirm.clone() else {
        return;
    };
    let mut cancel_clicked = false;
    let mut clear_clicked = false;
    let mut open = true;

    egui::Window::new("Clear completed history?")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} transaction{} will be removed from history",
                    ids.len(),
                    if ids.len() == 1 { "" } else { "s" }
                ))
                .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Only transactions that were already rolled back, or that never actually \
                     changed anything on disk, are included. Nothing with an applied change \
                     still awaiting rollback, or left interrupted, is ever removed - this never \
                     touches ROM files themselves, only the history record.",
                )
                .color(theme::muted(ui)),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for id in &ids {
                        ui.label(egui::RichText::new(id).monospace().small());
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("Cancel")).clicked() {
                    cancel_clicked = true;
                }
                if ui
                    .add(egui::Button::new("Clear completed history").fill(theme::DANGER))
                    .clicked()
                {
                    clear_clicked = true;
                }
            });
        });

    if cancel_clicked || !open {
        state.cancel_clear_confirmation();
    } else if clear_clicked {
        state.confirm_clear();
    }
}

/// The last undo attempt's outcome, when there is one. Shown until
/// superseded by the next undo attempt or a manual refresh.
fn show_undo_result(ui: &mut egui::Ui, state: &RepairHistoryPageState) {
    if let Some(outcome) = &state.undo_outcome {
        ui.add_space(8.0);
        let (headline, tone) = match &outcome.result {
            RollbackResult::FullyRolledBack => ("Undo complete", widgets::StatusTone::Success),
            RollbackResult::PartiallyRolledBack { .. } => {
                ("Undo partially completed", widgets::StatusTone::Warning)
            }
            RollbackResult::RollbackFailed { .. } => ("Undo refused", widgets::StatusTone::Blocked),
        };
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::status_badge(ui, headline, tone);
                ui.label(egui::RichText::new(&outcome.transaction.transaction_id).monospace());
            });
            match &outcome.result {
                RollbackResult::FullyRolledBack => {
                    ui.label(format!(
                        "{} file(s) restored to their original path.",
                        outcome.transaction.rolled_back_count()
                    ));
                }
                RollbackResult::PartiallyRolledBack {
                    rolled_back,
                    failed,
                } => {
                    ui.label(format!(
                        "{} restored, {} not restored.",
                        rolled_back.len(),
                        failed.len()
                    ));
                    for (path, reason) in failed {
                        ui.label(
                            egui::RichText::new(format!("{}: {reason}", path.display()))
                                .small()
                                .color(theme::muted(ui)),
                        );
                    }
                }
                RollbackResult::RollbackFailed { failed } => {
                    for (path, reason) in failed {
                        ui.label(
                            egui::RichText::new(format!("{}: {reason}", path.display())).small(),
                        );
                    }
                }
            }
        });
    }
    if let Some(error) = &state.undo_error {
        ui.add_space(8.0);
        widgets::banner(ui, "Undo failed", error, widgets::StatusTone::Blocked);
    }
}

fn detail_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_sized(
            [140.0, 0.0],
            egui::Label::new(egui::RichText::new(label).strong()),
        );
        ui.add(egui::Label::new(value).wrap());
    });
}

#[cfg(test)]
mod tests;
