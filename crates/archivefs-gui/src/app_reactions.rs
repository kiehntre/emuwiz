//! Application-level reactions to the requests raised during a frame.
//!
//! The shell chrome and the pages only report what the user asked for; the
//! decisions those requests imply - navigation, mode switches, worker
//! starts, refreshes and global feedback - are applied here, in the same
//! order and at the same point in the frame as before.

use eframe::egui;

use crate::*;

/// Apply the request the shell chrome raised this frame, if any.
pub(crate) fn apply_shell_request(
    app: &mut ArchiveFsApp,
    context: &egui::Context,
    navigation_request: Option<app_shell::ShellRequest>,
) {
    match navigation_request {
        Some(app_shell::ShellRequest::Navigate(NavClick::View(view))) => {
            app.navigate_to_main_view(view);
            if view == MainView::DiscConversion {
                app.optical_conversion_page.get_or_insert_with(
                    optical_conversion_page::OpticalConversionPageState::default,
                );
            }
        }
        Some(app_shell::ShellRequest::Navigate(NavClick::QuickRename)) => {
            app.sources_ui.quick_rename_mode = true;
            app.navigate_to_main_view(MainView::IdentifyRename);
        }
        Some(app_shell::ShellRequest::Navigate(NavClick::Overlay(overlay))) => {
            app.tools_overlay = overlay;
            if overlay == ToolsOverlay::Diagnostics {
                app.refresh_diagnostics(context);
            }
        }
        Some(app_shell::ShellRequest::Navigate(NavClick::Romm)) => {
            app.navigate_to_sources_tab(SourcesTab::Libraries);
        }
        Some(app_shell::ShellRequest::ScanLibrary) => {
            app.start_database_action(context.clone(), true);
        }
        Some(app_shell::ShellRequest::RefreshDatabase) => {
            app.start_database_action(context.clone(), false);
        }
        Some(app_shell::ShellRequest::RefreshView) => app.refresh(context),
        Some(app_shell::ShellRequest::SelectAllVisible) => {
            app.select_all_visible_requested = true;
        }
        Some(app_shell::ShellRequest::ClearSelection) => app.archive_context.clear_selection(),
        Some(app_shell::ShellRequest::ToggleActivity) => {
            app.show_activity = !app.show_activity;
        }
        Some(app_shell::ShellRequest::ShowAbout) => app.show_about = true,
        Some(app_shell::ShellRequest::ReturnToGamerView) => {
            app.ui_mode = GuiMode::GamerView;
            app.view = MainView::Library;
            app.tools_overlay = ToolsOverlay::None;
            save_gui_mode(app.ui_mode);
        }
        Some(app_shell::ShellRequest::GamerAddFolder(folder)) => {
            app.gamer_view_scan_review_available = false;
            app.start_source_action(context.clone(), SourceAction::Add(folder));
        }
        Some(app_shell::ShellRequest::GamerScan) => {
            app.gamer_view_scan_review_available = false;
            app.gamer_view_scan_pending_review = true;
            app.start_source_action(context.clone(), SourceAction::ScanAll);
        }
        Some(app_shell::ShellRequest::GamerSetup) => {
            app.ui_mode = GuiMode::AdvancedView;
            save_gui_mode(app.ui_mode);
            app.navigate_to_main_view(MainView::EmulatorSetup);
        }
        Some(app_shell::ShellRequest::GamerAdvanced) => {
            app.switch_to_advanced_view_at_home();
            save_gui_mode(app.ui_mode);
        }
        None => {}
    }
}
