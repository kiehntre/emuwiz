//! Ordered application-level polling and result reconciliation.
//!
//! The sequence in this module is intentionally kept in frame order. Feature
//! modules own their workers and poll methods; this module only coordinates
//! the existing calls before view-gated starts and page rendering.

use eframe::egui;

use crate::{
    ArchiveFsApp, GuiMode, MainView, SharedHistoryState, SharedRollbackState, database_state_path,
};

pub(crate) fn poll_and_reconcile(app: &mut ArchiveFsApp, context: &egui::Context) {
    app.reconcile_library_tab();
    app.reconcile_problems_repair_tab();
    app.reconcile_sources_tab();
    app.reconcile_selected_evidence_selection();
    app.reconcile_archive_preparation();
    app.poll_needs_attention(context);
    app.poll_platform_artwork_task(context);
    app.poll_shared_history();
    // Gamer View's "Undo last change" (docs/GUI_NAVIGATION_RESET_DESIGN.md
    // mandatory risk #2) drives this exact same `shared_rollback` state
    // machine while `app.view` stays `Library` (Gamer View never sets
    // `MainView::HistoryLogs`) - so this Advanced-View-only cleanup rule
    // must not fire while in Gamer View, or a rollback preview/review
    // started from Gamer View would be reset to `Idle` before the user
    // ever sees it. Advanced View's own behaviour (reset on leaving
    // History & Logs) is completely unchanged.
    if app.view != MainView::HistoryLogs
        && app.ui_mode != GuiMode::GamerView
        && matches!(
            app.shared_rollback,
            SharedRollbackState::Previewing { .. } | SharedRollbackState::Review { .. }
        )
    {
        app.shared_rollback = SharedRollbackState::Idle;
    }
    app.poll_shared_rollback();
    if app.view == MainView::HistoryLogs
        && matches!(app.shared_history, SharedHistoryState::NotLoaded)
    {
        app.refresh_shared_history(context.clone());
    }
    app.poll_load(context);
    app.poll_database_load(context);
    if app.sources_ui.dat_authority.tick(
        app.database_generation.0,
        database_state_path(&app.database_state),
        matches!(app.view, MainView::DatSources | MainView::NeedsAttention)
            && !app.database_state.is_loading(),
        context,
    ) {
        app.invalidate_needs_attention();
    }
    app.poll_diagnostics();
    app.poll_setup_action(context);
    app.poll_doctor_scan();
    app.poll_rpcs3_status();
    app.poll_pcsx2_status();
    app.poll_platform_action(context);
    app.poll_bulk_platform_action(context);
    app.poll_alias_action(context);
    app.poll_source_action(context);
    app.poll_bsfree_operation(context);
    app.poll_romm_operation(context);
    app.poll_catalogue_manager(context);
    app.poll_dolphin_catalogue_manager(context);
    app.poll_library_view_action(context);
    app.poll_archive_inspection();
    app.poll_archive_preparation(context);
    app.poll_selected_evidence();
    app.poll_identity_sources();
    app.poll_plan_preview();
    app.poll_missing_removal(context);
    app.poll_operation(context);
    app.poll_mount_all(context);
    app.poll_unmount_all(context);
    app.poll_retroarch_profiles();
    app.poll_pcsx2_profiles();
    app.poll_dolphin_profiles();
    app.poll_dolphin_local_profiles();
    app.poll_pcsx2_launch_profiles();
    app.poll_flycast_profiles();
    app.poll_pcsx2_firmware_evidence();
    app.poll_cheat_workflow(context);
    app.cheatbase_page.poll(context);
    app.emulator_download_page.poll(context);
    if let Some(_installed_id) = app.emulator_download_page.take_completed_install() {
        // A managed AppImage was just installed: re-run the read-only
        // discovery / Doctor / readiness so Emulator Setup and Play
        // availability reflect it. Discovery is authoritative - the
        // download page never asserts launch readiness itself.
        app.start_doctor_scan(context.clone());
    }
}
