//! The frame-scoped readiness values the shell and the pages share.
//!
//! `update` computes these once, before any chrome is drawn, so the shell
//! menus, the page dispatch and the blocking wording a page shows all agree
//! about what this frame is allowed to do. Computing them here also keeps
//! the one repaint request that depends on them in its original position:
//! after the view-gated starts and before the shell is drawn.

use eframe::egui;

use crate::{
    ArchiveFsApp, DiagnosticsState, LoadState, action_readiness_debug_lines,
    archive_action_block_reason, latest_generation_actions_safe, snapshot_identity,
};

pub(crate) struct FrameReadiness {
    pub(crate) loading: bool,
    pub(crate) busy: bool,
    pub(crate) has_database: bool,
    pub(crate) archive_actions_blocked: bool,
    pub(crate) archive_action_block_reason: Option<&'static str>,
    pub(crate) action_readiness_debug_lines: Vec<String>,
    pub(crate) missing_removal_available: bool,
}

pub(crate) fn frame_readiness(app: &mut ArchiveFsApp, context: &egui::Context) -> FrameReadiness {
    let loading = matches!(app.state, LoadState::Loading { .. });
    let diagnostics_loading = matches!(
        app.doctor_repair.diagnostics,
        DiagnosticsState::Loading { .. }
    );
    let busy = app.is_busy();
    let actions_safe = latest_generation_actions_safe(
        app.refresh_generation,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.doctor_repair.diagnostics,
    );
    let archive_actions_blocked = busy || !actions_safe;
    let archive_action_block_reason = archive_action_block_reason(
        busy,
        app.refresh_generation,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.doctor_repair.diagnostics,
    );
    let action_readiness_debug_lines = action_readiness_debug_lines(
        busy,
        app.refresh_generation,
        app.snapshot_generation,
        app.snapshot_stale,
        snapshot_identity(&app.state),
        &app.doctor_repair.diagnostics,
    );
    let missing_removal_available = app.missing_removal_action_available();
    if loading || diagnostics_loading || busy {
        context.request_repaint_after(std::time::Duration::from_millis(100));
    }

    let has_database = app.database_state.snapshot().is_some();
    FrameReadiness {
        loading,
        busy,
        has_database,
        archive_actions_blocked,
        archive_action_block_reason,
        action_readiness_debug_lines,
        missing_removal_available,
    }
}
