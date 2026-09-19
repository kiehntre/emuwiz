//! Central-panel page dispatch.
//!
//! `update` polls workers and draws the shell chrome; this module owns the
//! single central panel that decides which page is rendered for the current
//! route and collects the frame-level requests those pages emit. The branch
//! order, overlay precedence and page entry points are unchanged - only the
//! physical location of the dispatch moved. Page bodies and their reducers
//! stay with their existing owners.

use eframe::egui;

use crate::*;

/// Frame-scoped values `update` computed before the panel opened.
///
/// They are passed in rather than recomputed so the readiness/blocking
/// wording a page shows is byte-for-byte the value the shell already used
/// this frame.
pub(crate) struct PageDispatchInputs {
    pub(crate) busy: bool,
    pub(crate) archive_actions_blocked: bool,
    pub(crate) archive_action_block_reason: Option<&'static str>,
    pub(crate) action_readiness_debug_lines: Vec<String>,
    pub(crate) missing_removal_available: bool,
}

/// Frame-level requests produced while the central panel was drawn.
///
/// `update` applies these after the panel closes, in the same order as
/// before the dispatch moved out of it.
pub(crate) struct PageDispatchOutcome {
    pub(crate) retry: bool,
    pub(crate) requested_action: Option<AppOperationRequest>,
    pub(crate) diagnostics_action: Option<DiagnosticsUiAction>,
    pub(crate) health_dashboard_action: Option<HealthDashboardAction>,
    pub(crate) stop_mount_all: bool,
    pub(crate) stop_unmount_all: bool,
}

