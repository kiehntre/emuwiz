//! Undo for Publisher exports recovered from their sealed journals, so that
//! neither Preview plan nor a restart ends an export's rollback. Every gate and
//! every write lives in the core; this page only lists and reports.

use std::path::{Path, PathBuf};

use archivefs_core::dat::rename_apply::RollbackResult;
use archivefs_core::publisher_profile::recovery::{
    PublisherUndoFailure, RecoverableExport, abandon_publisher_links, availability_blocker,
    describe_undo_outcome, open_publisher_export_for_undo, recoverable_publisher_exports,
    rollback_publisher_export,
};

use super::{
    PublisherExecutionStage, PublisherJob, PublisherProfilePageState,
    destination_left_after_rollback, egui, widgets,
};

/// Shown above the typed confirmation for abandoning links.
pub(crate) const ABANDON_WARNING: &str = "WARNING: Abandoning these links is irreversible. The transaction will remain marked as failed, and any remaining links must be removed manually. No files will be deleted automatically.";

#[derive(Debug, Default)]
pub(crate) struct PublisherRecoveryState {
    loaded: bool,
    journal_dir: Option<PathBuf>,
    pub(super) exports: Vec<RecoverableExport>,
    /// What the last load could not show.
    pub(super) notice: Option<String>,
    pub(super) selected: Option<String>,
    pub(super) confirmation_text: String,
    pub(super) message: Option<String>,
    /// The export whose unavailable links the user is considering abandoning.
    pub(super) abandon_selected: Option<String>,
    pub(super) abandon_text: String,
}

impl PublisherRecoveryState {
    pub(crate) fn loaded(&self) -> bool {
        self.loaded
    }

    pub(super) fn note_journal_dir(&mut self, journal_dir: &Path) {
        self.journal_dir = Some(journal_dir.to_path_buf());
        self.loaded = false;
    }

    fn journal_dir(&self) -> Result<PathBuf, String> {
        match &self.journal_dir {
            Some(directory) => Ok(directory.clone()),
            None => archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
                .map_err(|error| format!("Cannot locate transaction journals: {error}.")),
        }
    }
}

fn recovery_phrase(export: &RecoverableExport) -> String {
    format!("ROLL BACK {} ITEMS", export.item_count)
}

impl PublisherProfilePageState {
    pub(crate) fn load_recoverable_exports(&mut self) {
        self.recovery.loaded = true;
        match self.recovery.journal_dir() {
            Ok(directory) => {
                let listing = recoverable_publisher_exports(&directory);
                self.recovery.exports = listing.exports;
                let mut notices = Vec::new();
                if let Some(error) = listing.unreadable {
                    notices.push(format!("Transaction journals could not be read: {error}."));
                }
                if listing.unverified > 0 {
                    notices.push(format!(
                        "{} export journal(s) could not be verified, so they are not listed and cannot be undone here.",
                        listing.unverified
                    ));
                }
                if listing.unparseable > 0 {
                    notices.push(format!(
                        "{} journal(s) could not be read, so hardlink exports cannot be undone here until they are repaired.",
                        listing.unparseable
                    ));
                }
                self.recovery.notice = (!notices.is_empty()).then(|| notices.join(" "));
            }
            Err(error) => {
                self.recovery.exports.clear();
                self.recovery.notice = Some(error);
            }
        }
        let exports = &self.recovery.exports;
        self.recovery
            .selected
            .take_if(|id| !exports.iter().any(|e| e.transaction.transaction_id == *id));
    }

    /// Whether the export this page holds may be let go: true when it has no
    /// rollback to lose, or its sealed journal proves the rollback survives. A
    /// rollback that cannot be recovered from disk is never dropped.
    pub(super) fn keep_rollback_durable(&mut self, refused: &str) -> bool {
        if self.rollback_confirmation_phrase().is_none() {
            return true;
        }
        let Some(transaction) = &self.execution_transaction else {
            return true;
        };
        let id = transaction.transaction.transaction_id.clone();
        let opened = self.recovery.journal_dir().and_then(|directory| {
            open_publisher_export_for_undo(&directory, &transaction.transaction)
        });
        match opened {
            Ok(_) => {
                self.load_recoverable_exports();
                self.recovery.message = Some(format!(
                    "Export {id} was not undone. It stays listed here until you roll it back."
                ));
                true
            }
            // A rollback now cannot finish; the links would be kept for a later
            // Undo, but trying only reports the same outage.
            Err(_) if availability_blocker(&transaction.transaction).is_some() => {
                self.execution_error = Some(format!(
                    "{refused}, because this export's folders are not available right now. Reconnect them and try again: a rollback now cannot finish. Nothing was changed."
                ));
                false
            }
            Err(reason) => {
                self.execution_error = Some(format!(
                    "{refused}, because this export's rollback could not be recovered later: {reason} Roll it back here first. Nothing was changed."
                ));
                false
            }
        }
    }

