//! Ordered application-level polling, reconciliation and view-gated starts.
//!
//! The sequence in this module is intentionally kept in frame order. Feature
//! modules own their workers and poll methods; this module only coordinates
//! the existing calls before page rendering: first the poll/reconcile pass,
//! then the automatic starts that a freshly-opened page needs.

use eframe::egui;

use crate::{
    ArchiveFsApp, BsFreeManagerState, BsFreeOperation, CheatEmulatorAdapter, CheatStepResource,
    DolphinCatalogueManagerState, DolphinProfilesState, GuiMode, MainView, SharedHistoryState,
    SharedRollbackState, catalogue_status_load_needed, database_state_path,
    dolphin_catalogue_status_load_needed,
};

pub(crate) fn poll_and_reconcile(app: &mut ArchiveFsApp, context: &egui::Context) {
    app.reconcile_library_tab();
    app.reconcile_problems_repair_tab();
    app.reconcile_sources_tab();
    app.reconcile_selected_evidence_selection();
    app.reconcile_archive_preparation();
    app.poll_needs_attention(context);
    app.artwork_media.platform_artwork.poll(context);
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

/// Start the background work a freshly-opened page needs.
///
/// Every start here is view-gated and idempotent: a page that has not been
/// visited does no I/O, and a page that is already loading or loaded starts
/// nothing. `update` ran this immediately after the poll phase and before
/// any readiness was computed, and it still does, so a result delivered by
/// this frame's polling is what these guards see.
pub(crate) fn start_view_gated_work(app: &mut ArchiveFsApp, context: &egui::Context) {
    if matches!(
        app.view,
        MainView::Sources | MainView::CheatsMods | MainView::CheatSources
    ) && matches!(
        app.catalogue_bsfree_ui.bsfree_manager,
        BsFreeManagerState::NotLoaded
    ) && app.catalogue_bsfree_ui.bsfree_operation.is_none()
    {
        app.start_bsfree_operation(context.clone(), BsFreeOperation::LoadStatus);
    }
    // Only once the Sources page is actually open, and only local reads - so
    // starting EmuWiz still makes no network request of any kind.
    if app.view == MainView::Sources
        && app.romm_ui.snapshot.is_none()
        && app.romm_ui.operation.is_none()
    {
        app.start_romm_status_load(context.clone());
    }
    if app.view == MainView::CheatsMods
        && app.cheat_workflow.as_ref().is_some_and(|workflow| {
            workflow.adapter == CheatEmulatorAdapter::Dolphin
                && workflow.selected_dolphin_profile_id.is_some()
                && matches!(workflow.dolphin_inventory, CheatStepResource::NotLoaded)
        })
        && matches!(
            app.emulator_readiness.dolphin_profiles,
            DolphinProfilesState::Ready(_)
        )
    {
        app.start_dolphin_inventory(context.clone());
    }
    if catalogue_status_load_needed(app.view, &app.catalogue_bsfree_ui.catalogue_manager) {
        app.start_catalogue_status_load(context.clone());
    }
    if dolphin_catalogue_status_load_needed(
        app.view,
        &app.catalogue_bsfree_ui.dolphin_catalogue_manager,
    ) {
        app.start_dolphin_catalogue_status_load(context.clone());
    }
    // The one quiet, automatic "Check for updates" per session: only
    // once a catalogue is confirmed installed, and only once ever
    // (`dolphin_catalogue_update_available` starts `None` and this is
    // the only place that can set it besides an explicit click).
    if app.view == MainView::CheatsMods
        && app
            .catalogue_bsfree_ui
            .dolphin_catalogue_update_available
            .is_none()
        && app
            .catalogue_bsfree_ui
            .dolphin_catalogue_update_check
            .is_none()
        && matches!(
            &app.catalogue_bsfree_ui.dolphin_catalogue_manager,
            DolphinCatalogueManagerState::Ready(snapshot) if snapshot.catalogue.is_some()
        )
    {
        app.start_dolphin_catalogue_update_check(context.clone());
    }
    if app.view == MainView::CheatsMods
        && app.cheat_workflow.as_ref().is_some_and(|workflow| {
            workflow.identity_request.is_none()
                && matches!(workflow.identity, CheatStepResource::NotLoaded)
        })
    {
        app.start_game_identity_inspection(context.clone());
    }
    if app.view == MainView::CheatsMods
        && app.cheat_workflow.as_ref().is_some_and(|workflow| {
            workflow.preview_request.is_none()
                && matches!(workflow.preview, CheatStepResource::NotLoaded)
        })
    {
        app.start_cheat_preview(context.clone());
    }
    // Matching is deliberately manual only - triggered by the "Find
    // matching cheat files" button (`start_cheat_candidate_match`),
    // never auto-started here. An earlier version auto-triggered this
    // every frame whenever `candidates` was `NotLoaded`; when a
    // prerequisite silently failed, that auto-trigger raced the
    // button's own click on the same silent failure, forever, with
    // neither ever producing a visible result. Matching now has
    // exactly one entry point, and every call to it produces a visible
    // state (see `start_cheat_candidate_match`'s doc comment).
}
