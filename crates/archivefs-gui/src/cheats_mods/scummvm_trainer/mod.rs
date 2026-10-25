//! Blue's ScummVM trainer controls over the existing core transaction engine.
mod backend;
#[cfg(test)]
mod tests;

use archivefs_core::patch_manager::{
    ScummVmTrainerOption, ScummVmTrainerSelection, ScummVmTrainerValue, SharedApplyResult,
    SharedApplyStatus, SharedRollbackPreview, SharedRollbackResult,
};
use backend::{Prepared, Receipt, Roots};
use eframe::egui;
use std::result::Result;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

const LAUNCH_NOTE: &str = "Saves an EmuWiz-owned trainer profile only. Saving does not launch ScummVM or activate trainers. To use it, choose this saved profile when launching the game from the Playing Library. Game files and saves are not modified.";

/// Profile section name and the file a launch of this game would load; used by
/// the Playing Library chooser so the two never disagree about the location.
pub(crate) const PROFILE_TARGET: &str = backend::TARGET_NAME;

pub(crate) fn owned_profile_path(folder: &Path, id: &str) -> Result<PathBuf, String> {
    Ok(Roots::managed()?.profile(folder, id))
}

enum Completed {
    Checked(Prepared),
    Preview(Prepared),
    Applied(SharedApplyResult),
    History(Roots, Vec<Receipt>, Vec<String>),
    UndoPreview(Roots, SharedRollbackPreview),
    Undone(SharedRollbackResult),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Confirmation {
    Apply,
    Undo,
}

#[derive(Default)]
pub(crate) struct State {
    folder: Option<PathBuf>,
    identity: Option<String>,
    options: Vec<ScummVmTrainerOption>,
    selections: Vec<ScummVmTrainerSelection>,
    prepared: Option<Prepared>,
    pending: Option<Receiver<Result<Completed, String>>>,
    confirmation: Option<Confirmation>,
    focus_cancel: bool,
    status: Option<String>,
    history_loaded: bool,
    receipts: Vec<Receipt>,
    problems: Vec<String>,
    history_roots: Option<Roots>,
    undo: Option<(Roots, SharedRollbackPreview)>,
}

impl State {
    fn start(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce() -> Result<Completed, String> + Send + 'static,
    ) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    pub(crate) fn poll(&mut self) {
        let Some(rx) = &self.pending else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Trainer worker stopped. Review durable History/recovery before retrying any change.".into()),
        };
        self.pending = None;
        match result {
            Err(error) => self.status = Some(format!("Refused or incomplete: {error}")),
            Ok(Completed::Checked(prepared)) => {
                self.identity = Some(prepared.identity.game_id.clone());
                self.options = prepared.preview.options;
                self.selections = self
                    .options
                    .iter()
                    .map(|o| ScummVmTrainerSelection {
                        key: o.key.clone(),
                        value: o.current_value.clone(),
                    })
                    .collect();
                self.prepared = None;
                self.status = Some(
                    "Compatibility checked. Choose options, then Preview; nothing has been saved."
                        .into(),
                );
            }
            Ok(Completed::Preview(prepared)) => {
                self.prepared = Some(prepared);
                self.status = Some("Preview ready; nothing has been saved.".into());
            }
            Ok(Completed::Applied(result)) => {
                let mut status = format!("Apply outcome: {:?}.", result.journal.status);
                if result.journal.status == SharedApplyStatus::Success
                    && result.journal_failure.is_none()
                {
                    status.push_str(
                        " Trainer profile saved; trainers are not activated by this action.",
                    );
                } else {
                    status.push_str(" Do not assume the profile was saved or unchanged. Review the receipt and recovery records before retrying.");
                }
                if let Some(path) = result.journal_path {
                    status.push_str(&format!(" History receipt: {}.", path.display()));
                }
                if let Some(error) = result.journal_failure {
                    status.push_str(&format!(" Receipt problem: {}.", error.detail));
                }
                for entry in &result.journal.entries {
                    for error in &entry.failures {
                        status.push_str(&format!(" {}.", error.detail));
                    }
                }
                self.status = Some(status);
                self.history_loaded = false;
                self.undo = None;
            }
            Ok(Completed::History(roots, receipts, problems)) => {
                self.history_roots = Some(roots);
                self.receipts = receipts;
                self.problems = problems;
            }
            Ok(Completed::UndoPreview(roots, preview)) => {
                self.undo = Some((roots, preview));
            }
            Ok(Completed::Undone(result)) => {
                self.status = Some(format!(
                    "Undo outcome: {:?}. {}",
                    result.status,
                    if result.status == SharedApplyStatus::Success {
                        "The owned profile change was undone."
                    } else {
                        "Undo was refused or incomplete. Review recovery records before retrying."
                    }
                ));
                self.history_loaded = false;
                self.undo = None;
                self.prepared = None;
                // The values cached before Undo are no longer authoritative.
                self.identity = None;
                self.options.clear();
                self.selections.clear();
            }
        }
    }