    pub(super) fn selected_recovery(&self) -> Option<&RecoverableExport> {
        let id = self.recovery.selected.as_ref()?;
        self.recovery
            .exports
            .iter()
            .find(|export| export.transaction.transaction_id == *id)
            .filter(|export| export.blocker.is_none())
    }

    fn abandon_target(&self) -> Option<&RecoverableExport> {
        let id = self.recovery.abandon_selected.as_ref()?;
        self.recovery
            .exports
            .iter()
            .find(|export| export.transaction.transaction_id == *id)
            .filter(|export| export.abandonable > 0)
    }

    pub(crate) fn abandon_phrase(&self) -> Option<String> {
        self.abandon_target()
            .map(|export| format!("ABANDON {} LINKS", export.abandonable))
    }

    pub(crate) fn can_abandon_recovered(&self) -> bool {
        self.abandon_phrase()
            .is_some_and(|phrase| phrase == self.recovery.abandon_text)
    }

    /// Records the user's decision not to undo links whose folders are gone.
    /// Nothing is deleted or moved, and the export is never shown as rolled back.
    pub(crate) fn abandon_recovered(&mut self) {
        self.recovery.message = None;
        let (export, directory) = match (self.abandon_target(), self.recovery.journal_dir()) {
            (_, Err(error)) => {
                self.recovery.message = Some(error);
                return;
            }
            (Some(export), Ok(directory)) if self.can_abandon_recovered() => {
                (export.clone(), directory)
            }
            _ => {
                self.recovery.message =
                    Some("Type the exact abandon confirmation phrase first.".into());
                return;
            }
        };
        let id = export.transaction.transaction_id.clone();
        let message = match abandon_publisher_links(&directory, &export.transaction) {
            Ok(record) => {
                let abandoned = |entry: &&archivefs_core::dat::rename_apply::TransactionEntry| {
                    entry.unknown.contains_key("emuwiz_undo_abandoned_v1")
                };
                let count = record.entries.iter().filter(abandoned).count();
                let quarantined: Vec<String> = record
                    .entries
                    .iter()
                    .filter(abandoned)
                    .filter_map(|entry| {
                        let dir = entry
                            .unknown
                            .get("emuwiz_hl_quarantine_v1")?
                            .get("dir")?
                            .as_str()?;
                        Some(format!(
                            "{}/{dir}",
                            entry.destination_path.parent()?.display()
                        ))
                    })
                    .collect();
                let quarantine_note = if quarantined.is_empty() {
                    String::new()
                } else {
                    format!(
                        " {} of them may already sit in a private, unlisted quarantine folder ({}); it is hidden and has no read permission, so grant yourself read access to look inside.",
                        quarantined.len(),
                        quarantined.join(", ")
                    )
                };
                format!(
                    "{count} link(s) of {id} were abandoned at your request. They were left in place, not undone and not deleted, and the decision is recorded in the journal. The export is NOT rolled back.{quarantine_note}"
                )
            }
            Err(reason) => format!("{reason} Nothing was changed for {id}."),
        };
        self.recovery.abandon_selected = None;
        self.recovery.abandon_text.clear();
        self.load_recoverable_exports();
        self.recovery.message = Some(message);
    }

    pub(crate) fn can_rollback_recovered(&self) -> bool {
        self.selected_recovery()
            .is_some_and(|export| recovery_phrase(export) == self.recovery.confirmation_text)
    }

