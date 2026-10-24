//! Blue's bezel journal view. Gold owns loading, preview, confirmation and
//! execution; this module only selects records and dispatches its actions.

use archivefs_core::patch_manager::{
    PreviewAdapter, SHARED_APPLY_SCHEMA_VERSION, SharedApplyJournal, SharedApplyStatus,
    SharedContentVerification, SharedRollbackOutcome,
};
use eframe::egui;

use crate::{ArchiveFsApp, HistoryPageAction, MainView, SharedHistoryState, SharedRollbackState};

/// Keep the embedded coordinator's existing view-gated cancellation intact.
pub(super) fn prepare(app: &mut ArchiveFsApp, active: bool) {
    if active {
        if app.view != MainView::HistoryLogs {
            // An apply in Artwork uses the same durable store. Re-entering
            // History reuses Gold's loader instead of keeping an empty cache.
            app.shared_history = SharedHistoryState::NotLoaded;
        }
        app.view = MainView::HistoryLogs;
    } else if app.view == MainView::HistoryLogs {
        app.view = MainView::Home;
    }
}

fn in_scope(journal: &SharedApplyJournal, global: bool) -> bool {
    // Existing bezel journals record the source artwork in selected_archive,
    // not the launched game. Do not invent a game association from its name.
    journal.context.source_mode == "retroarch_bezel" && global
}

/// Eligibility is only presentation. Core's fresh preview and exact-preview
/// executor remain the authority on whether the recorded files can be undone.
fn supported(journal: &SharedApplyJournal) -> bool {
    journal.schema_version == SHARED_APPLY_SCHEMA_VERSION
        && journal.context.adapter == PreviewAdapter::RetroArch
        && journal.context.source_mode == "retroarch_bezel"
        && !journal.entries.is_empty()
        && journal.entries.iter().all(|entry| {
            entry.plan_entry.adapter == PreviewAdapter::RetroArch
                && entry.plan_entry.content_verification
                    == Some(SharedContentVerification::RetroArchBezel)
        })
}

fn preview_allowed(journal: &SharedApplyJournal) -> bool {
    supported(journal)
        && journal.status == SharedApplyStatus::Success
        && !journal.dry_run
        && journal.rollback_operation_id.is_none()
}

/// Prepare Gold's existing source before deciding whether Blue is empty.
/// A failed source is shown separately; it is not a known operation record.
pub(super) fn prepare_source(app: &mut ArchiveFsApp, ctx: &egui::Context, global: bool) -> bool {
    prepare(app, true);
    if matches!(app.shared_history, SharedHistoryState::NotLoaded) {
        app.refresh_shared_history(ctx.clone());
    }
    app.poll_shared_history();
    if !global
        && matches!(
            app.shared_rollback,
            SharedRollbackState::Previewing { .. }
                | SharedRollbackState::Review { .. }
                | SharedRollbackState::NativeReview { .. }
        )
    {
        app.shared_rollback = SharedRollbackState::Idle;
    }
    match &app.shared_history {
        SharedHistoryState::Ready(loaded) => loaded
            .report
            .journals
            .iter()
            .any(|(_, j)| in_scope(j, global)),
        SharedHistoryState::NotLoaded | SharedHistoryState::Loading { .. } => global,
        SharedHistoryState::Failed(_) => false,
    }
}