    fn invalidate_preview(&mut self) {
        self.prepared = None;
        self.confirmation = None;
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, folder: &Path, title: &str) -> bool {
        self.poll();
        if self.folder.as_deref() != Some(folder) {
            if self.pending.is_some() {
                ui.spinner();
                ui.label("Finishing the trainer operation for the previously selected game. Its receipt remains in History.");
                return false;
            }
            self.folder = Some(folder.to_path_buf());
            self.identity = None;
            self.options.clear();
            self.selections.clear();
            self.invalidate_preview();
            self.undo = None;
            self.status = None;
        }
        ui.heading("ScummVM trainer options");
        ui.label(title);
        ui.label(LAUNCH_NOTE);
        ui.label("Only the documented Hypno engine options are supported. A title, filename or DAT match alone cannot enable Apply.");
        let busy = self.pending.is_some();
        if busy {
            ui.spinner();
            ui.label("Checking or recording the requested operation…");
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Check trainer compatibility"))
            .clicked()
        {
            self.invalidate_preview();
            self.identity = None;
            self.options.clear();
            self.selections.clear();
            let folder = folder.to_path_buf();
            let roots = Roots::managed();
            self.start(ui.ctx(), move || {
                backend::prepare(&folder, None, vec![], roots?).map(Completed::Checked)
            });
        }
        let mut changed = false;
        ui.add_enabled_ui(!busy, |ui| {
            if let Some(id) = &self.identity {
                ui.label(format!("Detected game: {id}"));
            }
            for (option, selection) in self.options.iter().zip(&mut self.selections) {
                ui.push_id(&option.key, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&option.label).on_hover_text(&option.description);
                        changed |= ui
                            .selectable_value(&mut selection.value, None, "Engine default")
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut selection.value,
                                Some(ScummVmTrainerValue::Boolean(true)),
                                "On",
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut selection.value,
                                Some(ScummVmTrainerValue::Boolean(false)),
                                "Off",
                            )
                            .changed();
                    });
                });
            }
        });
        if changed {
            self.invalidate_preview();
        }
        if ui
            .add_enabled(
                !busy && self.identity.is_some(),
                egui::Button::new("Preview trainer profile"),
            )
            .clicked()
        {
            self.invalidate_preview();
            let folder = folder.to_path_buf();
            let expected = self.identity.clone();
            let selections = self.selections.clone();
            let roots = Roots::managed();
            self.start(ui.ctx(), move || {
                backend::prepare(&folder, expected.as_deref(), selections, roots?)
                    .map(Completed::Preview)
            });
        }
        if let Some(prepared) = &self.prepared {
            ui.separator();
            ui.strong("Preview — no changes saved");
            ui.label(format!("Exact game: {}", prepared.identity.game_id));
            ui.monospace(prepared.identity.game_folder.display().to_string());
            ui.label("Only this owned configuration will be written:");
            ui.monospace(prepared.preview.configuration_path.display().to_string());
            for entry in &prepared.preview.transaction_plan.entries {
                ui.label(format!(
                    "{:?}; backup required: {}",
                    entry.proposed_action, entry.backup_required
                ));
            }
            for selection in &self.selections {
                ui.label(format!("{}: {:?}", selection.key, selection.value));
            }
            ui.label("Replacing an existing profile requires a verified backup. Apply rechecks detection, staged bytes and the existing destination; stale or unsafe inputs are refused.");
            if ui
                .add_enabled(!busy, egui::Button::new("Review Apply…"))
                .clicked()
            {
                self.confirmation = Some(Confirmation::Apply);
                self.focus_cancel = true;
            }
        }
        let open_history = ui.button("Open History & Undo").clicked();
        self.show_history(ui, Some(folder));
        open_history
    }

    pub(crate) fn prepare_history(&mut self, ctx: &egui::Context, folder: Option<&Path>) -> bool {
        self.poll();
        if !self.history_loaded && self.pending.is_none() {
            self.history_loaded = true;
            self.undo = None;
            let roots = Roots::managed();
            self.start(ctx, move || {
                let roots = roots?;
                let (receipts, problems) = backend::history(&roots);
                Ok(Completed::History(roots, receipts, problems))
            });
        }
        self.pending.is_some()
            || !self.problems.is_empty()
            || self.receipts.iter().any(|r| {
                folder.is_none_or(|folder| {
                    r.journal
                        .context
                        .selected_archive
                        .to_path_buf()
                        .ok()
                        .as_deref()
                        == Some(folder)
                })
            })
    }

    pub(crate) fn show_history(&mut self, ui: &mut egui::Ui, folder: Option<&Path>) {
        self.poll();
        if let Some(folder) = folder {
            if self
                .prepared
                .as_ref()
                .is_some_and(|p| p.identity.game_folder != folder)
            {
                self.invalidate_preview();
            }
            if self.undo.as_ref().is_some_and(|(_, preview)| {
                !self.receipts.iter().any(|r| {
                    r.journal.operation_id == preview.original_operation_id
                        && r.journal
                            .context
                            .selected_archive
                            .to_path_buf()
                            .ok()
                            .as_deref()
                            == Some(folder)
                })
            }) {
                self.undo = None;
                self.confirmation = None;
            }
        }
        ui.separator();
        ui.strong("ScummVM trainer History & Undo");
        if ui
            .add_enabled(
                self.pending.is_none(),
                egui::Button::new("Refresh trainer receipts"),
            )
            .clicked()
        {
            self.history_loaded = false;
            self.undo = None;
        }
        self.prepare_history(ui.ctx(), folder);
        for problem in &self.problems {
            ui.label(problem);
        }
        if self.pending.is_some() {
            ui.spinner();
        }
        let mut request = None;
        for (index, receipt) in self.receipts.iter().enumerate().filter(|(_, r)| {
            folder.is_none_or(|folder| {
                r.journal
                    .context
                    .selected_archive
                    .to_path_buf()
                    .ok()
                    .as_deref()
                    == Some(folder)
            })
        }) {
            ui.push_id(&receipt.journal.operation_id, |ui| {
                ui.label(format!(
                    "{} · {} · {:?}",
                    receipt.journal.operation_id,
                    receipt.journal.context.verified_game_identity,
                    receipt.journal.status
                ));
                ui.monospace(&receipt.journal.context.selected_archive.display);
                ui.monospace(receipt.journal_path.display().to_string());
                for entry in &receipt.journal.entries {
                    ui.label(format!("{:?}", entry.outcome));
                    for error in &entry.failures {
                        ui.label(&error.detail);
                    }
                }
                if ui
                    .add_enabled(self.pending.is_none(), egui::Button::new("Preview Undo"))
                    .clicked()
                {
                    request = Some(index);
                }
            });
        }
        if self.receipts.is_empty() && self.pending.is_none() {
            ui.label("No trainer receipts loaded. Interrupted operations belong in recovery records; absence here is not proof that no change occurred.");
        }
        if let (Some(index), Some(roots)) = (request, self.history_roots.clone()) {
            self.invalidate_preview();
            self.undo = None;
            let receipt = Receipt {
                journal_path: self.receipts[index].journal_path.clone(),
                journal: self.receipts[index].journal.clone(),
            };
            self.start(ui.ctx(), move || {
                backend::undo_preview(&receipt, &roots).map(|p| Completed::UndoPreview(roots, p))
            });
        }
        if let Some((_, preview)) = &self.undo {
            ui.strong(format!("Undo preview: {}", preview.original_operation_id));
            for entry in &preview.entries {
                ui.label(format!(
                    "{}: {:?}",
                    entry
                        .destination
                        .as_ref()
                        .map_or("Unknown destination", |p| p.display.as_str()),
                    entry.outcome
                ));
                if let Some(error) = &entry.failure {
                    ui.label(&error.detail);
                }
            }
            ui.label("Undo restores the verified backup or removes only the installed profile. Changed destinations or missing/changed backups refuse Undo.");
            if ui
                .add_enabled(
                    preview.available && self.pending.is_none(),
                    egui::Button::new("Review Undo…"),
                )
                .clicked()
            {
                self.confirmation = Some(Confirmation::Undo);
                self.focus_cancel = true;
            }
        }
        if let Some(status) = &self.status {
            ui.label(status);
        }
        self.show_confirmation(ui);
    }

    fn show_confirmation(&mut self, ui: &mut egui::Ui) {
        let Some(kind) = self.confirmation else {
            return;
        };
        let mut confirmed = false;
        let mut cancelled = false;
        let response = egui::Modal::new(egui::Id::new("scummvm_trainer_confirmation")).show(ui.ctx(), |ui| {
            ui.heading(if kind == Confirmation::Apply { "Apply this trainer profile?" } else { "Undo this trainer profile change?" });
            ui.label(LAUNCH_NOTE);
            if kind == Confirmation::Apply {
                if let Some(prepared) = &self.prepared {
                    ui.label(&prepared.identity.game_id);
                    ui.monospace(prepared.preview.configuration_path.display().to_string());
                    ui.label("Confirm writing this exact preview, including backed-up replacement of an existing profile. History records the result.");
                }
            } else if let Some((_, preview)) = &self.undo {
                ui.label(format!("Operation: {}", preview.original_operation_id));
                for entry in &preview.entries {
                    if let Some(destination) = &entry.destination { ui.monospace(&destination.display); }
                    ui.label(format!("{:?}", entry.outcome));
                }
            }
            ui.horizontal(|ui| {
                let cancel = ui.button("Cancel");
                if self.focus_cancel { cancel.request_focus(); self.focus_cancel = false; }
                cancelled = cancel.clicked();
                confirmed = ui.add_enabled(self.pending.is_none(), egui::Button::new("Confirm")).clicked();
            });
        });
        if response.should_close() || cancelled {
            self.confirmation = None;
            return;
        }
        if !confirmed {
            return;
        }
        self.confirmation = None;
        match kind {
            Confirmation::Apply => {
                if let Some(prepared) = self.prepared.take() {
                    self.start(ui.ctx(), move || {
                        backend::apply(prepared).map(Completed::Applied)
                    });
                }
            }
            Confirmation::Undo => {
                if let Some((roots, preview)) = self.undo.take() {
                    self.start(ui.ctx(), move || {
                        Ok(Completed::Undone(backend::undo(preview, roots)))
                    });
                }
            }
        }
    }
}
