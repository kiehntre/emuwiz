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
        Some(app_shell::ShellRequest::Navigate(NavClick::Enhancement(section))) => {
            crate::cheats_mods_preview::select_enhancement_section(context, section);
            app.navigate_to_main_view(MainView::CheatsMods);
        }
        Some(app_shell::ShellRequest::Navigate(NavClick::View(view))) => {
            // These labels name exact tasks, not the last visited tab in an
            // old consolidated shell (Sources might otherwise reopen DATs).
            match view {
                MainView::Sources => app.navigate_to_sources_tab(SourcesTab::Libraries),
                MainView::Problems => {
                    app.navigate_to_problems_repair_tab(ProblemsRepairTab::Overview)
                }
                _ => app.navigate_to_main_view(view),
            }
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
        Some(app_shell::ShellRequest::SimpleMode) => {
            app.ui_mode = GuiMode::Simple;
            app.navigate_to_main_view(MainView::Home);
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

/// Apply the requests the central panel raised this frame.
///
/// `update` used to run this block inline immediately after the panel
/// closed; the order of the arms is unchanged, including the health
/// dashboard's ability to raise an `AppOperationRequest` that the final
/// arm then services in the same frame.
pub(crate) fn apply_page_requests(
    app: &mut ArchiveFsApp,
    context: &egui::Context,
    outcome: app_pages::PageDispatchOutcome,
) {
    let app_pages::PageDispatchOutcome {
        retry,
        mut requested_action,
        diagnostics_action,
        health_dashboard_action,
        stop_mount_all,
        stop_unmount_all,
    } = outcome;
    if stop_mount_all {
        app.request_mount_all_stop();
    }
    if let Some(action) = diagnostics_action {
        match action {
            DiagnosticsUiAction::Refresh => app.refresh_diagnostics(context),
            DiagnosticsUiAction::Continue => {
                app.tools_overlay = ToolsOverlay::None;
                app.refresh(context);
            }
            DiagnosticsUiAction::ViewLastSnapshot => {
                app.tools_overlay = ToolsOverlay::None;
            }
            DiagnosticsUiAction::CreateStarterConfig => {
                app.start_setup_action(context.clone(), SetupAction::CreateStarterConfig)
            }
            DiagnosticsUiAction::CreateMountRoot => {
                app.start_setup_action(context.clone(), SetupAction::CreateMountRoot)
            }
            DiagnosticsUiAction::OpenConfigFolder => {
                app.start_setup_action(context.clone(), SetupAction::OpenConfigFolder)
            }
            DiagnosticsUiAction::CopyConfigPath => {
                if let DiagnosticsState::Ready { report, .. } = &app.doctor_repair.diagnostics
                    && let Some(path) = &report.config_path
                {
                    let path = path.display().to_string();
                    let _ = app.clipboard.set_text(path.clone());
                    app.history.record(HistoryEntry::new(
                        ActivityAction::Setup,
                        None,
                        ActivityOutcome::Completed,
                        format!("Copied config path: {path}"),
                    ));
                }
            }
        }
    }
    if let Some(action) = health_dashboard_action {
        match action {
            HealthDashboardAction::BackToLibrary => {
                app.navigate_to_library_tab(LibraryTab::Archives);
            }
            HealthDashboardAction::Archive(request) => {
                requested_action = Some(AppOperationRequest::Archive(request));
            }
            HealthDashboardAction::RefreshDiagnostics => {
                app.refresh_diagnostics(context);
            }
            HealthDashboardAction::OpenMissingReview => {
                app.navigate_to_missing_catalogue_review();
            }
            HealthDashboardAction::OpenDuplicateReview => {
                app.navigate_to_library_tab(LibraryTab::Duplicates);
            }
            HealthDashboardAction::ViewInLibrary(path) => {
                app.navigate_to_library_tab(LibraryTab::Archives);
                app.archive_context.select_only(path);
            }
            HealthDashboardAction::Inspect(path) => {
                requested_action = Some(AppOperationRequest::InspectArchive(path));
            }
            HealthDashboardAction::FilterByCategory(filter) => {
                app.health_duplicate_ui.health_filters.category = filter;
            }
        }
    }
    if stop_unmount_all {
        app.request_unmount_all_stop();
    }
    if retry {
        app.refresh(context);
    }
    if let Some(request) = requested_action {
        match request {
            AppOperationRequest::Archive(request) => {
                app.start_operation(
                    context.clone(),
                    request.action,
                    request.archive_path,
                    request.cleanup_after_unmount,
                );
            }
            AppOperationRequest::MountAll(items) => {
                app.start_mount_all(context.clone(), items);
            }
            AppOperationRequest::UnmountAll {
                items,
                cleanup_after_unmount,
            } => {
                app.start_unmount_all(context.clone(), items, cleanup_after_unmount);
            }
            AppOperationRequest::PlatformAssignment {
                archive_path,
                action,
            } => {
                app.start_platform_action(context.clone(), archive_path, action);
            }
            AppOperationRequest::BulkPlatformAssignment {
                archive_paths,
                kind,
            } => {
                app.start_bulk_platform_action(context.clone(), archive_paths, kind);
            }
            AppOperationRequest::RemoveMissing(archive_paths) => {
                app.start_missing_removal(context.clone(), archive_paths);
            }
            AppOperationRequest::UpdateGameFolder => {
                app.navigate_to_sources_tab(SourcesTab::Libraries);
            }
            AppOperationRequest::FullRescan => {
                app.start_source_action(context.clone(), SourceAction::ScanAll);
            }
            AppOperationRequest::ReviewMissingGames => {
                app.navigate_to_missing_catalogue_review();
            }
            AppOperationRequest::InspectArchive(archive_path) => {
                app.start_archive_inspection(context.clone(), archive_path);
            }
            AppOperationRequest::ShowInLibraryViews(archive_path) => {
                app.navigate_to_library_tab(LibraryTab::Views);
                app.library_view_focus_archive = Some(archive_path);
            }
            AppOperationRequest::OpenCheatsMods(archive_path) => {
                app.archive_context.select_only(archive_path.clone());
                app.open_cheats_mods_workspace(context, archive_path);
            }
            AppOperationRequest::OpenDatSources => {
                app.sources_ui.quick_rename_mode = false;
                app.view = MainView::DatSources;
            }
            AppOperationRequest::ApplyScreenScraperEnrichment(action) => {
                let crate::screenscraper_enrichment_page::ScreenScraperEnrichmentAction::Apply {
                    archive_id,
                    values,
                    receipt,
                } = *action;
                app.apply_screenscraper_enrichment(context.clone(), archive_id, values, receipt);
            }
        }
    }
}
