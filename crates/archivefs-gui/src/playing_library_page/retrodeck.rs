//! Durable RetroDECK receipt selection, rollback and recovery. Rendering is pure.
use super::*;
use archivefs_core::launch::es_de_publish::operation::{self, OperationOutcome, Phase};

impl PlayingLibraryPageState {
    pub(crate) fn rediscover_retrodeck_operations(&mut self) {
        let (operations, errors) = operation::discover(&self.journal_dir);
        self.retrodeck_operations = operations;
        self.retrodeck_discovery_errors = errors;
    }

    fn accept_retrodeck_outcome(&mut self, outcome: OperationOutcome) {
        self.retrodeck_phase = Some(outcome.phase);
        self.retrodeck_error = outcome.detail.clone();
        self.retrodeck_selected_id = Some(outcome.transaction.transaction_id.clone());
        self.retrodeck_applied = outcome
            .transaction
            .entries
            .iter()
            .any(|e| e.applied_at_unix.is_some())
            .then(|| outcome.transaction.clone());
        self.retrodeck_operations
            .retain(|p| p.transaction.transaction_id != outcome.transaction.transaction_id);
        self.retrodeck_operations.push(outcome);
    }

    pub(super) fn apply_retrodeck_reviewed(&mut self, mut transaction: RenameTransaction) {
        let projection = self.retrodeck_projection.as_ref().unwrap();
        if !projection.visibility.is_verified() || projection.es_de_publication.refused.len() > 0 {
            self.retrodeck_error = Some(
                "RetroDECK Apply blocked: visibility or gamelist compatibility needs review".into(),
            );
            self.retrodeck_pending_apply = false;
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&projection.retrodeck_rom_root)
            .and_then(|_| std::fs::create_dir_all(&self.journal_dir))
        {
            self.retrodeck_error = Some(e.to_string());
            self.retrodeck_pending_apply = false;
            return;
        }
        if let Err(e) = operation::prepare(&self.journal_dir, &mut transaction) {
            self.retrodeck_error = Some(e.to_string());
            self.retrodeck_pending_apply = false;
            return;
        }
        let result = operation::apply(
            &projection.es_de_publication,
            &mut ApplyExecution {
                approved_paths: transaction
                    .entries
                    .iter()
                    .map(|e| e.source_path.to_string_lossy().into_owned())
                    .collect(),
                transaction: &mut transaction,
                current_generation: self.plan_generation,
                trusted: TrustedRoots::from_paths([
                    projection.retrodeck_rom_root.clone(),
                    self.source_root_draft_path(),
                ]),
                journal_dir: self.journal_dir.clone(),
                hard_conflict_mode: HardConflictMode::AbortAll,
                cancel: &AtomicBool::new(false),
                directory_policy: DirectoryPolicy::SameFilesystem,
                allow_symlink_source: false,
            },
        );
        self.retrodeck_pending_apply = false;
        self.retrodeck_confirm_text.clear();
        match result {
            Ok(outcome) => self.accept_retrodeck_outcome(outcome),
            Err(error) => {
                if let Ok(outcome) =
                    operation::inspect(&self.journal_dir, &transaction.transaction_id)
                {
                    self.accept_retrodeck_outcome(outcome);
                }
                self.retrodeck_error = Some(format!(
                    "RetroDECK did not complete: {error}. Preserve the receipts and review recovery."
                ));
            }
        }
        self.rediscover_retrodeck_operations();
    }

    pub(crate) fn rollback_retrodeck(&mut self) {
        let Some(id) = self.retrodeck_selected_id.clone() else {
            self.retrodeck_error =
                Some("Recovery needs review: select the original combined RetroDECK receipt; legacy link-only evidence cannot authorize metadata rollback.".into());
            return;
        };
        match operation::restore(&self.journal_dir, &id, &AtomicBool::new(false)) {
            Ok(outcome) => self.accept_retrodeck_outcome(outcome),
            Err(error) => self.retrodeck_error = Some(format!("Recovery needs review: {error}")),
        }
        self.rediscover_retrodeck_operations();
    }
}

pub(super) fn show_operations(
    ui: &mut egui::Ui,
    state: &mut PlayingLibraryPageState,
    action: &mut Option<PlayingLibraryPageAction>,
) {
    if state.retrodeck_operations.is_empty() && state.retrodeck_discovery_errors.is_empty() {
        return;
    }
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "RetroDECK History and Recovery",
            Some(
                "Durable receipts keep links and gamelist metadata together. Originals stay untouched.",
            ),
        );
        let mut selected = None;
        for outcome in &state.retrodeck_operations {
            let label = format!(
                "{} · {:?}",
                outcome.transaction.transaction_id, outcome.phase
            );
            if ui
                .selectable_label(
                    state.retrodeck_selected_id.as_deref()
                        == Some(outcome.transaction.transaction_id.as_str()),
                    label,
                )
                .clicked()
            {
                selected = Some(outcome.clone());
            }
        }
        if let Some(outcome) = selected {
            state.accept_retrodeck_outcome(outcome);
        }
        for error in &state.retrodeck_discovery_errors {
            ui.colored_label(
                theme::warning(ui),
                format!("Recovery evidence needs review: {error}"),
            );
        }
        match state.retrodeck_phase {
            Some(Phase::Completed) => {
                ui.colored_label(theme::success(ui),"RetroDECK library created; links and game list committed. Undo is available from this receipt.");
            }
            Some(Phase::Restored) => {
                ui.colored_label(theme::success(ui),"RetroDECK library rolled back; previous game list restored and generated links removed.");
            }
            Some(_) => {
                ui.colored_label(theme::warning(ui),"RetroDECK recovery required: the combined operation did not complete. Review this receipt before restoring.");
            }
            None => {}
        }
        if let Some(error) = &state.retrodeck_error {
            ui.colored_label(theme::warning(ui), error);
        }
        if state.retrodeck_phase.is_some_and(|p| p != Phase::Restored) {
            let label = if state.retrodeck_phase == Some(Phase::Completed) {
                "Roll back RetroDECK Library"
            } else {
                "Recover RetroDECK Library"
            };
            if widgets::action_button(ui, label, widgets::ActionStyle::Quiet, !state.async_busy)
                .clicked()
            {
                *action = Some(PlayingLibraryPageAction::RollbackRetroDeck);
            }
        }
    });
}
