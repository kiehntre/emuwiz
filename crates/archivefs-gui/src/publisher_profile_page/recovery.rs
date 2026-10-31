//! Undo for Publisher exports recovered from their sealed journals, so that
//! neither Preview plan nor a restart ends an export's rollback. Every gate and
//! every write lives in the core; this page only lists and reports.

use std::path::{Path, PathBuf};

use archivefs_core::dat::rename_apply::RollbackResult;
use archivefs_core::publisher_profile::recovery::{
    PublisherUndoFailure, RecoverableExport, open_publisher_export_for_undo,
    recoverable_publisher_exports, rollback_publisher_export,
};

use super::{
    PublisherExecutionStage, PublisherJob, PublisherProfilePageState,
    destination_left_after_rollback, egui, widgets,
};

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
                Ok((_, outcome)) => format!(
                    "Rollback was not complete: {:?}. Review transaction {id}.",
                    outcome.rollback.result
                ),
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
            if let Some(blocker) = &export.blocker {
                ui.label(blocker);
            } else if ui.button(format!("Choose {id}")).clicked() {
                state.recovery.selected = Some(id.clone());
                state.recovery.confirmation_text.clear();
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