    pub(crate) fn rollback_recovered(&mut self) {
        self.recovery.message = None;
        let (export, directory) = match (self.selected_recovery(), self.recovery.journal_dir()) {
            (Some(export), Ok(directory)) if self.can_rollback_recovered() => {
                (export.clone(), directory)
            }
            (_, Err(error)) => {
                self.recovery.message = Some(error);
                return;
            }
            _ => {
                self.recovery.message =
                    Some("Type the exact rollback confirmation phrase first.".into());
                return;
            }
        };
        let id = export.transaction.transaction_id.clone();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        self.recovery.message = Some(
            match rollback_publisher_export(&directory, &export.transaction, &cancel) {
                Ok((record, outcome))
                    if matches!(outcome.rollback.result, RollbackResult::FullyRolledBack) =>
                {
                    let remaining = destination_left_after_rollback(&record, &outcome);
                    if remaining.is_empty() {
                        format!(
                            "Export {id} rolled back. Source files and pre-existing destination data remain untouched."
                        )
                    } else {
                        format!(
                            "Rollback finished, but these destination paths remain: {}. Review transaction {id}.",
                            remaining.join(", ")
                        )
                    }
                }
                Ok((record, outcome)) => describe_undo_outcome(&record, &outcome),
                Err(PublisherUndoFailure::Refused(reason)) => {
                    format!("{reason} Nothing was rolled back for {id}.")
                }
                Err(PublisherUndoFailure::Interrupted(reason)) => format!(
                    "Rollback needs reconciliation: {reason} Some links may already be removed. Review transaction {id}."
                ),
            },
        );
        self.recovery.selected = None;
        self.recovery.confirmation_text.clear();
        let message = self.recovery.message.take();
        self.load_recoverable_exports();
        self.recovery.message = message;
    }
}

/// Lists exports recovered from sealed journals. Rendered ahead of the preview
/// so it is reachable after a restart, when no preview exists.
pub(super) fn show_recoverable_exports(ui: &mut egui::Ui, state: &mut PublisherProfilePageState) {
    let current = state
        .execution_transaction
        .as_ref()
        .filter(|_| {
            !matches!(
                state.execution_stage,
                PublisherExecutionStage::Preview | PublisherExecutionStage::Review
            )
        })
        .map(|transaction| transaction.transaction.transaction_id.clone());
    let exports: Vec<RecoverableExport> = state
        .recovery
        .exports
        .iter()
        .filter(|export| Some(&export.transaction.transaction_id) != current.as_ref())
        .cloned()
        .collect();
    if exports.is_empty() && state.recovery.message.is_none() && state.recovery.notice.is_none() {
        return;
    }
    let mut request_rollback = false;
    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Exports you can still undo").strong());
        ui.label("Listed from sealed transaction journals. Exports made before this version are not listed and cannot be undone here.");
        if let Some(notice) = &state.recovery.notice {
            ui.label(notice);
        }
        if let Some(message) = &state.recovery.message {
            ui.label(message);
        }
        if (state.recovery.notice.is_some()
            || exports.iter().any(|export| export.blocker.is_some()))
            && ui.button("Check again").clicked()
        {
            state.recovery.loaded = false;
        }
        for export in &exports {
            let id = &export.transaction.transaction_id;
            ui.label(format!(
                "Transaction {id}: {} item(s) in {}",
                export.item_count, export.transaction.source_scan_root
            ));
            if export.abandonable > 0 {
                ui.label(format!(
                    "{} link(s) cannot be reached because a folder or device is unavailable. Reconnect it and choose Check again. If it is gone for good, you can abandon those links: they are left in place, not undone and not deleted.",
                    export.abandonable
                ));
                if ui
                    .button(format!("Review abandoning links of {id}"))
                    .clicked()
                {
                    state.recovery.abandon_selected = Some(id.clone());
                    state.recovery.abandon_text.clear();
                }
            }
            if let Some(blocker) = &export.blocker {
                ui.label(blocker);
            } else if ui.button(format!("Choose {id}")).clicked() {
                state.recovery.selected = Some(id.clone());
                state.recovery.confirmation_text.clear();
            }
        }
        if let Some(phrase) = state.abandon_phrase() {
            ui.colored_label(ui.visuals().warn_fg_color, ABANDON_WARNING);
            ui.label(format!(
                "Abandoning records that these links will not be undone. Nothing is deleted or moved, the export is not marked rolled back, and the links must be removed by hand. Type {phrase} to confirm."
            ));
            ui.text_edit_singleline(&mut state.recovery.abandon_text);
            if ui
                .add_enabled(
                    state.can_abandon_recovered(),
                    egui::Button::new("Abandon these links"),
                )
                .clicked()
            {
                state.abandon_recovered();
            }
        }
        if let Some(phrase) = state.selected_recovery().map(recovery_phrase) {
            ui.label(format!(
                "Type {phrase} to remove this export's destination entries."
            ));
            ui.text_edit_singleline(&mut state.recovery.confirmation_text);
            if ui
                .add_enabled(
                    state.can_rollback_recovered(),
                    egui::Button::new("Roll back this export"),
                )
                .clicked()
            {
                request_rollback = true;
            }
        }
    });
    if request_rollback {
        state.start(PublisherJob::RollbackRecovered, ui.ctx().clone());
    }
}