pub(crate) fn show_pages(
    app: &mut ArchiveFsApp,
    context: &egui::Context,
    inputs: PageDispatchInputs,
) -> PageDispatchOutcome {
    let PageDispatchInputs {
        busy,
        archive_actions_blocked,
        archive_action_block_reason,
        action_readiness_debug_lines,
        missing_removal_available,
    } = inputs;
    let mut retry = false;
    let mut requested_action = None;
    let mut diagnostics_action = None;
    let mut health_dashboard_action = None;
    let mut stop_mount_all = false;
    let mut stop_unmount_all = false;
    egui::CentralPanel::default().show(context, |ui| {
        if app.ui_mode == GuiMode::Simple {
            crate::simple_mode::readability(ui);
        }
        let width = if app.tools_overlay == ToolsOverlay::None {
            main_view_content_width(app.view)
        } else if app.tools_overlay == ToolsOverlay::ArchiveInspector {
            ui_layout::ContentWidth::Wide
        } else {
            ui_layout::ContentWidth::Normal
        };
        let page_scroll = main_view_uses_page_scroll(app.view)
            || app.tools_overlay == ToolsOverlay::SaveVault
            || (app.ui_mode != GuiMode::AdvancedView && app.view == MainView::Library);
        ui_layout::page(ui, width, page_scroll, app.view, |ui| {
            app.reconcile_cheats_mods_context(context);

            if app.ui_mode == GuiMode::Simple && app.view == MainView::ReadyToPlay
                && app.tools_overlay == ToolsOverlay::None {
                widgets::workflow_header(ui, "Play", "Choose a game from My Games. EmuWiz will show whether it is ready and what to set up next.");
                if crate::simple_mode::primary_button(ui, "Choose a game to play", true).clicked() {
                    app.navigate_to_main_view(MainView::Library);
                    return;
                }
                ui.label("Next: select a game, then press Play. Opening the game list does not launch anything.");
                ui.collapsing("Advanced details — readiness summary", |ui| {
                    app.emulator_readiness.ready_to_play_page.show(ui);
                });
                return;
            }

            if app.tools_overlay != ToolsOverlay::None {
                match app.tools_overlay {
                    ToolsOverlay::SaveVault => {
                        let focused_archive = app.archive_context.focused.clone();
                        app.invalidate_pcsx2_status_if_selection_changed(focused_archive.as_deref());
                        let verified_ps2_serial = app.cheat_workflow.as_ref()
                            .and_then(pcsx2_identity_for_workflow).and_then(|id| id.serial);
                        let action = pcsx2_page::show_save_vault_landing(
                            ui, app.ui_mode == GuiMode::AdvancedView, verified_ps2_serial.as_deref(),
                            &app.emulator_readiness.pcsx2_status, &mut app.emulator_readiness.pcsx2_save_vault,
                        );
                        app.handle_pcsx2_action(context, action);
                    }
                    ToolsOverlay::Diagnostics => {
                        diagnostics_action = show_setup_diagnostics(
                            ui,
                            &app.doctor_repair.diagnostics,
                            app.setup_action.is_some(),
                            app.feedback.as_ref(),
                            app.refresh_error.as_deref(),
                            app.snapshot_stale && matches!(app.state, LoadState::Ready(_)),
                            app.doctor_repair.config_previously_confirmed,
                        );
                    }
                    ToolsOverlay::PlatformAliases => {
                        if widgets::show_tools_overlay_header(ui, "Platform Aliases") {
                            app.tools_overlay = ToolsOverlay::None;
                        }
                        let cached_aliases = app
                            .database_state
                            .snapshot()
                            .map(|snapshot| snapshot.platform_aliases.as_slice())
                            .unwrap_or(&[]);
                        if let Some(action) = show_platform_aliases_panel(
                            ui,
                            cached_aliases,
                            &mut app.library_ui.new_alias_text,
                            &mut app.library_ui.new_alias_platform_choice,
                            app.library_ui.alias_action.is_some(),
                            &mut app.clipboard,
                        ) {
                            app.start_alias_action(context.clone(), action);
                        }
                    }
                    ToolsOverlay::DatabaseStatus => {
                        if widgets::show_tools_overlay_header(ui, "Database Status") {
                            app.tools_overlay = ToolsOverlay::None;
                        }
                        if let Some(action) = show_database_panel(ui, &app.database_state) {
                            match action {
                                DatabasePanelAction::ScanLibrary
                                | DatabasePanelAction::ScanAndUpgradeLibrary => {
                                    app.start_database_action(context.clone(), true);
                                }
                                DatabasePanelAction::ViewRecentlyFound => {
                                    app.navigate_to_library_tab(LibraryTab::RecentlyFound);
                                }
                                DatabasePanelAction::RefreshStatus
                                | DatabasePanelAction::RetryLoad => {
                                    app.start_database_action(context.clone(), false);
                                }
                                DatabasePanelAction::ViewSkippedFiles => {
                                    app.show_skipped_files = true;
                                    app.skipped_files_filter = None;
                                }
                            }
                        }
                    }
                    // The original `DoctorReport` checks, unchanged. The
                    // Doctor *page* now shows the shared read-only
                    // findings instead (see `show_doctor_page`), so this
                    // overlay keeps the older per-check view - and the
                    // summary/report text that belongs with it -
                    // reachable rather than dropping either.
                    ToolsOverlay::DoctorChecks => {
                        if widgets::show_tools_overlay_header(ui, "Doctor Checks") {
                            app.tools_overlay = ToolsOverlay::None;
                        }
                        let doctor = match &app.state {
                            LoadState::Ready(data) => Some(&data.doctor),
                            _ => None,
                        };
                        if let Some(report) = doctor {
                            ui.label(doctor_summary_text(report));
                            if widgets::action_button(
                                ui,
                                "Copy summary",
                                widgets::ActionStyle::Secondary,
                                true,
                            )
                            .clicked()
                            {
                                let _ = app.clipboard.set_text(doctor_report_text(report));
                            }
                            ui.add_space(8.0);
                        }
                        show_doctor_checks_panel(ui, doctor);
                    }
                    ToolsOverlay::ArchiveInspector => {
                        if let Some(inspector) = app.archive_inspector.as_mut()
                            && show_archive_inspector_panel(ui, inspector, &mut app.clipboard)
                        {
                            app.tools_overlay = ToolsOverlay::None;
                        }
                    }
                    ToolsOverlay::Onboarding => {
                        app.show_onboarding_overlay(ui, context);
                    }
                    ToolsOverlay::None => unreachable!(),
                }
                return;
            }

            // docs/GUI_NAVIGATION_RESET_DESIGN.md: Gamer View is one
            // screen (`app.view` stays at its default, `Library`,
            // the whole time it's active) plus the existing,
            // unmodified Cheats & Mods page when opened from the
            // selected-game action panel below. Every other
            // `MainView` destination is Advanced-View-only and
            // unreachable while `ui_mode` is `GamerView`, since
            // nothing in this mode's UI ever sets `app.view` to one.
            if (app.ui_mode == GuiMode::GamerView && app.view != MainView::CheatsMods)
                || (app.ui_mode == GuiMode::Simple && app.view == MainView::Library) {
                app.artwork_media.es_de_media.start(ui.ctx().clone());
                if app.artwork_media.es_de_media.poll() {
                    app.artwork_media.gamer_covers.identity_refreshed();
                    app.artwork_media.gamer_screenshots.identity_refreshed();
                    if let Some(worker) = app.artwork_media.gamer_cover_worker.as_ref() {
                        worker.update_esde(app.artwork_media.es_de_media.snapshot().cloned());
                    }
                }
                if let Some(path) = app.archive_context.focused.clone() {
                    let evidence_is_stale = match &app.selected_evidence_ui.selected_evidence {
                        selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                            report.path != path
                        }
                        selected_evidence_page::SelectedEvidenceState::Loading {
                            path: loading_path, ..
                        } => loading_path != &path,
                        selected_evidence_page::SelectedEvidenceState::Idle => true,
                        selected_evidence_page::SelectedEvidenceState::Error {
                            path: error_path, ..
                        } => error_path != &path,
                    };
                    if evidence_is_stale {
                        app.start_selected_evidence_load(context.clone(), path);
                    }
                }
                app.maybe_start_selected_evidence_enrichment(context);
                if matches!(app.emulator_readiness.retroarch_profiles, RetroArchProfilesState::NotScanned) {
                    app.start_retroarch_profile_scan(context.clone());
                }
                let data = match &app.state {
                    LoadState::Ready(data) => Some(data.as_ref()),
                    LoadState::Loading { previous, .. } => previous.as_deref(),
                    LoadState::Error(_) => None,
                };
                // A reloaded library may map the same path to a different
                // archive, and a re-imported RomM catalogue changes what any
                // path resolves to, so both discard every answer rather than
                // risk drawing one game's cover beside another. A search or a
                // platform change does neither: it narrows which records are
                // visible without changing what any of them is, so covers
                // already loaded stay loaded and are not fetched twice.
                let library = data.map(|data| data.config_identity.clone());
                if app.artwork_media.gamer_cover_library != library {
                    app.artwork_media.gamer_cover_library = library;
                    app.artwork_media.gamer_covers.library_changed();
                    app.artwork_media.gamer_screenshots.library_changed();
                }
                // Answers first, so a cover that arrived since the last frame
                // is drawn in this one. Anything from a superseded generation
                // is dropped inside `absorb`.
                if let Some(worker) = app.artwork_media.gamer_cover_worker.as_ref() {
                    for update in worker.drain_delivery() {
                        app.artwork_media.gamer_covers.absorb_delivery(&update);
                        app.artwork_media.gamer_screenshots.absorb_delivery(&update);
                    }
                    for reply in worker.drain() {
                        if !app.artwork_media.gamer_covers.absorb(ui.ctx(), reply.clone()) {
                            app.artwork_media.gamer_screenshots.absorb(ui.ctx(), reply);
                        }
                    }
                }
                let mut cover_requests: Vec<crate::gamer_artwork::CoverJob> = Vec::new();
                let mut screenshot_requests: Vec<crate::gamer_artwork::CoverJob> = Vec::new();
                // Enrichment (synopsis/genre/players/rating/release year):
                // answers first, same as covers above, then a request only
                // when the focused game actually changed - never once per
                // frame, and never for a row that merely scrolled into view.
                if let Some(worker) = app.artwork_media.game_metadata_worker.as_mut() {
                    for reply in worker.poll() {
                        app.artwork_media.selected_game_metadata = Some((reply.local_path, reply.result));
                    }
                }
                let focused_archive = app.archive_context.focused.clone();
                let metadata_is_stale = app
                    .artwork_media
                    .selected_game_metadata
                    .as_ref()
                    .map(|(path, _)| path)
                    != focused_archive.as_ref();
                if metadata_is_stale
                    && let Some(path) = focused_archive.as_ref()
                    && app.artwork_media.game_metadata_worker_allowed
                {
                    let worker = app.artwork_media.game_metadata_worker.get_or_insert_with(|| {
                        crate::game_metadata::GameMetadataWorker::start(ui.ctx().clone())
                    });
                    worker.request(path);
                }
                let game_metadata = app
                    .artwork_media
                    .selected_game_metadata
                    .as_ref()
                    .filter(|(path, _)| Some(path) == focused_archive.as_ref())
                    .map(|(_, result)| result);
                let gamer_launch_input = app.build_launch_readiness_input(data);
                let gamer_play_action =
                    launch_readiness_page::gamer_play_action(&gamer_launch_input);
                let gamer_identity_status = match &app.selected_evidence_ui.selected_evidence {
                    selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                        Some(gamer_identity_status_from_verdict(report.identity.status))
                    }
                    selected_evidence_page::SelectedEvidenceState::Loading { .. }
                    | selected_evidence_page::SelectedEvidenceState::Idle => {
                        Some(GamerIdentityStatus::StillChecking)
                    }
                    selected_evidence_page::SelectedEvidenceState::Error { .. } => None,
                };
                let (prepared_member, member_choices_owned, preparation_message_owned) =
                    focused_archive
                        .as_deref()
                        .map(|path| app.archive_preparation_view(path))
                        .unwrap_or((false, None, None));
                let member_choices = member_choices_owned.as_deref();
                let preparation_message = preparation_message_owned.as_deref();
                let artwork = app.artwork_media.platform_artwork.render_assets();
                let gamer_action = show_gamer_view(
                    ui,
                    data,
                    GamerViewViewState {
                        filter: &mut app.library_ui.filter,
                        library_filters: &mut app.library_ui.library_filters,
                        archive_context: &mut app.archive_context,
                        screen: &mut app.gamer_view_screen,
                        busy: archive_actions_blocked,
                        block_reason: archive_action_block_reason,
                        cleanup_after_unmount: app.mount_ui.cleanup_after_unmount,
                        cheat_workflow: app.cheat_workflow.as_ref(),
                        feedback: app.feedback.as_ref(),
                        scan_review_available: app.gamer_view_scan_review_available,
                        artwork_directory: artwork.directory,
                        artwork_cache: artwork.cache,
                        covers: &mut app.artwork_media.gamer_covers,
                        screenshots: &mut app.artwork_media.gamer_screenshots,
                        cover_requests: &mut cover_requests,
                        screenshot_requests: &mut screenshot_requests,
                        game_metadata,
                        identity_status: gamer_identity_status,
                        prepared_member,
                        member_choices,
                        preparation_message,
                        play_action: &gamer_play_action,
                        retroarch_launch_state: &mut app.launch_retroarch,
                        dolphin_launch_state: &mut app.launch_dolphin,
                        pcsx2_launch_state: &mut app.launch_pcsx2,
                        standalone_launch_state: &mut app.launch_standalone,
                        alpha_jump: &mut app.artwork_media.gamer_alpha_jump,
                    },
                );
                // Started only once the list has actually asked for something,
                // so a session that never opens Gamer View never opens the
                // catalogue, and an empty or unfiltered-to-nothing list starts
                // no thread at all.
                if (!cover_requests.is_empty() || !screenshot_requests.is_empty())
                    && app.artwork_media.gamer_cover_worker_allowed
                {
                    let worker = app.artwork_media.gamer_cover_worker.get_or_insert_with(|| {
                        crate::gamer_artwork::CoverWorker::start(
                            ui.ctx().clone(),
                            app.gui_config.source_roots().ok().map(<[PathBuf]>::to_vec),
                            app.artwork_media.es_de_media.snapshot().cloned(),
                            app.artwork_media.launchbox_local_media.snapshot().cloned(),
                        )
                    });
                    let generation = app.artwork_media.gamer_covers.generation();
                    for job in cover_requests {
                        worker.request(generation, job);
                    }
                    for job in screenshot_requests {
                        worker.request(generation, job);
                    }
                }
                match gamer_action {
                    Some(GamerViewAction::Prepare(archive_path)) => {
                        app.start_archive_preparation(context.clone(), archive_path);
                    }
                    Some(GamerViewAction::SelectArchiveMember(archive_path, member_name)) => {
                        app.select_archive_member(
                            context,
                            archive_path,
                            member_name,
                        );
                    }
                    Some(GamerViewAction::Play(request)) => {
                        request.start(
                            &mut app.launch_retroarch,
                            &mut app.launch_dolphin,
                            &mut app.launch_pcsx2,
                            &mut app.launch_standalone,
                            &mut app.launch_amiga_whdload,
                        );
                    }
                    Some(GamerViewAction::Operation(request)) => {
                        if matches!(
                            request.action,
                            ArchiveAction::Unmount | ArchiveAction::LazyUnmount
                        ) {
                            app.archive_preparation_generation =
                                app.archive_preparation_generation.next();
                            app.archive_preparation = ArchivePreparationState::Idle;
                        }
                        requested_action = Some(AppOperationRequest::Archive(request));
                    }
                    Some(GamerViewAction::OpenCheatsMods(archive_path)) => {
                        requested_action = Some(AppOperationRequest::OpenCheatsMods(archive_path));
                    }
                    Some(GamerViewAction::CopyLocation(folder)) => {
                        match app.clipboard.set_text(folder) {
                            Ok(()) => {
                                app.feedback = Some(ActionFeedback {
                                    succeeded: true,
                                    message: "Copied the game's folder location to the clipboard.".to_string(),
                                    cleanup: None,
                                    warning: None,
                                    more_information: None,
                                });
                            }
                            Err(error) => {
                                app.feedback = Some(ActionFeedback {
                                    succeeded: false,
                                    message: format!("Could not copy to the clipboard: {error}"),
                                    cleanup: None,
                                    warning: None,
                                    more_information: None,
                                });
                            }
                        }
                    }
                    Some(GamerViewAction::Undo) => {
                        app.start_cheat_install_rollback(context.clone());
                    }
                    Some(GamerViewAction::AddGamesFolder(folder)) => {
                        app.sources_ui.gamer_view_pending_first_scan = Some(folder.clone());
                        app.start_source_action(context.clone(), SourceAction::Add(folder));
                    }
                    Some(GamerViewAction::ReviewScan) => {
                        app.gamer_view_scan_review_available = false;
                        if app.ui_mode != GuiMode::Simple {
                            app.ui_mode = GuiMode::AdvancedView;
                            save_gui_mode(app.ui_mode);
                        }
                        app.navigate_to_sources_tab(SourcesTab::Discovery);
                    }
                    Some(GamerViewAction::ScanForNewGames) => {
                        app.start_source_action(context.clone(), SourceAction::ScanAll);
                    }
                    Some(GamerViewAction::ReviewIdentity(archive_path)) => {
                        app.review_identity(archive_path);
                    }
                    Some(GamerViewAction::OpenLaunchChoices(archive_path)) => {
                        app.review_identity(archive_path);
                    }
                    Some(GamerViewAction::CheckEmulators(archive_path)) => {
                        app.archive_context.select_only(archive_path);
                        if app.ui_mode != GuiMode::Simple {
                            app.ui_mode = GuiMode::AdvancedView;
                            save_gui_mode(app.ui_mode);
                        }
                        app.start_doctor_scan(context.clone());
                        app.navigate_to_main_view(MainView::EmulatorSetup);
                    }
                    Some(GamerViewAction::OpenEmulatorSetup(archive_path, focus)) => {
                        app.open_emulator_setup_for(archive_path, focus);
                    }
                    // A match guard here (clippy's suggestion) would make
                    // this otherwise-exhaustive `GamerViewAction` match
                    // non-exhaustive and force a redundant fallback arm.
                    #[allow(clippy::collapsible_match)]
                    Some(GamerViewAction::RefreshGameInformation) => {
                        if app.artwork_media.game_metadata_worker_allowed {
                            let worker = app.artwork_media.game_metadata_worker.get_or_insert_with(|| {
                                crate::game_metadata::GameMetadataWorker::start(
                                    context.clone(),
                                )
                            });
                            worker.reload();
                            // Cleared so the panel shows its "not yet resolved"
                            // state until the reload (queued strictly before
                            // this re-request on the same worker channel)
                            // finishes and answers it - never stale data
                            // presented as freshly refreshed.
                            app.artwork_media.selected_game_metadata = None;
                            if let Some(path) = &app.archive_context.focused {
                                worker.request(path);
                            }
                        }
                    }
                    None => {}
                }
                return;
            }

            if let Some(error) = &app.refresh_error {
                if app.ui_mode == GuiMode::Simple {
                    widgets::banner(ui, "Could not refresh your games", "Your files are unchanged, and the previous results are still shown. Use Add My Games to check that the games folder is connected, then scan it again.", widgets::StatusTone::Warning);
                    widgets::technical_details(ui, "simple-refresh-error", |ui| { ui.label(error); });
                } else {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Refresh failed; showing the last known snapshot: {error}"),
                );
                }
                ui.separator();
            }
            if let Some(batch) = app.mount_ui.mount_all.as_ref() {
                stop_mount_all = show_mount_all_progress(ui, &batch.progress);
                ui.separator();
            }
            if let Some(batch) = app.mount_ui.unmount_all.as_ref() {
                stop_unmount_all = show_unmount_all_progress(ui, &batch.progress);
                ui.separator();
            }

            if app.view == MainView::Museum {
                let library = app.database_state.snapshot().map(home_library_snapshot);
                let selected_game = app.museum_selected_game();
                let mut artwork = app.artwork_media.platform_artwork.render_assets();
                if let Some(worker) = app.artwork_media.gamer_cover_worker.as_ref() {
                    for update in worker.drain_delivery() {
                        app.artwork_media.gamer_covers.absorb_delivery(&update);
                        app.artwork_media.gamer_screenshots.absorb_delivery(&update);
                    }
                    for reply in worker.drain() {
                        if !app.artwork_media.gamer_covers.absorb(ui.ctx(), reply.clone()) {
                            app.artwork_media.gamer_screenshots.absorb(ui.ctx(), reply);
                        }
                    }
                }
                let mut screenshot_requests = Vec::new();
                let action = museum_page::show_with_selected_game_and_artwork_with_hero(
                    ui,
                    &mut app.artwork_media.museum_hero,
                    &mut app.artwork_media.museum_page,
                    library.as_ref(),
                    selected_game.as_ref(),
                    Some(&app.artwork_media.gamer_covers),
                    Some(&mut app.artwork_media.gamer_screenshots),
                    Some(&mut artwork),
                    &mut screenshot_requests,
                );
                if !screenshot_requests.is_empty() && app.artwork_media.gamer_cover_worker_allowed {
                    let worker = app.artwork_media.gamer_cover_worker.get_or_insert_with(|| {
                        crate::gamer_artwork::CoverWorker::start(
                            ui.ctx().clone(),
                            app.gui_config.source_roots().ok().map(<[PathBuf]>::to_vec),
                            app.artwork_media.es_de_media.snapshot().cloned(),
                            app.artwork_media.launchbox_local_media.snapshot().cloned(),
                        )
                    });
                    let generation = app.artwork_media.gamer_covers.generation();
                    for job in screenshot_requests {
                        worker.request(generation, job);
                    }
                }
                match action {
                    Some(museum_page::MuseumAction::BrowseLibraryForPlatform(_)) => {
                        app.navigate_to_library_tab(LibraryTab::Archives);
                    }
                    Some(museum_page::MuseumAction::OpenEmulatorSetup) => {
                        app.navigate_to_main_view(MainView::EmulatorSetup);
                    }
                    Some(museum_page::MuseumAction::OpenSelectedEvidence) => {
                        app.navigate_to_main_view(MainView::Selected);
                    }
                    Some(museum_page::MuseumAction::OpenCheats(path)) => {
                        app.open_cheats_mods_workspace(context, path);
                    }
                    Some(museum_page::MuseumAction::OpenRomm) => {
                        app.navigate_to_sources_tab(SourcesTab::Libraries);
                    }
                    Some(museum_page::MuseumAction::OpenDiscConversion) => {
                        app.navigate_to_main_view(MainView::DiscConversion);
                    }
                    None => {}
                }
                return;
            }

            if app.view == MainView::CheckGames {
                app.show_dat_sources_page_mode(ui, false);
                return;
            }
            if app.view == MainView::Home && app.ui_mode == GuiMode::Simple {
                if let Some(target) = crate::simple_mode::show_home(ui) {
                    app.navigate_to_main_view(target);
                }
                return;
            }
            if app.view == MainView::Home {
                let source_folder_count = app
                    .gui_config
                    .source_roots()
                    .map(|roots| roots.len())
                    .unwrap_or(0);
                let has_database = app.database_state.snapshot().is_some();
                // `config_missing` (banner only) still comes from the
                // background setup diagnostics; the "Set up emulators"
                // card's readiness comes from `doctor_scan` - the same
                // state its "Open Doctor" action lands on.
                let config_missing = match &app.doctor_repair.diagnostics {
                    DiagnosticsState::Ready { report, .. } => report.config_missing,
                    DiagnosticsState::Loading { .. } | DiagnosticsState::Error { .. } => false,
                };
                let setup_check = setup_check_summary(&app.doctor_repair.doctor_scan);
                let first_run = missing_config_is_first_run(app.doctor_repair.config_previously_confirmed);
                // Never triggers the load these pages themselves start
                // on first visit - `None` here means "not visited yet
                // this session", not "not configured".
                let cheat_sources_enabled_count = app
                    .sources_ui
                    .cheat_sources_page
                    .as_ref()
                    .map(|page| page.enabled_source_count());
                let dat_sources_registered_count = app
                    .sources_ui
                    .dat_sources_page
                    .as_ref()
                    .map(|page| page.registered_source_count());
                let romm_state_label = app
                    .romm_ui.snapshot
                    .as_ref()
                    .map(|snapshot| romm_readiness_label(&snapshot.status.state));

                let home_inputs = home_page::HomeInputs {
                    source_folder_count,
                    has_database,
                    setup_check,
                    config_missing,
                    first_run,
                    cheat_sources_enabled_count,
                    dat_sources_registered_count,
                    romm_state_label,
                };
                let home_view = home_page::build_home_view(&home_inputs);
                if let Some(card) = home_page::show_home_page(ui, &home_view) {
                    app.sources_ui.quick_rename_mode = card == home_page::HomeCard::QuickRename;
                    app.navigate_to_home_card(card);
                }
            }

            if app.view == MainView::NeedsAttention {
                if let Some(destination) = needs_attention::show_needs_attention_page(
                    ui,
                    &mut app.needs_attention,
                ) {
                    app.navigate_attention(destination);
                }
                return;
            }

            if let Some(tab) = sources_tab_for_main_view(app.view) {
                app.show_sources_page(context, ui, tab);
                return;
            }

            if app.view == MainView::CanonicalOrganisation {
                app.show_rom_organisation_page(ui);
                return;
            }

            if app.view == MainView::PublisherProfiles {
                app.show_publisher_profile_page(ui);
                return;
            }

            if app.view == MainView::IdentifyRename {
                app.show_identify_rename_page(ui);
                return;
            }

            if app.view == MainView::LibraryViewHistory {
                app.show_library_view_history_page(ui);
                return;
            }

            if app.view == MainView::MediaSets {
                if let Some(snapshot) = app.database_state.snapshot() {
                    app.sources_ui.media_sets_page
                        .refresh(&snapshot.archives, app.database_generation.0);
                }
                media_sets_page::show_media_sets_page(ui, &mut app.sources_ui.media_sets_page);
                return;
            }

            if app.view == MainView::CheatsMods {
                // Manual QA finding: opening Cheats & Mods from
                // Gamer View's selected-game panel had no obvious way
                // back - only this page's own "Open Library" button,
                // deep in its content, which isn't a substitute for a
                // clear, always-visible "back to games" affordance in
                // the mode whose entire premise is "no navigation
                // puzzle." Advanced View is unaffected: it still
                // reaches this page only through the sidebar and
                // already has its own established navigation.
                if app.ui_mode == GuiMode::GamerView
                    && ui.button("\u{2190} Back to games").clicked()
                {
                    app.view = MainView::Library;
                }
                let play_target = if app.cheat_workflow.is_some() {
                    let live_for_play_target = match &app.state {
                        LoadState::Ready(data) => Some(data.as_ref()),
                        LoadState::Loading { previous, .. } => previous.as_deref(),
                        LoadState::Error(_) => None,
                    };
                    match launch_readiness_page::gamer_play_action(
                        &app.build_launch_readiness_input(live_for_play_target),
                    ) {
                        launch_readiness_page::GamerPlayAction::Launch(request) => {
                            Some(request.adapter_name())
                        }
                        launch_readiness_page::GamerPlayAction::BlockedTyped(_) => None,
                    }
                } else {
                    None
                };
                let live = match &app.state {
                    LoadState::Ready(data) => Some(data.as_ref()),
                    LoadState::Loading { previous, .. } => previous.as_deref(),
                    LoadState::Error(_) => None,
                };
                let retroarch_route = app.cheat_workflow.as_ref().is_some_and(|workflow| {
                    workflow.adapter == CheatEmulatorAdapter::RetroArch
                });
                let dolphin_route = app
                    .cheat_workflow
                    .as_ref()
                    .is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Dolphin);
                let now_unix_seconds = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_secs());
                let bsfree_context = app.cheat_workflow.as_ref().map(|workflow| {
                    (
                        workflow.archive_path.clone(),
                        workflow.display_name.clone(),
                        workflow.platform.clone().unwrap_or_default(),
                    )
                });
                let cheatbase_seed = app.cheat_workflow.as_ref().map(|workflow| {
                    cheatbase_page::CheatBaseGameSeed {
                        title: workflow.display_name.clone(),
                        platform: workflow.platform.clone(),
                        region: workflow.region.clone(),
                    }
                });
                let user_cheat_library = live
                    .map(|data| {
                        data.records
                            .iter()
                            .map(|record| archivefs_core::patch_manager::UserCheatLibraryGame {
                                game_id: record.mount_plan.archive.path.display().to_string(),
                                title: record
                                    .metadata
                                    .title
                                    .clone()
                                    .unwrap_or_else(|| record.identity.display_name.clone()),
                                platform: record
                                    .metadata
                                    .platform
                                    .clone()
                                    .or_else(|| record.identity.platform.clone()),
                                region: record
                                    .metadata
                                    .region
                                    .clone()
                                    .or_else(|| record.identity.region.clone()),
                                serial: None,
                                title_id: None,
                                crc: None,
                                content_hash: record.identity.content_hash.clone(),
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let user_cheat_selected = app.cheat_workflow.as_ref().map(|workflow| {
                    (
                        workflow.archive_path.display().to_string(),
                        workflow.display_name.clone(),
                    )
                });
                let local_cheat_install_context = app
                    .cheat_workflow
                    .as_ref()
                    .filter(|workflow| workflow.adapter == CheatEmulatorAdapter::RetroArch)
                    .map(|workflow| {
                        local_cheat_install_context(workflow, &app.emulator_readiness.retroarch_profiles)
                    });
                let local_pcsx2_install_context = app
                    .cheat_workflow
                    .as_ref()
                    .filter(|workflow| workflow.adapter == CheatEmulatorAdapter::Pcsx2)
                    .and_then(|workflow| {
                        local_pcsx2_install_context(workflow, &app.emulator_readiness.pcsx2_profiles)
                    });
                let local_dolphin_install_context = app
                    .cheat_workflow
                    .as_ref()
                    .filter(|workflow| workflow.adapter == CheatEmulatorAdapter::Dolphin)
                    .and_then(|workflow| {
                        local_dolphin_install_context(workflow, &app.emulator_readiness.dolphin_profiles)
                    });
                let local_xenia_install_context = app
                    .cheat_workflow
                    .as_ref()
                    .filter(|workflow| workflow.adapter == CheatEmulatorAdapter::Xenia)
                    .map(|workflow| local_xenia_install_context(workflow, &app.emulator_readiness.xenia_profiles));
                let (action, catalogue_action, dolphin_catalogue_action, bsfree_action, cheatbase_action) = ui_layout::page(
                    ui,
                    ui_layout::ContentWidth::Wide,
                    true,
                    "cheats_mods_workspace_scroll",
                    |ui| {
                        // Drained before the workspace renders, so a
                        // running install/undo is reflected in this
                        // same frame's render, not one frame late.
                        if app.dolphin_texture_mod.pcsx2_texture_mod.poll() || app.dolphin_texture_mod.pcsx2_texture_mod.is_busy() || app.dolphin_texture_mod.poll() || app.dolphin_texture_mod.is_busy()
                        {
                            ui.ctx().request_repaint();
                        }
                        if app.local_mod_package.poll() || app.local_mod_package.is_busy() {
                            ui.ctx().request_repaint();
                        }
                        if let Some(workflow) = app.cheat_workflow.as_ref() {
                            show_cheat_play_target_warning(
                                ui,
                                workflow.adapter,
                                play_target,
                            );
                            ui.add_space(theme::SECTION_GAP / 2.0);
                        }
                        let action = show_cheats_mods_page(
                            ui,
                            app.cheat_workflow.as_mut(),
                            &app.emulator_readiness.retroarch_profiles,
                            &app.emulator_readiness.pcsx2_profiles,
                            &app.emulator_readiness.dolphin_profiles,
                            &app.emulator_readiness.xenia_profiles,
                            live,
                            app.database_state.snapshot(),
                            &app.history,
                            busy || app.catalogue_bsfree_ui.catalogue_retrieval.is_some(),
                            &mut app.clipboard,
                            &mut app.dolphin_texture_mod,
                            &mut app.local_mod_package,
                        );
                        // Keep these renderers running each frame: they also
                        // poll existing jobs and invalidate stale selections.
                        // Presentation order, not lifecycle, is changed.
                        ui.add_space(theme::SECTION_GAP);
                        app.user_cheat_import_page.show(
                            ui,
                            &ui.ctx().clone(),
                            &user_cheat_library,
                            user_cheat_selected
                                .as_ref()
                                .map(|(id, title)| (id.as_str(), title.as_str())),
                            local_cheat_install_context.as_ref(),
                            local_pcsx2_install_context.as_ref(),
                            local_dolphin_install_context.as_ref(),
                            local_xenia_install_context.as_ref(),
                        );
                        ui.add_space(theme::SECTION_GAP);
                        let cheatbase_action = cheatbase_page::show_cheatbase_page(
                            ui,
                            &mut app.cheatbase_page,
                            cheatbase_seed,
                        );
                        ui.add_space(theme::SECTION_GAP);
                        let dolphin_catalogue_action = dolphin_route.then(|| {
                            let action = show_dolphin_catalogue_manager(
                                ui,
                                &app.catalogue_bsfree_ui.dolphin_catalogue_manager,
                                app.catalogue_bsfree_ui.dolphin_catalogue_retrieval.as_ref(),
                                app.catalogue_bsfree_ui.dolphin_catalogue_last_result.as_ref(),
                                DolphinCatalogueCardContext {
                                    review: app.catalogue_bsfree_ui.dolphin_catalogue_review,
                                    update_available: app.catalogue_bsfree_ui.dolphin_catalogue_update_available,
                                    remove_confirm: app.catalogue_bsfree_ui.dolphin_catalogue_remove_confirm,
                                    now_unix_seconds,
                                },
                                &mut app.clipboard,
                            );
                            ui.add_space(theme::SECTION_GAP);
                            action
                        }).flatten();
                        ui.add_space(theme::SECTION_GAP);
                        app.cheat_reconciliation_review.show(ui);
                        ui.add_space(theme::SECTION_GAP);
                        let bsfree_action = show_bsfree_game_browser(
                            ui,
                            &app.catalogue_bsfree_ui.bsfree_manager,
                            app.catalogue_bsfree_ui.bsfree_operation.is_some(),
                            &mut app.catalogue_bsfree_ui.bsfree_ui,
                            bsfree_context.as_ref(),
                        );
                        let catalogue_action = retroarch_route.then(|| {
                            ui.add_space(theme::SECTION_GAP);
                            widgets::section_header(
                                ui,
                                "Database and sources",
                                Some(
                                    "Download, update, or verify the trusted RetroArch cheat database without leaving this page.",
                                ),
                            );
                            show_retroarch_catalogue_manager(
                                ui,
                                &app.catalogue_bsfree_ui.catalogue_manager,
                                app.catalogue_bsfree_ui.catalogue_review.as_ref(),
                                app.catalogue_bsfree_ui.catalogue_retrieval.as_ref(),
                                app.catalogue_bsfree_ui.catalogue_last_result.as_ref(),
                                &mut app.clipboard,
                            )
                        }).flatten();
                        (action, catalogue_action, dolphin_catalogue_action, bsfree_action, cheatbase_action)
                    });
                let picker_rows = live
                    .map(|data| {
                        build_display_rows(
                            &data.records,
                            &data.rows,
                            app.database_state.snapshot(),
                        )
                    })
                    .unwrap_or_default();
                if let Some(catalogue_action) = catalogue_action {
                    app.handle_catalogue_manager_action(context, catalogue_action);
                }
                if let Some(dolphin_catalogue_action) = dolphin_catalogue_action {
                    app.handle_dolphin_catalogue_manager_action(context, dolphin_catalogue_action);
                }
                if let Some(bsfree_action) = bsfree_action {
                    app.start_bsfree_operation(context.clone(), bsfree_action);
                }
                if let Some(cheatbase_action) = cheatbase_action {
                    app.cheatbase_page.handle(cheatbase_action, context.clone());
                }
                match action {
                    Some(CheatWorkflowAction::ChooseArchive) => {
                        app.open_cheat_archive_picker();
                    }
                    Some(CheatWorkflowAction::OpenLibrary) => {
                        app.navigate_to_library_tab(LibraryTab::Archives);
                    }
                    Some(CheatWorkflowAction::RescanProfiles) => {
                        app.start_retroarch_profile_scan(context.clone());
                    }
                    Some(CheatWorkflowAction::RescanPcsx2Profiles) => {
                        app.start_pcsx2_profile_scan(context.clone());
                    }
                    Some(CheatWorkflowAction::InspectPcsx2Profile) => {
                        app.start_pcsx2_inventory(context.clone());
                    }
                    Some(CheatWorkflowAction::FetchPcsx2GameHacking { force_refresh }) => {
                        app.start_pcsx2_gamehacking_fetch(context.clone(), force_refresh);
                    }
                    Some(CheatWorkflowAction::ConfirmPcsx2GameHackingMatch { game_id }) => {
                        app.confirm_pcsx2_gamehacking_match(context.clone(), game_id);
                    }
                    Some(CheatWorkflowAction::TogglePcsx2CheatSelected { id, selected }) => {
                        app.update_pcsx2_cheat_selection(&id, selected);
                    }
                    Some(CheatWorkflowAction::InstallSelectedPcsx2) => {
                        app.start_pcsx2_install_preview();
                    }
                    Some(CheatWorkflowAction::FetchGameCubeGameHacking { force_refresh }) => {
                        app.start_gamecube_gamehacking_fetch(context.clone(), force_refresh);
                    }
                    Some(CheatWorkflowAction::ConfirmGameCubeGameHackingMatch { game_id }) => {
                        app.confirm_gamecube_gamehacking_match(context.clone(), game_id);
                    }
                    Some(CheatWorkflowAction::ToggleGameCubeGameHackingCheatSelected {
                        index,
                        selected,
                    }) => {
                        app.update_gamecube_gamehacking_cheat_selection(index, selected);
                    }
                    Some(CheatWorkflowAction::InstallSelectedGameCubeGameHacking) => {
                        app.start_gamecube_gamehacking_install_preview();
                    }
                    Some(CheatWorkflowAction::RemoveSelectedGameCubeGameHacking) => {
                        app.start_gamecube_gamehacking_removal_preview();
                    }
                    Some(CheatWorkflowAction::OpenBrowserImport(platform)) => {
                        app.open_browser_import(platform);
                    }
                    Some(CheatWorkflowAction::CloseBrowserImport) => {
                        app.close_browser_import();
                    }
                    Some(CheatWorkflowAction::OpenGameHackingPageInBrowser) => {
                        app.open_gamehacking_page_in_browser();
                    }
                    Some(CheatWorkflowAction::CopyGameHackingPageUrl) => {
                        app.copy_gamehacking_page_url();
                    }
                    Some(CheatWorkflowAction::ImportBrowserSavedFile) => {
                        app.import_browser_saved_file(context.clone());
                    }
                    Some(CheatWorkflowAction::ToggleBrowserImportPaste(open)) => {
                        if let Some(state) = app
                            .cheat_workflow
                            .as_mut()
                            .and_then(|workflow| workflow.browser_import.as_mut())
                        {
                            state.paste_open = open;
                        }
                    }
                    Some(CheatWorkflowAction::ImportBrowserPastedText) => {
                        app.import_browser_pasted_text(context.clone());
                    }
                    Some(CheatWorkflowAction::ImportBrowserClipboard) => {
                        app.import_browser_clipboard(context.clone());
                    }
                    Some(CheatWorkflowAction::ChooseBrowserImportKind(kind)) => {
                        if let Some(state) = app
                            .cheat_workflow
                            .as_mut()
                            .and_then(|workflow| workflow.browser_import.as_mut())
                        {
                            state.kind = kind;
                        }
                    }
                    Some(CheatWorkflowAction::FetchBsFreeGameCube { search_title }) => {
                        app.start_bsfree_gamecube_search(context.clone(), search_title);
                    }
                    Some(CheatWorkflowAction::ConfirmBsFreeGameCubeMatch { upstream_uid }) => {
                        app.start_bsfree_gamecube_confirm(context.clone(), upstream_uid);
                    }
                    Some(CheatWorkflowAction::ToggleBsFreeGameCubeCheatSelected {
                        index,
                        selected,
                    }) => {
                        app.update_bsfree_gamecube_cheat_selection(index, selected);
                    }
                    Some(CheatWorkflowAction::SelectAllBsFreeGameCubeCheats) => {
                        app.update_bsfree_gamecube_cheat_selection_all(true);
                    }
                    Some(CheatWorkflowAction::ClearAllBsFreeGameCubeCheats) => {
                        app.update_bsfree_gamecube_cheat_selection_all(false);
                    }
                    Some(CheatWorkflowAction::InstallSelectedBsFreeGameCube) => {
                        app.start_bsfree_gamecube_install_preview();
                    }
                    Some(CheatWorkflowAction::FetchBsFreeWii { search_title }) => {
                        app.start_bsfree_wii_search(context.clone(), search_title);
                    }
                    Some(CheatWorkflowAction::ConfirmBsFreeWiiMatch { upstream_uid }) => {
                        app.start_bsfree_wii_confirm(context.clone(), upstream_uid);
                    }
                    Some(CheatWorkflowAction::ToggleBsFreeWiiCheatSelected {
                        index,
                        selected,
                    }) => {
                        app.update_bsfree_wii_cheat_selection(index, selected);
                    }
                    Some(CheatWorkflowAction::SelectAllBsFreeWiiCheats) => {
                        app.update_bsfree_wii_cheat_selection_all(true);
                    }
                    Some(CheatWorkflowAction::ClearAllBsFreeWiiCheats) => {
                        app.update_bsfree_wii_cheat_selection_all(false);
                    }
                    Some(CheatWorkflowAction::InstallSelectedBsFreeWii) => {
                        app.start_bsfree_wii_install_preview();
                    }
                    Some(CheatWorkflowAction::RescanDolphinProfiles) => {
                        app.start_dolphin_profile_scan(context.clone());
                    }
                    Some(CheatWorkflowAction::InspectDolphinProfile) => {
                        app.start_dolphin_inventory(context.clone());
                    }
                    Some(CheatWorkflowAction::InspectExistingLibrary) => {
                        app.start_existing_retroarch_library_inspection(context.clone());
                    }
                    Some(CheatWorkflowAction::RefreshSources) => {
                        app.start_cheat_source_list(context.clone());
                    }
                    Some(CheatWorkflowAction::ManageCatalogue) => {
                        app.view = MainView::Sources;
                        app.start_catalogue_status_load(context.clone());
                    }
                    Some(CheatWorkflowAction::UseCachedSnapshot) => {
                        app.start_cheat_source_fetch(context.clone(), true);
                    }
                    Some(CheatWorkflowAction::ReviewApply) => {
                        app.review_cheat_apply();
                    }
                    Some(CheatWorkflowAction::ConfirmApply) => {
                        app.start_cheat_apply(context.clone());
                    }
                    Some(CheatWorkflowAction::CancelApply) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.transaction = CheatTransactionState::Idle;
                            workflow.transaction_notice = Some(
                                "Installation cancelled before apply; no live emulator file was changed."
                                    .to_string(),
                            );
                        }
                        app.history.record(HistoryEntry::new(
                            ActivityAction::CheatInstall,
                            app.cheat_workflow
                                .as_ref()
                                .map(|workflow| workflow.archive_path.clone()),
                            ActivityOutcome::Cancelled,
                            "Install cancelled before the write phase; nothing was changed.",
                        ));
                    }
                    Some(CheatWorkflowAction::MatchCandidates) => {
                        app.start_cheat_candidate_match(context.clone());
                    }
                    Some(CheatWorkflowAction::SelectCandidate(relative_path)) => {
                        app.apply_cheat_candidate_choice(&relative_path);
                    }
                    Some(CheatWorkflowAction::ClearCandidateChoice) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.candidate_selection = None;
                            workflow.candidate_load_error = None;
                            workflow.preview = CheatStepResource::NotLoaded;
                            workflow.preview_request = None;
                            workflow.transaction = CheatTransactionState::Idle;
                        }
                    }
                    Some(CheatWorkflowAction::ToggleCheatSelected { index, selected }) => {
                        app.update_cheat_selection(|selection| {
                            selection.set_selected(index, selected);
                        });
                    }
                    Some(CheatWorkflowAction::ToggleCheatEnabled { index, enabled }) => {
                        app.update_cheat_selection(|selection| {
                            selection.set_enabled(index, enabled);
                        });
                    }
                    Some(CheatWorkflowAction::SelectAllCheats) => {
                        app.update_cheat_selection(CheatSelection::select_all);
                    }
                    Some(CheatWorkflowAction::ClearAllCheats) => {
                        app.update_cheat_selection(CheatSelection::clear_all);
                    }
                    Some(CheatWorkflowAction::BuildInstallPreview) => {
                        app.start_generated_cheat_preview(context.clone());
                    }
                    Some(CheatWorkflowAction::RollbackInstall) => {
                        app.start_cheat_install_rollback(context.clone());
                    }
                    Some(CheatWorkflowAction::FetchDolphinProvider { force_refresh }) => {
                        app.start_dolphin_provider_fetch(context.clone(), force_refresh);
                    }
                    Some(CheatWorkflowAction::RescanXeniaProfiles) => {
                        app.start_xenia_profile_scan();
                    }
                    Some(CheatWorkflowAction::FetchXeniaProvider { force_refresh }) => {
                        app.start_xenia_provider_fetch(context.clone(), force_refresh);
                    }
                    Some(CheatWorkflowAction::SelectXeniaCandidate(index)) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.xenia_selected_candidate_index = Some(index);
                            workflow.xenia_selection = None;
                            workflow.xenia_destination_error = None;
                            workflow.preview = CheatStepResource::NotLoaded;
                            workflow.preview_request = None;
                            workflow.transaction = CheatTransactionState::Idle;
                        }
                    }
                    Some(CheatWorkflowAction::ClearXeniaCandidateChoice) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.xenia_selected_candidate_index = None;
                            workflow.xenia_selection = None;
                            workflow.xenia_destination_error = None;
                            workflow.preview = CheatStepResource::NotLoaded;
                            workflow.preview_request = None;
                            workflow.transaction = CheatTransactionState::Idle;
                        }
                    }
                    Some(CheatWorkflowAction::AcknowledgeXeniaPartialVerification(
                        acknowledged,
                    )) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut()
                            && let Some(state) = workflow.xenia_selection.as_mut()
                        {
                            state.selection.partial_verification_acknowledged = acknowledged;
                        }
                    }
                    Some(CheatWorkflowAction::ToggleXeniaPatchSelected {
                        index,
                        selected,
                    }) => {
                        app.update_xenia_patch_selection(|selection| {
                            selection.set_selected(index, selected);
                        });
                    }
                    Some(CheatWorkflowAction::SelectAllXeniaPatches) => {
                        app.update_xenia_patch_selection(XeniaPatchSelection::select_all);
                    }
                    Some(CheatWorkflowAction::ClearAllXeniaPatches) => {
                        app.update_xenia_patch_selection(XeniaPatchSelection::clear_all);
                    }
                    Some(CheatWorkflowAction::BuildXeniaInstallPreview) => {
                        app.start_xenia_install_preview();
                    }
                    Some(CheatWorkflowAction::ToggleDolphinCodeSelected {
                        index,
                        selected,
                    }) => {
                        app.update_dolphin_code_selection(|selection| {
                            selection.set_selected(index, selected);
                        });
                    }
                    Some(CheatWorkflowAction::SelectAllDolphinCodes) => {
                        app.update_dolphin_code_selection(
                            DolphinProviderCodeSelection::select_all,
                        );
                    }
                    Some(CheatWorkflowAction::ClearAllDolphinCodes) => {
                        app.update_dolphin_code_selection(
                            DolphinProviderCodeSelection::clear_all,
                        );
                    }
                    Some(CheatWorkflowAction::BuildDolphinInstallPreview) => {
                        app.start_dolphin_install_preview();
                    }
                    Some(CheatWorkflowAction::ChooseDolphinProfile(profile_id)) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.dolphin_profile_choice = Some(profile_id);
                        }
                        app.confirm_dolphin_profile_choice();
                    }
                    Some(CheatWorkflowAction::ChooseXeniaProfile(profile_id)) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.xenia_profile_choice = Some(profile_id);
                        }
                        app.confirm_xenia_profile_choice();
                    }
                    Some(CheatWorkflowAction::InstallSelectedDolphin) => {
                        app.start_beginner_install_dolphin();
                    }
                    Some(CheatWorkflowAction::InstallSelectedXenia) => {
                        app.start_beginner_install_xenia();
                    }
                    Some(CheatWorkflowAction::ToggleDolphinShowExactChanges(show)) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.dolphin_show_exact_changes = show;
                        }
                    }
                    Some(CheatWorkflowAction::ToggleXeniaShowExactChanges(show)) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.xenia_show_exact_changes = show;
                        }
                    }
                    Some(CheatWorkflowAction::ToggleDolphinDetailsOpen(open)) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.dolphin_details_open = open;
                        }
                    }
                    Some(CheatWorkflowAction::ToggleXeniaDetailsOpen(open)) => {
                        if let Some(workflow) = app.cheat_workflow.as_mut() {
                            workflow.xenia_details_open = open;
                        }
                    }
                    Some(CheatWorkflowAction::OpenApplyHistory) => {
                        app.shared_history_operation = app
                            .cheat_workflow
                            .as_ref()
                            .and_then(|workflow| match &workflow.transaction {
                                CheatTransactionState::Result { result, .. } => {
                                    Some(result.journal.operation_id.clone())
                                }
                                _ => None,
                            });
                        app.shared_history = SharedHistoryState::NotLoaded;
                        app.view = MainView::HistoryLogs;
                    }
                    None => {}
                }
                let picker_action = app.cheat_archive_picker.as_mut().and_then(|picker| {
                    show_cheat_archive_picker(
                        context,
                        picker,
                        &picker_rows,
                        &mut app.library_ui.library_filters.platform,
                        &mut app.clipboard,
                    )
                });
                match picker_action {
                    Some(CheatArchivePickerAction::Cancel) => {
                        app.cheat_archive_picker = None;
                    }
                    Some(CheatArchivePickerAction::Select(path)) => {
                        let requires_confirmation = cheat_archive_change_requires_confirmation(
                            app.cheat_workflow.as_ref(),
                            &path,
                        );
                        app.cheat_archive_picker = None;
                        if requires_confirmation {
                            app.confirm_cheat_archive_change = Some(path);
                        } else {
                            app.apply_cheat_archive_choice(context, path);
                        }
                    }
                    None => {}
                }
                if let Some(path) = app.confirm_cheat_archive_change.clone() {
                    let candidate_still_exists = picker_rows
                        .iter()
                        .any(|row| row.origin == RowOrigin::Live && row.path == path);
                    egui::Window::new("Change Cheats & Mods archive?")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .show(context, |ui| {
                            ui.label("The current archive has catalogue retrieval state. Changing archive discards that page state; it does not alter either archive or RetroArch.");
                            ui.label(path.display().to_string());
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    app.confirm_cheat_archive_change = None;
                                }
                                if ui
                                    .add_enabled(
                                        candidate_still_exists,
                                        egui::Button::new("Change archive"),
                                    )
                                    .clicked()
                                {
                                    app.apply_cheat_archive_choice(context, path.clone());
                                }
                            });
                        });
                }
                return;
            }

            if app.view == MainView::Mount {
                let live = match &app.state {
                    LoadState::Ready(data) => Some(data.as_ref()),
                    _ => None,
                };
                let action = show_mount_page(
                    ui,
                    live,
                    app.mount_ui.mount_all_result.as_ref(),
                    MountPageViewState {
                        queue: &mut app.mount_ui.mount_queue,
                        search: &mut app.mount_ui.mount_search,
                        platform: &mut app.library_ui.library_filters.platform,
                        confirm: &mut app.mount_ui.confirm_mount_queue,
                        busy: archive_actions_blocked,
                        block_reason: archive_action_block_reason,
                    },
                );
                app.handle_mount_page_action(context, action);
                return;
            }

            if app.view == MainView::Selected {
                let action = app.show_game_details(
                    context,
                    ui,
                    archive_actions_blocked,
                    archive_action_block_reason,
                );
                app.handle_mount_page_action(context, action);
                return;
            }

            if app.view == MainView::ActiveMounts {
                let live_records = match &app.state {
                    LoadState::Ready(data) => Some(data.records.as_slice()),
                    _ => None,
                };
                let action = show_active_mounts_page(
                    ui,
                    live_records,
                    &mut app.mount_ui.active_mounts_confirm_unmount,
                    &mut app.mount_ui.cleanup_after_unmount,
                    app.feedback.as_ref(),
                    archive_actions_blocked,
                );
                match action {
                    Some(ActiveMountsPageAction::Unmount(archive_path)) => {
                        requested_action =
                            Some(AppOperationRequest::Archive(OperationRequest {
                                action: ArchiveAction::Unmount,
                                archive_path,
                                cleanup_after_unmount: app.mount_ui.cleanup_after_unmount,
                            }));
                    }
                    Some(ActiveMountsPageAction::OpenInLibrary(path)) => {
                        app.navigate_to_library_tab(LibraryTab::Archives);
                        app.archive_context.select_only(path);
                    }
                    Some(ActiveMountsPageAction::Refresh) => app.refresh(context),
                    None => {}
                }
                ui.add_space(theme::SECTION_GAP);
                show_active_mounts_recent_activity(ui, &app.history);
                return;
            }

            // First-class workflow destinations - rendered standalone,
            // never wrapped in Problems & Repair chrome (see 0.8.1's
            // "core workflows directly discoverable" pass). Home, the
            // sidebar and the top menu all route straight here.
            if app.view == MainView::ExactDuplicateReview {
                widgets::page_header_with_icon(
                    ui,
                    crate::ui::icons::CHECK,
                    "Duplicate Finder",
                    "Find identical or equivalent copies in your library, keep one, and move \
                     the rest into a recoverable quarantine. Nothing is permanently deleted.",
                );
                ui.add_space(theme::SECTION_GAP);
                app.show_exact_duplicate_review_page(ui);
                return;
            }

            if app.view == MainView::DiscConversion {
                app.show_optical_conversion_page(ui);
                return;
            }

            if app.view == MainView::StorageHealth {
                if let Some(snapshot) = app.database_state.snapshot() {
                    app.storage_health_page.show(ui, &snapshot.archives);
                } else {
                    ui.heading("Storage Health");
                    ui.label("Storage analysis is not available until the library catalogue is loaded.");
                }
                return;
            }

            if app.view == MainView::TapeInspector {
                let selected_path = app.archive_context.focused.clone();
                if let Some(path) = selected_path
                    .as_ref()
                    .filter(|path| tape_analysis_page::is_tape_path(path))
                {
                    let evidence_is_stale = match &app.selected_evidence_ui.selected_evidence {
                        selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                            report.path != *path
                        }
                        selected_evidence_page::SelectedEvidenceState::Loading {
                            path: loading_path, ..
                        } => loading_path != path,
                        selected_evidence_page::SelectedEvidenceState::Idle => true,
                        selected_evidence_page::SelectedEvidenceState::Error {
                            path: error_path, ..
                        } => error_path != path,
                    };
                    if evidence_is_stale {
                        app.start_selected_evidence_load(context.clone(), path.clone());
                    }
                }
                let (analysis, analysis_error) = match &app.selected_evidence_ui.selected_evidence {
                    selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                        if Some(report.path.as_path()) == selected_path.as_deref() =>
                    {
                        (report.tape_analysis.as_ref(), None)
                    }
                    selected_evidence_page::SelectedEvidenceState::Error {
                        path: error_path,
                        message,
                        ..
                    } if Some(error_path.as_path()) == selected_path.as_deref() => {
                        (None, Some(message.as_str()))
                    }
                    _ => (None, None),
                };
                let live_records = match &app.state {
                    LoadState::Ready(data) => Some(data.records.as_slice()),
                    _ => None,
                };
                let tape_action = tape_analysis_page::show_page_with_error(
                    ui,
                    selected_path
                        .as_deref()
                        .filter(|path| tape_analysis_page::is_tape_path(path)),
                    analysis,
                    analysis_error,
                    live_records,
                    &mut app.tape_inspector_filter,
                );
                if let Some(
                    tape_analysis_page::TapeInspectorAction::ChooseFile(path)
                    | tape_analysis_page::TapeInspectorAction::SelectLibraryTape(path),
                ) = tape_action
                {
                    app.archive_context.select_only(path);
                }
                return;
            }

            if app.view == MainView::EmulatorSetup {
                app.show_emulator_setup_page(ui, context);
                return;
            }

            if app.view == MainView::ReadyToPlay {
                let selected_identity = app
                    .archive_context
                    .focused
                    .as_ref()
                    .map(|path| path.display().to_string());
                let live = match &app.state {
                    LoadState::Ready(data) => Some(data.as_ref()),
                    _ => None,
                };
                let results = selected_identity
                    .map(|identity| {
                        let input = app.build_launch_readiness_input(live);
                        launch_readiness_page::ready_to_play_result(&input, identity)
                    })
                    .into_iter()
                    .collect();
                app.emulator_readiness.ready_to_play_page.set_results(results);
                app.emulator_readiness.ready_to_play_page.show(ui);
                return;
            }

            if app.view == MainView::EmulatorInventory {
                let emulator_running = app.launch_dolphin.is_active()
                    || app.launch_pcsx2.is_active()
                    || app.launch_standalone.is_active();
                app.emulator_readiness.emulator_inventory_page.show(ui, emulator_running);
                return;
            }

            if app.view == MainView::BiosProjection {
                app.emulator_readiness.bios_projection_page.show(ui);
                if app
                    .emulator_readiness
                    .bios_projection_page
                    .take_doctor_refresh_request()
                {
                    app.start_doctor_scan(context.clone());
                }
                return;
            }

            if problems_repair_tab_for_main_view(app.view).is_some() {
                app.show_problems_repair_page(ui, context);
                return;
            }

            if app.view == MainView::HistoryLogs {
                let history_action = show_history_logs_page(
                    ui,
                    &app.shared_history,
                    &mut app.shared_rollback,
                    app.shared_history_operation.as_deref(),
                    &mut app.history,
                    &mut app.history_filters,
                    &mut app.clipboard,
                    &mut app.doctor_repair.database_restore_plan,
                    &mut app.doctor_repair.database_restore_confirmation,
                    &mut app.doctor_repair.database_restore_feedback,
                    app.database_state.is_loading() || busy,
                );
                match history_action {
                    Some(HistoryPageAction::PreviewRollback {
                        journal_path,
                        destination_root,
                    }) => app.start_shared_rollback_preview(
                        context.clone(),
                        journal_path,
                        destination_root,
                    ),
                    Some(HistoryPageAction::ConfirmRollback) => {
                        app.start_shared_rollback(context.clone());
                    }
                    Some(HistoryPageAction::CancelRollback) => {
                        app.shared_rollback = SharedRollbackState::Idle;
                    }
                    Some(HistoryPageAction::Refresh) => {
                        app.shared_history = SharedHistoryState::NotLoaded;
                    }
                    Some(HistoryPageAction::ReviewDatabaseRestore { backup_path }) => {
                        match default_database_path()
                            .and_then(|live| archivefs_core::prepare_database_restore(live, backup_path))
                        {
                            Ok(plan) => {
                                app.doctor_repair.database_restore_plan = Some(plan);
                                app.doctor_repair.database_restore_confirmation.clear();
                                app.doctor_repair.database_restore_feedback = None;
                            }
                            Err(error) => app.doctor_repair.database_restore_feedback = Some(error.to_string()),
                        }
                    }
                    Some(HistoryPageAction::ExecuteDatabaseRestore) => {
                        if app.database_state.is_loading() || app.is_busy() {
                            app.doctor_repair.database_restore_feedback = Some("Database is busy loading or scanning; restore remains blocked until it is idle.".into());
                        } else if let Some(plan) = app.doctor_repair.database_restore_plan.clone() {
                            match archivefs_core::restore_database(&plan, &app.doctor_repair.database_restore_confirmation) {
                                Ok(result) => {
                                    app.doctor_repair.database_restore_feedback = Some(format!("{} Emergency backup: {}", result.receipt.message, result.emergency_backup_path.display()));
                                    app.doctor_repair.database_restore_plan = None;
                                    app.doctor_repair.database_restore_confirmation.clear();
                                    app.database_generation = app.database_generation.next();
                                    let generation = app.database_generation;
                                    let previous = app.database_state.snapshot().cloned().map(Box::new);
                                    app.database_state = start_database_load(context.clone(), generation, previous, false);
                                    app.shared_history = SharedHistoryState::NotLoaded;
                                }
                                Err(error) => app.doctor_repair.database_restore_feedback = Some(error.to_string()),
                            }
                        }
                    }
                    None => {}
                }
                return;
            }

            if app.view == MainView::Settings {
                app.artwork_media.platform_artwork.prepare_settings(context);
                let mount_root = match &app.state {
                    LoadState::Ready(data) => Some(data.mount_root.as_path()),
                    _ => None,
                };
                let action = show_settings_page(
                    ui,
                    &app.database_state,
                    &app.doctor_repair.diagnostics,
                    &app.emulator_readiness.retroarch_profiles,
                    mount_root,
                    busy,
                    &mut app.clipboard,
                    &mut app.artwork_media.platform_artwork,
                    &mut app.screenscraper_page,
                );
                match action {
                    Some(SettingsPageAction::OpenConfigFolder) => {
                        app.start_setup_action(context.clone(), SetupAction::OpenConfigFolder);
                    }
                    Some(SettingsPageAction::ValidateConfiguration) => {
                        app.refresh_diagnostics(context);
                    }
                    Some(SettingsPageAction::OpenDiagnostics) => {
                        app.tools_overlay = ToolsOverlay::Diagnostics;
                        app.refresh_diagnostics(context);
                    }
                    Some(SettingsPageAction::RescanRetroArchProfiles) => {
                        app.start_retroarch_profile_scan(context.clone());
                    }
                    Some(SettingsPageAction::RunFirstTimeSetupAgain) => {
                        app.restart_onboarding();
                    }
                    Some(SettingsPageAction::PlatformArtwork(action)) => {
                        app.artwork_media
                            .platform_artwork
                            .dispatch(context.clone(), action);
                    }
                    None => {}
                }
                return;
            }

            if app.view == MainView::About {
                let mount_root = match &app.state {
                    LoadState::Ready(data) => Some(data.mount_root.as_path()),
                    _ => None,
                };
                show_about_contents(
                    ui,
                    &app.database_state,
                    &app.doctor_repair.diagnostics,
                    mount_root,
                    &mut app.clipboard,
                );
                return;
            }

            // The unified Library shell: one heading and one tab
            // selector shared by all five Library-related
            // destinations, dispatching to each tab's existing,
            // otherwise-unmodified content. `library_tab_for_main_view`
            // covers the Library-related MainView variants (see its doc
            // comment), so this replaces what
            // used to be separate `if app.view == MainView::X` blocks.
            //
            // The Archives arm deliberately does *not* `return`:
            // falling through to the existing `match &app.state`
            // block below is exactly how MainView::Library already
            // reached it before this shell existed. The other three
            // arms `return` after rendering, exactly as their own
            // standalone `if` blocks used to.
            if library_tab_for_main_view(app.view).is_some() {
                let mut add_folder = false;
                let clicked_tab = if app.ui_mode == GuiMode::GamerView {
                    show_library_shell_header(ui, app.library_tab)
                } else {
                    navigation::show_library_shell_header_with_actions(ui, app.library_tab, |ui| {
                    add_folder = widgets::action_button(ui, "Add Game Folder", widgets::ActionStyle::Primary, true).clicked();
                    })
                };
                if let Some(clicked_tab) = clicked_tab {
                    app.navigate_to_library_tab(clicked_tab);
                }
                if add_folder {
                    app.navigate_to_sources_tab(SourcesTab::Libraries);
                }

                match app.library_tab {
                    LibraryTab::Archives => {}
                    LibraryTab::Health => {
                        // `cached_health_issues` needs `&mut self` (it
                        // may rebuild and store the cache); `.to_vec()`
                        // copies the small already-built
                        // `Vec<HealthIssue>` out and ends that mutable
                        // borrow immediately, so the immutable borrows
                        // of `app.database_state`/`app.state` just
                        // below (for the much larger
                        // `LoadedData`/`CachedLibrarySnapshot`, passed
                        // by reference rather than cloned) never
                        // conflict with it.
                        let issues = app.cached_health_issues().to_vec();
                        if let Some(snapshot) = app.database_state.snapshot() {
                            let live_data = match &app.state {
                                LoadState::Ready(data) => Some(data.as_ref()),
                                _ => None,
                            };
                            health_dashboard_action = show_health_dashboard_panel(
                                ui,
                                live_data,
                                snapshot,
                                &issues,
                                HealthDashboardViewState {
                                    filters: &mut app.health_duplicate_ui.health_filters,
                                    sort_field: &mut app.health_duplicate_ui.health_sort_field,
                                    sort_ascending: &mut app.health_duplicate_ui.health_sort_ascending,
                                    selected_issue: &mut app.health_duplicate_ui.selected_health_issue,
                                    busy: archive_actions_blocked,
                                    clipboard: &mut app.clipboard,
                                },
                            );
                        } else {
                            ui.label("Scan the library to see the health dashboard.");
                        }
                        return;
                    }
                    LibraryTab::Duplicates => {
                        if let Some(snapshot) = app.database_state.snapshot() {
                            match show_duplicate_review_panel(
                                ui,
                                &snapshot.duplicate_report,
                                DuplicateReviewViewState {
                                    filters: &mut app.health_duplicate_ui.duplicate_filters,
                                    sort_field: &mut app.health_duplicate_ui.duplicate_sort_field,
                                    sort_ascending: &mut app.health_duplicate_ui.duplicate_sort_ascending,
                                    selected_group: &mut app.health_duplicate_ui.selected_duplicate_group,
                                    selected_archive: &mut app.health_duplicate_ui.selected_duplicate_archive,
                                    clipboard: &mut app.clipboard,
                                },
                            ) {
                                Some(DuplicateReviewAction::Close) => {
                                    app.navigate_to_library_tab(LibraryTab::Archives);
                                }
                                Some(DuplicateReviewAction::ViewInLibrary(path)) => {
                                    app.navigate_to_library_tab(LibraryTab::Archives);
                                    app.archive_context.select_only(path);
                                }
                                Some(DuplicateReviewAction::Inspect(path)) => {
                                    app.start_archive_inspection(context.clone(), path);
                                }
                                None => {}
                            }
                        } else {
                            ui.label("Scan the library to review duplicates.");
                        }
                        return;
                    }
                    LibraryTab::Views => {
                        let all_source_folders = app
                            .database_state
                            .snapshot()
                            .map(|snapshot| snapshot.source_views.as_slice())
                            .unwrap_or(&[]);
                        let library_view_action = show_library_views_page(
                            ui,
                            &app.library_views,
                            all_source_folders,
                            app.library_view_action.is_some(),
                            app.library_view_last_plan.as_ref(),
                            app.library_view_focus_archive.as_deref(),
                            &mut app.library_view_plan_filter,
                            &mut app.library_view_form_dialog,
                            &mut app.library_view_remove_dialog,
                            &mut app.clipboard,
                        );
                        if let Some(library_view_action) = library_view_action {
                            app.start_library_view_action(
                                context.clone(),
                                library_view_action,
                            );
                        }
                        return;
                    }
                    LibraryTab::RecentlyFound => {}
                }
            }

            match &app.state {
                LoadState::Loading { .. } => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.spinner();
                        ui.heading("Loading EmuWiz data...");
                        ui.label("Scanning runs in the background.");
                    });
                    // Requirement 1: show cached library rows before the
                    // live snapshot finishes loading, clearly labelled as
                    // last-known state. This is a read-only preview -
                    // built with the same ArchiveRow/show_archive_rows
                    // machinery as the live table, but with no selection
                    // or action wiring at all, so it cannot expose a mount
                    // or unmount button even in principle.
                    if let Some(snapshot) = app.database_state.snapshot() {
                        ui.separator();
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "Showing last-known catalogue state while the live snapshot loads.",
                        );
                        let preview_rows: Vec<ArchiveRow> = snapshot
                            .archives
                            .iter()
                            .map(|persisted| {
                                let path_exists = persisted.absolute_path.exists();
                                ArchiveRow::from_cached(persisted, path_exists)
                            })
                            .collect();
                        let row_height = fixed_row_height(
                            ui.text_style_height(&egui::TextStyle::Body),
                            ui.spacing().interact_size.y,
                        );
                        let horizontal_spacing = ui.spacing().item_spacing.x;
                        let preview_widths = app.library_ui.library_column_widths.as_array();
                        egui::ScrollArea::horizontal()
                            .id_salt("cache_preview_horizontal")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(table_width(
                                    horizontal_spacing,
                                    &preview_widths,
                                ));
                                // This cache-only loading preview has no sort
                                // state of its own to wire up (it disappears
                                // the moment the live snapshot loads) - the
                                // headers render inertly, unsorted. Still
                                // resizable (shares `app.library_ui.library_column_widths`
                                // with the real table) so a resize made here
                                // is not lost once the live snapshot loads.
                                let _ = show_header_row(
                                    ui,
                                    &COLUMN_HEADERS,
                                    &COLUMN_SORT_FIELDS,
                                    row_height,
                                    None,
                                    true,
                                    &mut app.library_ui.library_column_widths,
                                );
                                ui.separator();
                                let body_height = ui.available_height().max(row_height);
                                egui::ScrollArea::vertical()
                                    .id_salt("cache_preview_vertical")
                                    .max_height(body_height)
                                    .auto_shrink([false, false])
                                    .show_rows(
                                        ui,
                                        row_height,
                                        preview_rows.len(),
                                        |ui, row_range| {
                                            // Discard the result: this preview never sets
                                            // selected_archive, so it can never drive
                                            // show_selected_archive's action buttons, and
                                            // `records: &[]` means its context menu (if
                                            // right-clicked) offers nothing live either.
                                            let _ = show_archive_rows(
                                                ui,
                                                &preview_rows,
                                                None,
                                                row_range,
                                                row_height,
                                                None,
                                                &mut HashSet::new(),
                                                &mut None,
                                                &preview_widths,
                                                &RowMenuContext {
                                                    records: &[],
                                                    cached: None,
                                                    busy: true,
                                                    block_reason: None,
                                                    platform_busy: false,
                                                    retroarch_profiles: &app
                                                        .emulator_readiness
                                                        .retroarch_profiles,
                                                    library_views_configured: false,
                                                    library_view_last_plan: None,
                                                },
                                            );
                                        },
                                    );
                            });
                    }
                }
                LoadState::Error(error) => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            "Could not load EmuWiz",
                        );
                        ui.label(error);
                        ui.add_space(8.0);
                        retry = ui.button("Retry").clicked();
                    });
                }
                LoadState::Ready(data) => {
                    let missing_removal_unavailable_reason =
                        app.missing_removal_unavailable_reason();
                    let merged_rows = cached_display_rows(
                        &mut app.library_ui.merged_rows,
                        MergedDisplayRowsKey {
                            live_data_ptr: std::ptr::from_ref(data.as_ref()) as usize,
                            database_snapshot_ptr: app
                                .database_state
                                .snapshot()
                                .map(|snapshot| std::ptr::from_ref(snapshot) as usize),
                            refresh_generation: app.refresh_generation,
                            snapshot_generation: app.snapshot_generation,
                            database_generation: app.database_generation,
                        },
                        &data.records,
                        &data.rows,
                        app.database_state.snapshot(),
                    );
                    requested_action = show_loaded_data(
                        ui,
                        data,
                        LoadedViewState {
                            merged_rows,
                            filter: &mut app.library_ui.filter,
                            filtered_rows: &mut app.library_ui.filtered_rows,
                            selected_archive: &mut app.archive_context.focused,
                            operation: app.mount_ui.operation.as_ref(),
                            busy: archive_actions_blocked,
                            block_reason: archive_action_block_reason,
                            action_readiness_debug_lines: &action_readiness_debug_lines,
                            feedback: app.feedback.as_ref(),
                            confirm_unmount: &mut app.mount_ui.confirm_unmount,
                            confirm_lazy_unmount: &mut app.mount_ui.confirm_lazy_unmount,
                            confirm_lazy_unmount_final: &mut app.mount_ui.confirm_lazy_unmount_final,
                            confirm_mount_all: &mut app.mount_ui.confirm_mount_all,
                            focus_mount_all_cancel: &mut app.mount_ui.focus_mount_all_cancel,
                            mount_all_typed_count: &mut app.mount_ui.mount_all_typed_count,
                            confirm_unmount_all: &mut app.mount_ui.confirm_unmount_all,
                            focus_unmount_all_cancel: &mut app.mount_ui.focus_unmount_all_cancel,
                            unmount_all_typed_count: &mut app.mount_ui.unmount_all_typed_count,
                            confirm_unmount_selected: &mut app.mount_ui.confirm_unmount_selected,
                            focus_unmount_selected_cancel: &mut app
                                .mount_ui
                                .focus_unmount_selected_cancel,
                            confirm_mount_selected: &mut app.mount_ui.confirm_mount_selected,
                            focus_mount_selected_cancel: &mut app.mount_ui.focus_mount_selected_cancel,
                            mount_selected_typed_count: &mut app.mount_ui.mount_selected_typed_count,
                            confirm_bulk_platform_action: &mut app
                                .confirm_bulk_platform_action,
                            focus_bulk_platform_cancel: &mut app.focus_bulk_platform_cancel,
                            bulk_platform_action_typed_count: &mut app
                                .bulk_platform_action_typed_count,
                            focus_lazy_cancel: &mut app.mount_ui.focus_lazy_cancel,
                            focus_final_lazy_cancel: &mut app.mount_ui.focus_final_lazy_cancel,
                            lazy_unmount_offers: &app.mount_ui.lazy_unmount_offers,
                            remount_offers: &app.mount_ui.remount_offers,
                            cleanup_after_unmount: &mut app.mount_ui.cleanup_after_unmount,
                            mount_all_result: app.mount_ui.mount_all_result.as_ref(),
                            unmount_all_result: app.mount_ui.unmount_all_result.as_ref(),
                            history: &mut app.history,
                            cached: app.database_state.snapshot(),
                            library_filters: &mut app.library_ui.library_filters,
                            platform_choice: &mut app.library_ui.platform_choice,
                            platform_custom_text: &mut app.library_ui.platform_custom_text,
                            platform_busy: app.library_ui.platform_action.is_some(),
                            retroarch_profiles: &app.emulator_readiness.retroarch_profiles,
                            selected_evidence: &app.selected_evidence_ui.selected_evidence,
                            selected_archives: &mut app.archive_context.selected,
                            bulk_platform_choice: &mut app.library_ui.bulk_platform_choice,
                            bulk_platform_busy: app.library_ui.bulk_platform_action.is_some(),
                            missing_removal_available,
                            missing_removal_unavailable_reason,
                            missing_removal_busy: app.library_ui.missing_removal.is_some(),
                            confirm_remove_missing: &mut app.library_ui.confirm_remove_missing,
                            missing_removal_typed_count: &mut app.missing_removal_typed_count,
                            sort_field: &mut app.library_ui.sort_field,
                            sort_ascending: &mut app.library_ui.sort_ascending,
                            library_scroll_offset: &mut app.library_ui.library_scroll_offset,
                            clipboard: &mut app.clipboard,
                            select_all_visible_requested: &mut app
                                .select_all_visible_requested,
                            library_source_filter: &mut app.library_ui.library_source_filter,
                            library_column_widths: &mut app.library_ui.library_column_widths,
                            library_views_configured: !app.library_views.is_empty(),
                            library_view_last_plan: app.library_view_last_plan.as_ref(),
                            recent_scan: if app.library_tab == LibraryTab::RecentlyFound {
                                app.database_state
                                    .snapshot()
                                    .and_then(|snapshot| snapshot.recently_found.as_ref())
                            } else {
                                None
                            },
                            recent_view: app.library_tab == LibraryTab::RecentlyFound,
                            library_platform_query: &mut app.library_ui.library_platform_query,
                            screenscraper_state: &mut app.screenscraper_enrichment,
                            screenscraper_settings: &app.screenscraper_page,
                        },
                    );
                    if app.library_tab == LibraryTab::Archives
                        && app.archive_context.focused.is_some()
                    {
                        ui.add_space(crate::ui::theme::SECTION_GAP);
                        let game_details_action = app.show_game_details(
                            context,
                            ui,
                            archive_actions_blocked,
                            archive_action_block_reason,
                        );
                        app.handle_mount_page_action(context, game_details_action);
                    }
                }
            }
        });
    });
    PageDispatchOutcome {
        retry,
        requested_action,
        diagnostics_action,
        health_dashboard_action,
        stop_mount_all,
        stop_unmount_all,
    }
}
