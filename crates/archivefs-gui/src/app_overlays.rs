//! Application-wide overlays.
//!
//! Only the three genuinely shell-owned surfaces live here: the activity
//! panel (and the one navigation it can request), the About window and the
//! skipped-files window. Feature confirmations and feature dialogs stay
//! with the pages that own them.
//!
//! These run after the shell chrome and before the central panel, exactly
//! as they did inside `update`, so overlay precedence is unchanged.

use eframe::egui;

use crate::*;

pub(crate) fn show_global_overlays(app: &mut ArchiveFsApp, context: &egui::Context) {
    if app.ui_mode == GuiMode::AdvancedView
        && let Some(ActivityPanelAction::ShowRelatedArchive(path)) = show_activity_panel(
            context,
            &mut app.history,
            &mut app.show_activity,
            &mut app.clipboard,
        )
    {
        app.navigate_to_library_tab(LibraryTab::Archives);
        app.archive_context.select_only(path);
    }

    if app.show_about {
        let mount_root = match &app.state {
            LoadState::Ready(data) => Some(data.mount_root.as_path()),
            _ => None,
        };
        show_about_window(
            context,
            &mut app.show_about,
            &app.database_state,
            &app.doctor_repair.diagnostics,
            mount_root,
            &mut app.clipboard,
        );
    }

    if app.show_skipped_files {
        let summary = match &app.database_state {
            DatabaseState::Ready {
                last_scan_summary: Some(summary),
                ..
            } => Some(summary),
            _ => None,
        };
        show_skipped_files_window(
            context,
            &mut app.show_skipped_files,
            summary,
            &mut app.skipped_files_filter,
        );
    }
}