/// Gold retains all operation/rollback state; painting consumes the cached
/// source after the existing History sections have laid out their rows.
pub(super) fn show(ui: &mut egui::Ui, app: &mut ArchiveFsApp, global: bool) -> bool {
    let present = prepare_source(app, ui.ctx(), global);
    if global
        && !present
        && matches!(app.shared_history, SharedHistoryState::Ready(_))
        && matches!(app.shared_rollback, SharedRollbackState::Idle)
    {
        return false;
    }
    ui.separator();
    ui.heading("Bezel history & Undo");
    ui.label(
        "Saved bezel operations. Undo starts with a fresh preview and a separate confirmation.",
    );

    if !global {
        ui.label("Saved bezel records identify source artwork, not a verified game. Open all History to review bezel operations and Undo.");
    }
    // A preview opened for a different game or an unsupported record cannot
    // be confirmed here. Dropping an in-flight preview receiver is safe too;
    // this never cancels an executor that has already been confirmed.
    if let SharedRollbackState::Review { preview, .. } = &app.shared_rollback {
        let belongs = match &app.shared_history {
            SharedHistoryState::Ready(loaded) => loaded.report.journals.iter().any(|(path, j)| {
                j.operation_id == preview.original_operation_id
                    && *path == preview.journal_path
                    && in_scope(j, global)
                    && preview_allowed(j)
            }),
            _ => false,
        };
        if !belongs {
            app.shared_rollback = SharedRollbackState::Idle;
        }
    }
    if matches!(
        app.shared_rollback,
        SharedRollbackState::NativeReview { .. }
    ) {
        app.shared_rollback = SharedRollbackState::Idle;
    }
    if let SharedRollbackState::Review { preview, .. } = &app.shared_rollback
        && preview
            .entries
            .iter()
            .any(|entry| entry.outcome == SharedRollbackOutcome::AlreadyRolledBack)
    {
        ui.label("Already undone. The saved apply record remains a record of the earlier change.");
    }
    let mut action =
        crate::administration_pages::show_shared_rollback_card(ui, &mut app.shared_rollback);
    if ui
        .add_enabled(
            !matches!(app.shared_history, SharedHistoryState::Loading { .. }),
            egui::Button::new("Refresh bezel history"),
        )
        .clicked()
    {
        action = Some(HistoryPageAction::Refresh);
    }
    match &app.shared_history {
        SharedHistoryState::NotLoaded | SharedHistoryState::Loading { .. } => {
            ui.spinner();
            ui.label("Loading saved bezel operations…");
        }
        SharedHistoryState::Failed(message) => {
            ui.label(format!("Bezel history unavailable: {message}"));
        }
        SharedHistoryState::Ready(loaded) => {
            if !loaded.report.warnings.is_empty() || !loaded.report.complete {
                ui.label("Some saved operation records could not be read. Preserve those records for review.");
            }
            let mut rows: Vec<_> = loaded
                .report
                .journals
                .iter()
                .filter(|(_, j)| in_scope(j, global))
                .collect();
            rows.sort_by(|a, b| {
                b.1.timestamp_unix_seconds
                    .cmp(&a.1.timestamp_unix_seconds)
                    .then_with(|| b.1.operation_id.cmp(&a.1.operation_id))
            });
            if rows.is_empty() {
                ui.label("No saved bezel operations in this scope.");
            }
            let total = rows.len();
            for (path, journal) in rows.into_iter().take(200) {
                ui.push_id(&journal.operation_id, |ui| {
                ui.group(|ui| {
                    ui.strong(crate::administration_pages::shared_history_title(journal));
                    ui.label(crate::administration_pages::format_shared_history_time(journal.timestamp_unix_seconds, std::time::SystemTime::now()));
                    let read_only = if !supported(journal) {
                        Some("Read-only: this operation has no supported bezel Undo contract.")
                    } else if journal.rollback_operation_id.is_some() {
                        Some("Already undone. This record is read-only.")
                    } else if !preview_allowed(journal) {
                        Some("Read-only: this operation did not record a completed bezel apply. Review recovery records for interrupted changes.")
                    } else { None };
                    if let Some(reason) = read_only { ui.label(reason); }
                    ui.collapsing("Files and operation details", |ui| {
                        ui.monospace(&journal.operation_id);
                        ui.monospace(&journal.destination_root.display);
                        for entry in &journal.entries { ui.monospace(&entry.plan_entry.destination_relative_path.display); }
                    });
                    let paths = path.to_path_buf().ok().zip(journal.destination_root.to_path_buf().ok());
                    if ui.add_enabled(read_only.is_none() && paths.is_some() && matches!(app.shared_rollback, SharedRollbackState::Idle), egui::Button::new("Preview bezel Undo")).clicked()
                        && let Some((journal_path, destination_root)) = paths {
                            action = Some(HistoryPageAction::PreviewRollback { journal_path, destination_root });
                        }
                });
                });
            }
            if total > 200 {
                ui.label(format!("Showing the newest 200 of {total} bezel records."));
            }
        }
    }
    dispatch(app, ui.ctx(), action);
    present
}

fn dispatch(app: &mut ArchiveFsApp, ctx: &egui::Context, action: Option<HistoryPageAction>) {
    match action {
        Some(HistoryPageAction::PreviewRollback {
            journal_path,
            destination_root,
        }) => {
            app.start_shared_rollback_preview(ctx.clone(), journal_path, destination_root);
        }
        Some(HistoryPageAction::ConfirmRollback) => app.start_shared_rollback(ctx.clone()),
        Some(HistoryPageAction::CancelRollback) => app.shared_rollback = SharedRollbackState::Idle,
        Some(HistoryPageAction::Refresh) => {
            if matches!(
                app.shared_rollback,
                SharedRollbackState::Previewing { .. } | SharedRollbackState::Review { .. }
            ) {
                app.shared_rollback = SharedRollbackState::Idle;
            }
            app.shared_history = SharedHistoryState::NotLoaded;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
