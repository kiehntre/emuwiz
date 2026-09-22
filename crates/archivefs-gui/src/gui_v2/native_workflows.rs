//! Native v2 host for the established emulator setup and launch coordinator.
//!
//! The coordinator remains the single owner of evidence gathering, adapter
//! discovery, launch planning and process execution. This bridge only gives
//! those existing workflows v2 navigation and Activity integration.

use super::{activity::Activity, environment::EnvironmentSnapshot, routes::Route};
use crate::{
    ArchiveFsApp, DolphinLocalProfilesState, FlycastProfilesState, LoadState,
    Pcsx2FirmwareEvidenceState, Pcsx2LaunchProfilesState, RetroArchProfilesState, app_polling,
    identity_sources_page, launch_readiness_page, selected_evidence_page,
};
use eframe::egui;
use std::path::{Path, PathBuf};

pub(super) struct NativeWorkflows {
    pub(super) app: ArchiveFsApp,
    selected: Option<PathBuf>,
    selected_game: Option<i64>,
    readiness_job: Option<u64>,
    setup_job: Option<u64>,
    launch_job: Option<u64>,
    launch_was_active: bool,
    launch_kind: Option<LaunchKind>,
    source_job: Option<u64>,
    provider_job: Option<u64>,
    metadata_job: Option<u64>,
    dat_job: Option<u64>,
    cheat_job: Option<u64>,
    source_library_reload: bool,
    artwork_reload: bool,
}

#[derive(Clone, Copy)]
enum LaunchKind {
    RetroArch,
    Dolphin,
    Pcsx2,
    Standalone,
    AmigaWhdLoad,
}

impl NativeWorkflows {
    pub(super) fn new(context: egui::Context) -> Self {
        #[cfg(not(test))]
        let mut app = ArchiveFsApp::new_without_initial_load(context.clone());
        #[cfg(test)]
        let mut app = crate::tests::app_for_operation_tests();
        // These are full v2 task pages, not the old shell's simplified
        // navigation mode. Keep the shared workflow widgets and all of their
        // safe actions visible without rendering the legacy chrome.
        app.ui_mode = crate::GuiMode::AdvancedView;
        // The embedded coordinator applies the legacy shell theme while it is
        // constructed. Restore v2's typography immediately; the shared page
        // widgets themselves remain theme-independent.
        super::readable_style(&context);
        Self {
            app,
            selected: None,
            selected_game: None,
            readiness_job: None,
            setup_job: None,
            launch_job: None,
            launch_was_active: false,
            launch_kind: None,
            source_job: None,
            provider_job: None,
            metadata_job: None,
            dat_job: None,
            cheat_job: None,
            source_library_reload: false,
            artwork_reload: false,
        }
    }

    pub(super) fn poll(&mut self, context: &egui::Context, activity: &mut Activity) {
        app_polling::poll_and_reconcile(&mut self.app, context);

        let changed = self.app.launch_retroarch.poll()
            | self.app.launch_dolphin.poll()
            | self.app.launch_pcsx2.poll()
            | self.app.launch_standalone.poll()
            | self.app.launch_amiga_whdload.poll();
        let active = self.launch_active();
        if changed || active {
            context.request_repaint();
        }
        self.observe_launch_activity(activity);
        self.observe_readiness_activity(activity);
        self.observe_source_activity(activity);
        self.observe_provider_activity(activity);
        self.observe_metadata_activity(activity);
        self.poll_dat_activity(context, activity);
        self.observe_cheat_activity(activity);

        let checking_setup = self.app.doctor_repair.doctor_scan.is_running();
        if checking_setup && self.setup_job.is_none() {
            let job = activity.queue(
                "Checking emulator readiness",
                Route::Section(super::routes::Section::Emulators),
                false,
            );
            activity.start(job);
            self.setup_job = Some(job);
        } else if !checking_setup && let Some(job) = self.setup_job.take() {
            activity.finish(job, "Emulator and firmware checks are ready.".into(), None);
        }
    }

    fn observe_launch_activity(&mut self, activity: &mut Activity) {
        let active = self.launch_active();
        if active && !self.launch_was_active {
            self.launch_kind = self.active_launch_kind();
            let job = activity.queue("Launching game", self.selected_route(), false);
            activity.start(job);
            self.launch_job = Some(job);
        } else if !active
            && self.launch_was_active
            && let Some(job) = self.launch_job.take()
        {
            let error = self.launch_failure();
            activity.finish(
                job,
                if error.is_some() {
                    "The game could not be launched safely.".into()
                } else {
                    "The emulator launch process finished.".into()
                },
                error,
            );
            self.launch_kind = None;
        }
        self.launch_was_active = active;
    }

    fn observe_readiness_activity(&mut self, activity: &mut Activity) {
        let Some(job) = self.readiness_job else {
            return;
        };
        if let Some(error) = self.readiness_error() {
            activity.finish(
                job,
                "EmuWiz couldn't finish checking this game for launch.".into(),
                Some(error),
            );
            self.readiness_job = None;
        } else if !matches!(
            self.launch_input(),
            launch_readiness_page::LaunchReadinessInput::EvidenceNotLoaded
        ) {
            activity.finish(job, "Launch readiness is ready to review.".into(), None);
            self.readiness_job = None;
        }
    }

    pub(super) fn show_launch(
        &mut self,
        ui: &mut egui::Ui,
        game_id: i64,
        path: &Path,
        activity: &mut Activity,
    ) -> bool {
        self.selected_game = Some(game_id);
        if self.selected.as_deref() != Some(path)
            && let Some(job) = self.readiness_job.take()
        {
            activity.finish(
                job,
                "Selection changed; the previous readiness result was discarded.".into(),
                None,
            );
        }
        self.select(path);
        self.start_launch_readiness(ui.ctx());

        let input = self.launch_input();
        let readiness_error = self.readiness_error();
        if readiness_error.is_none()
            && matches!(
                input,
                launch_readiness_page::LaunchReadinessInput::EvidenceNotLoaded
            )
            && self.readiness_job.is_none()
        {
            let job = activity.queue(
                "Preparing launch",
                Route::Task {
                    section: super::routes::Section::Launch,
                    game: game_id,
                },
                false,
            );
            activity.start(job);
            self.readiness_job = Some(job);
        }
        self.observe_readiness_activity(activity);

        let (open_setup, retry_readiness) = egui::ScrollArea::vertical()
            .id_salt(("v2_native_launch", game_id))
            .show(ui, |ui| {
                ui.label("Choose an emulator, review its readiness, then launch. EmuWiz will refuse unsafe or incomplete media.");
                if readiness_error.is_some() {
                    crate::ui::components::banner(
                        ui,
                        "Launch readiness could not be checked",
                        "Your game was not changed. Retry when the file is available.",
                        crate::ui::components::StatusTone::Warning,
                    );
                }
                let retry_readiness = readiness_error.is_some()
                    && ui.button("Retry readiness check").clicked();
                let launch_active = self.launch_active();
                let action = ui.add_enabled_ui(!launch_active, |ui| {
                    launch_readiness_page::show_launch_readiness_panel(
                        ui,
                        &input,
                        &mut self.app.launch_retroarch,
                        &mut self.app.launch_dolphin,
                        &mut self.app.launch_pcsx2,
                        &mut self.app.launch_standalone,
                        &mut self.app.launch_amiga_whdload,
                    )
                }).inner;
                if launch_active {
                    ui.label("A launch is already in progress. Wait for it to finish before starting another.");
                }
                (
                    matches!(
                        action,
                        Some(launch_readiness_page::LaunchReadinessPageAction::OpenDoctor)
                    ),
                    retry_readiness,
                )
            })
            .inner;
        if retry_readiness && let Some(path) = self.selected.clone() {
            self.app
                .start_selected_evidence_load(ui.ctx().clone(), path);
        }
        self.observe_launch_activity(activity);
        open_setup
    }

    pub(super) fn show_setup(
        &mut self,
        ui: &mut egui::Ui,
        environment: Option<&EnvironmentSnapshot>,
    ) {
        if let Some(environment) = environment {
            lifecycle_setup_panel(ui, &environment.lifecycle, &mut self.app);
        }
        let context = ui.ctx().clone();
        self.app
            .navigate_to_main_view(crate::navigation::MainView::EmulatorSetup);
        self.app.show_emulator_setup_page(ui, &context);
    }

    pub(super) fn show_sources(&mut self, ui: &mut egui::Ui, activity: &mut Activity) {
        self.app
            .navigate_to_main_view(crate::navigation::MainView::Sources);
        app_polling::start_view_gated_work(&mut self.app, ui.ctx());
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.app.source_action_available(),
                    egui::Button::new("Add source"),
                )
                .clicked()
            {
                self.app.sources_ui.sources_add_dialog =
                    Some(crate::source_controller::SourcesAddDialogState::default());
            }
            ui.label("Add and scan existing folders; source files are never moved or deleted.");
            if ui.button("Verification Data / DATs").clicked() {
                self.app
                    .navigate_to_sources_tab(crate::navigation::SourcesTab::Dats);
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("v2_native_sources")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let context = ui.ctx().clone();
                self.app
                    .show_sources_page(&context, ui, self.app.sources_tab);
            });
        self.observe_source_activity(activity);
        self.observe_provider_activity(activity);
        self.observe_dat_activity(activity);
    }

    pub(super) fn show_dat_sources(&mut self, ui: &mut egui::Ui, activity: &mut Activity) {
        self.app
            .navigate_to_sources_tab(crate::navigation::SourcesTab::Dats);
        app_polling::start_view_gated_work(&mut self.app, ui.ctx());
        ui.label("Import, validate, activate and review verification data using the same version-bound evidence as library checks.");
        egui::ScrollArea::vertical()
            .id_salt("v2_native_dat_sources")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let context = ui.ctx().clone();
                self.app
                    .show_sources_page(&context, ui, crate::navigation::SourcesTab::Dats);
            });
        self.observe_dat_activity(activity);
    }

    pub(super) fn show_cheats(
        &mut self,
        ui: &mut egui::Ui,
        selected: Option<&crate::gui_v2::library::Game>,
        activity: &mut Activity,
    ) -> Option<Route> {
        let Some(game) = selected else {
            crate::ui::components::card(ui, |ui| {
                ui.heading("Choose a game first");
                ui.label("Open a game from Games, then choose Mods & Cheats to review compatible cheats.");
                ui.label("No cheat is changed by browsing this page.");
            });
            return None;
        };
        let path = game.archive.absolute_path.clone();
        if !self
            .app
            .cheat_workflow
            .as_ref()
            .is_some_and(|workflow| workflow.archive_path == path)
        {
            self.app.open_cheats_mods_workspace(ui.ctx(), path);
        }
        let Some(workflow) = self.app.cheat_workflow.as_ref() else {
            crate::ui::components::banner(
                ui,
                "Cheat identity is not ready",
                "EmuWiz could not bind this selection to the loaded library record. Refresh the library and try again; no files were changed.",
                crate::ui::components::StatusTone::Warning,
            );
            return None;
        };
        ui.label(format!("Selected game: {} · {}", game.title, game.platform));
        ui.label(match workflow.adapter.display_name() {
            Some(adapter) => format!("Supported cheat target: {adapter}"),
            None => "Unsupported format for automatic cheat apply".to_string(),
        });
        let live = match &self.app.state {
            LoadState::Ready(data) => Some(data.as_ref()),
            LoadState::Loading { previous, .. } => previous.as_deref(),
            LoadState::Error(_) => None,
        };
        // The shared controller rejects overlapping requests as a second line
        // of defence. Also disable its submit controls while any cheat worker
        // is active so a double click is visibly refused at the v2 surface.
        let busy = cheat_activity_state(self.app.cheat_workflow.as_ref()).is_some();
        let action = crate::cheats_mods_preview::show_cheats_mods_page(
            ui,
            self.app.cheat_workflow.as_mut(),
            &self.app.emulator_readiness.retroarch_profiles,
            &self.app.emulator_readiness.pcsx2_profiles,
            &self.app.emulator_readiness.dolphin_profiles,
            &self.app.emulator_readiness.xenia_profiles,
            live,
            self.app.database_state.snapshot(),
            &self.app.history,
            busy,
            &mut self.app.clipboard,
            &mut self.app.dolphin_texture_mod,
            &mut self.app.local_mod_package,
        );
        let library = live
            .map(|data| {
                data.records
                    .iter()
                    .map(
                        |record| archivefs_core::patch_manager::UserCheatLibraryGame {
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
                        },
                    )
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let selected_game = self.app.cheat_workflow.as_ref().map(|workflow| {
            (
                workflow.archive_path.display().to_string(),
                workflow.display_name.clone(),
            )
        });
        let local_retroarch = self
            .app
            .cheat_workflow
            .as_ref()
            .filter(|workflow| workflow.adapter == crate::CheatEmulatorAdapter::RetroArch)
            .map(|workflow| {
                crate::local_cheat_install_context(
                    workflow,
                    &self.app.emulator_readiness.retroarch_profiles,
                )
            });
        let local_pcsx2 = self
            .app
            .cheat_workflow
            .as_ref()
            .filter(|workflow| workflow.adapter == crate::CheatEmulatorAdapter::Pcsx2)
            .and_then(|workflow| {
                crate::local_pcsx2_install_context(
                    workflow,
                    &self.app.emulator_readiness.pcsx2_profiles,
                )
            });
        let local_dolphin = self
            .app
            .cheat_workflow
            .as_ref()
            .filter(|workflow| workflow.adapter == crate::CheatEmulatorAdapter::Dolphin)
            .and_then(|workflow| {
                crate::local_dolphin_install_context(
                    workflow,
                    &self.app.emulator_readiness.dolphin_profiles,
                )
            });
        let local_xenia = self
            .app
            .cheat_workflow
            .as_ref()
            .filter(|workflow| workflow.adapter == crate::CheatEmulatorAdapter::Xenia)
            .map(|workflow| {
                crate::local_xenia_install_context(
                    workflow,
                    &self.app.emulator_readiness.xenia_profiles,
                )
            });
        ui.add_space(12.0);
        let context = ui.ctx().clone();
        self.app.user_cheat_import_page.show(
            ui,
            &context,
            &library,
            selected_game
                .as_ref()
                .map(|(id, title)| (id.as_str(), title.as_str())),
            local_retroarch.as_ref(),
            local_pcsx2.as_ref(),
            local_dolphin.as_ref(),
            local_xenia.as_ref(),
        );
        let destination = action.and_then(|action| self.handle_cheat_action(&context, action));
        self.observe_cheat_activity(activity);
        destination
    }

    pub(super) fn take_source_library_reload(&mut self) -> bool {
        std::mem::take(&mut self.source_library_reload)
    }

    pub(super) fn take_artwork_reload(&mut self) -> bool {
        std::mem::take(&mut self.artwork_reload)
    }

    pub(super) fn has_cheat_history(&self) -> bool {
        self.app.history.entries().any(|entry| {
            matches!(
                entry.action,
                crate::activity_history::ActivityAction::CheatSourceRetrieval
                    | crate::activity_history::ActivityAction::CheatPreview
                    | crate::activity_history::ActivityAction::CheatInstall
            )
        })
    }

    pub(super) fn show_cheat_history(&self, ui: &mut egui::Ui) -> bool {
        let entries = self
            .app
            .history
            .entries()
            .filter(|entry| {
                matches!(
                    entry.action,
                    crate::activity_history::ActivityAction::CheatSourceRetrieval
                        | crate::activity_history::ActivityAction::CheatPreview
                        | crate::activity_history::ActivityAction::CheatInstall
                )
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return false;
        }
        ui.heading("Cheat activity");
        for entry in entries {
            crate::ui::components::card(ui, |ui| {
                ui.strong(entry.action.to_string());
                ui.label(entry.outcome.to_string());
                ui.label(&entry.message);
                if let Some(path) = &entry.archive_path {
                    ui.collapsing("Advanced Details", |ui| {
                        ui.monospace(path.display().to_string());
                    });
                }
            });
        }
        let undo_available = self.app.cheat_workflow.as_ref().is_some_and(|workflow| {
            matches!(
                workflow.transaction,
                crate::CheatTransactionState::Result { .. }
            )
        });
        if undo_available {
            ui.label("Undo is available from the reviewed result in Mods & Cheats.");
            ui.button("Open Mods & Cheats to review undo").clicked()
        } else {
            ui.label("Undo is unavailable unless the selected adapter produced a recoverable transaction.");
            false
        }
    }

    pub(super) fn show_metadata_tools(
        &mut self,
        ui: &mut egui::Ui,
        selected: Option<(i64, &Path)>,
        activity: &mut Activity,
    ) -> bool {
        let context = ui.ctx().clone();
        let mut changed = false;
        ui.collapsing("ScreenScraper provider setup", |ui| {
            crate::screenscraper_page::show_screen_scraper_settings(
                ui,
                &mut self.app.screenscraper_page,
                self.app.screenscraper_enrichment.is_running(),
            );
        });
        if let Some((game_id, path)) = selected {
            self.select(path);
            let record = match &self.app.state {
                LoadState::Ready(data) => data
                    .records
                    .iter()
                    .find(|record| record.mount_plan.archive.path == path),
                LoadState::Loading { previous, .. } => previous.as_ref().and_then(|data| {
                    data.records
                        .iter()
                        .find(|record| record.mount_plan.archive.path == path)
                }),
                LoadState::Error(_) => None,
            }
            .cloned();
            if let Some(record) = record {
                let existing = self
                    .app
                    .database_state
                    .snapshot()
                    .and_then(|snapshot| snapshot.screenscraper_enrichments.get(&game_id))
                    .cloned();
                if let Some(
                    crate::screenscraper_enrichment_page::ScreenScraperEnrichmentAction::Apply {
                        archive_id,
                        values,
                        receipt,
                    },
                ) = crate::screenscraper_enrichment_page::show(
                    ui,
                    &mut self.app.screenscraper_enrichment,
                    &self.app.screenscraper_page,
                    &record,
                    game_id,
                    existing.as_ref(),
                ) {
                    self.app
                        .apply_screenscraper_enrichment(context, archive_id, values, receipt);
                    changed = self
                        .app
                        .feedback
                        .as_ref()
                        .is_some_and(|feedback| feedback.succeeded);
                }
            } else {
                ui.label("Loading the selected game's provider-safe metadata identity…");
            }
        }
        self.observe_metadata_activity(activity);
        changed
    }

    fn observe_metadata_activity(&mut self, activity: &mut Activity) {
        let running = self.app.screenscraper_enrichment.is_running();
        if running && self.metadata_job.is_none() {
            let job = activity.queue("Refreshing metadata", self.selected_route(), false);
            activity.start(job);
            self.metadata_job = Some(job);
        } else if !running && let Some(job) = self.metadata_job.take() {
            let feedback = self.app.feedback.as_ref();
            let error = feedback
                .filter(|feedback| !feedback.succeeded)
                .map(|feedback| feedback.message.clone());
            activity.finish(
                job,
                feedback.map_or_else(
                    || "Metadata candidates are ready to review.".into(),
                    |feedback| feedback.message.clone(),
                ),
                error,
            );
        }
    }

    pub(super) fn observe_source_activity(&mut self, activity: &mut Activity) {
        let running = self
            .app
            .sources_ui
            .source_action
            .as_ref()
            .map(|running| running.action.clone());
        if self.source_job.is_none()
            && let Some(action) = running.clone()
        {
            let title = match action {
                crate::platform_source_actions::SourceAction::ScanOne(_)
                | crate::platform_source_actions::SourceAction::ScanAll => "Scanning source",
                crate::platform_source_actions::SourceAction::Add(_) => "Adding source",
                crate::platform_source_actions::SourceAction::Remove { .. } => "Removing source",
                _ => "Updating source",
            };
            let job = activity.queue(
                title,
                Route::Section(super::routes::Section::Sources),
                false,
            );
            activity.start(job);
            self.source_job = Some(job);
        } else if running.is_none()
            && let Some(job) = self.source_job.take()
        {
            let feedback = self.app.feedback.as_ref();
            let error = feedback
                .filter(|feedback| !feedback.succeeded)
                .map(|feedback| feedback.message.clone());
            let summary = feedback.map_or_else(
                || "The source operation finished safely.".into(),
                |feedback| feedback.message.clone(),
            );
            if error.is_none() {
                self.source_library_reload = true;
            }
            activity.finish(job, summary, error);
        }
    }

    fn observe_provider_activity(&mut self, activity: &mut Activity) {
        let loading = matches!(
            self.app.artwork_media.es_de_media.state(),
            crate::es_de_media_state::EsDeProviderState::Loading
        ) || matches!(
            self.app.artwork_media.launchbox_local_media.state(),
            crate::launchbox_local_state::LaunchBoxLocalState::Loading
        );
        if loading && self.provider_job.is_none() {
            let job = activity.queue(
                "Refreshing artwork providers",
                Route::Section(super::routes::Section::Artwork),
                false,
            );
            activity.start(job);
            self.provider_job = Some(job);
        } else if !loading && let Some(job) = self.provider_job.take() {
            let errors = [
                self.app.artwork_media.es_de_media.error(),
                self.app.artwork_media.launchbox_local_media.error(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            activity.finish(
                job,
                if errors.is_empty() {
                    "Local artwork providers are ready.".into()
                } else {
                    "Provider refresh finished with items needing attention.".into()
                },
                (!errors.is_empty()).then(|| errors.join("\n")),
            );
            self.artwork_reload = true;
        }
    }

    fn poll_dat_activity(&mut self, context: &egui::Context, activity: &mut Activity) {
        if let Some(page) = self.app.sources_ui.dat_sources_page.as_mut()
            && (page.poll() || page.is_busy())
        {
            context.request_repaint();
        }
        self.observe_dat_activity(activity);
    }

    fn observe_dat_activity(&mut self, activity: &mut Activity) {
        let state = self
            .app
            .sources_ui
            .dat_sources_page
            .as_ref()
            .and_then(crate::dat_sources_page::DatSourcesPageState::background_activity);
        let error = self
            .app
            .sources_ui
            .dat_sources_page
            .as_ref()
            .and_then(crate::dat_sources_page::DatSourcesPageState::background_error);
        observe_dat_activity_state(activity, &mut self.dat_job, state, error);
    }

    fn handle_cheat_action(
        &mut self,
        context: &egui::Context,
        action: crate::CheatWorkflowAction,
    ) -> Option<Route> {
        use crate::CheatWorkflowAction as A;
        match action {
            A::ChooseArchive | A::OpenLibrary => {
                return Some(Route::Section(super::routes::Section::Games));
            }
            A::RescanProfiles => self.app.start_retroarch_profile_scan(context.clone()),
            A::RescanPcsx2Profiles => self.app.start_pcsx2_profile_scan(context.clone()),
            A::InspectPcsx2Profile => self.app.start_pcsx2_inventory(context.clone()),
            A::FetchPcsx2GameHacking { force_refresh } => self
                .app
                .start_pcsx2_gamehacking_fetch(context.clone(), force_refresh),
            A::ConfirmPcsx2GameHackingMatch { game_id } => self
                .app
                .confirm_pcsx2_gamehacking_match(context.clone(), game_id),
            A::TogglePcsx2CheatSelected { id, selected } => {
                self.app.update_pcsx2_cheat_selection(&id, selected);
            }
            A::InstallSelectedPcsx2 => self.app.start_pcsx2_install_preview(),
            A::RescanDolphinProfiles => self.app.start_dolphin_profile_scan(context.clone()),
            A::InspectDolphinProfile => self.app.start_dolphin_inventory(context.clone()),
            A::InspectExistingLibrary => self
                .app
                .start_existing_retroarch_library_inspection(context.clone()),
            A::RefreshSources => self.app.start_cheat_source_list(context.clone()),
            A::ManageCatalogue => {
                self.app
                    .navigate_to_sources_tab(crate::navigation::SourcesTab::Cheats);
                return Some(Route::Section(super::routes::Section::Sources));
            }
            A::UseCachedSnapshot => self.app.start_cheat_source_fetch(context.clone(), true),
            A::ReviewApply => self.app.review_cheat_apply(),
            A::ConfirmApply => self.app.start_cheat_apply(context.clone()),
            A::CancelApply => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.transaction = crate::CheatTransactionState::Idle;
                    workflow.transaction_notice = Some(
                        "Installation cancelled before apply; no live emulator file was changed."
                            .to_string(),
                    );
                }
                self.app
                    .history
                    .record(crate::activity_history::HistoryEntry::new(
                        crate::activity_history::ActivityAction::CheatInstall,
                        self.app
                            .cheat_workflow
                            .as_ref()
                            .map(|workflow| workflow.archive_path.clone()),
                        crate::activity_history::ActivityOutcome::Cancelled,
                        "Install cancelled before the write phase; nothing was changed.",
                    ));
            }
            A::OpenApplyHistory => {
                return Some(Route::Section(super::routes::Section::History));
            }
            A::MatchCandidates => self.app.start_cheat_candidate_match(context.clone()),
            A::SelectCandidate(path) => self.app.apply_cheat_candidate_choice(&path),
            A::ClearCandidateChoice => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.candidate_selection = None;
                    workflow.candidate_load_error = None;
                    workflow.preview = crate::CheatStepResource::NotLoaded;
                    workflow.preview_request = None;
                    workflow.transaction = crate::CheatTransactionState::Idle;
                }
            }
            A::ToggleCheatSelected { index, selected } => {
                self.app.update_cheat_selection(|selection| {
                    selection.set_selected(index, selected);
                })
            }
            A::ToggleCheatEnabled { index, enabled } => {
                self.app.update_cheat_selection(|selection| {
                    selection.set_enabled(index, enabled);
                })
            }
            A::SelectAllCheats => self
                .app
                .update_cheat_selection(archivefs_core::patch_manager::CheatSelection::select_all),
            A::ClearAllCheats => self
                .app
                .update_cheat_selection(archivefs_core::patch_manager::CheatSelection::clear_all),
            A::BuildInstallPreview => self.app.start_generated_cheat_preview(context.clone()),
            A::RollbackInstall => self.app.start_cheat_install_rollback(context.clone()),
            A::FetchDolphinProvider { force_refresh } => self
                .app
                .start_dolphin_provider_fetch(context.clone(), force_refresh),
            A::ToggleDolphinCodeSelected { index, selected } => {
                self.app.update_dolphin_code_selection(|selection| {
                    selection.set_selected(index, selected);
                })
            }
            A::SelectAllDolphinCodes => self.app.update_dolphin_code_selection(
                archivefs_core::patch_manager::DolphinProviderCodeSelection::select_all,
            ),
            A::ClearAllDolphinCodes => self.app.update_dolphin_code_selection(
                archivefs_core::patch_manager::DolphinProviderCodeSelection::clear_all,
            ),
            A::BuildDolphinInstallPreview => self.app.start_dolphin_install_preview(),
            A::RescanXeniaProfiles => self.app.start_xenia_profile_scan(),
            A::FetchXeniaProvider { force_refresh } => self
                .app
                .start_xenia_provider_fetch(context.clone(), force_refresh),
            A::SelectXeniaCandidate(index) => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.xenia_selected_candidate_index = Some(index);
                    workflow.xenia_selection = None;
                    workflow.xenia_destination_error = None;
                    workflow.preview = crate::CheatStepResource::NotLoaded;
                    workflow.preview_request = None;
                    workflow.transaction = crate::CheatTransactionState::Idle;
                }
            }
            A::ClearXeniaCandidateChoice => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.xenia_selected_candidate_index = None;
                    workflow.xenia_selection = None;
                    workflow.xenia_destination_error = None;
                    workflow.preview = crate::CheatStepResource::NotLoaded;
                    workflow.preview_request = None;
                    workflow.transaction = crate::CheatTransactionState::Idle;
                }
            }
            A::AcknowledgeXeniaPartialVerification(acknowledged) => {
                if let Some(selection) = self
                    .app
                    .cheat_workflow
                    .as_mut()
                    .and_then(|workflow| workflow.xenia_selection.as_mut())
                {
                    selection.selection.partial_verification_acknowledged = acknowledged;
                }
            }
            A::ToggleXeniaPatchSelected { index, selected } => {
                self.app.update_xenia_patch_selection(|selection| {
                    selection.set_selected(index, selected);
                })
            }
            A::SelectAllXeniaPatches => self.app.update_xenia_patch_selection(
                archivefs_core::patch_manager::XeniaPatchSelection::select_all,
            ),
            A::ClearAllXeniaPatches => self.app.update_xenia_patch_selection(
                archivefs_core::patch_manager::XeniaPatchSelection::clear_all,
            ),
            A::BuildXeniaInstallPreview => self.app.start_xenia_install_preview(),
            A::ChooseDolphinProfile(profile) => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.dolphin_profile_choice = Some(profile);
                }
                self.app.confirm_dolphin_profile_choice();
            }
            A::ChooseXeniaProfile(profile) => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.xenia_profile_choice = Some(profile);
                }
                self.app.confirm_xenia_profile_choice();
            }
            A::InstallSelectedDolphin => self.app.start_beginner_install_dolphin(),
            A::InstallSelectedXenia => self.app.start_beginner_install_xenia(),
            A::ToggleDolphinShowExactChanges(show) => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.dolphin_show_exact_changes = show;
                }
            }
            A::ToggleXeniaShowExactChanges(show) => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.xenia_show_exact_changes = show;
                }
            }
            A::ToggleDolphinDetailsOpen(open) => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.dolphin_details_open = open;
                }
            }
            A::ToggleXeniaDetailsOpen(open) => {
                if let Some(workflow) = self.app.cheat_workflow.as_mut() {
                    workflow.xenia_details_open = open;
                }
            }
            A::FetchGameCubeGameHacking { force_refresh } => self
                .app
                .start_gamecube_gamehacking_fetch(context.clone(), force_refresh),
            A::ConfirmGameCubeGameHackingMatch { game_id } => self
                .app
                .confirm_gamecube_gamehacking_match(context.clone(), game_id),
            A::ToggleGameCubeGameHackingCheatSelected { index, selected } => self
                .app
                .update_gamecube_gamehacking_cheat_selection(index, selected),
            A::InstallSelectedGameCubeGameHacking => {
                self.app.start_gamecube_gamehacking_install_preview();
            }
            A::RemoveSelectedGameCubeGameHacking => {
                self.app.start_gamecube_gamehacking_removal_preview();
            }
            A::OpenBrowserImport(platform) => self.app.open_browser_import(platform),
            A::CloseBrowserImport => self.app.close_browser_import(),
            A::OpenGameHackingPageInBrowser => self.app.open_gamehacking_page_in_browser(),
            A::CopyGameHackingPageUrl => self.app.copy_gamehacking_page_url(),
            A::ImportBrowserSavedFile => self.app.import_browser_saved_file(context.clone()),
            A::ToggleBrowserImportPaste(open) => {
                if let Some(state) = self
                    .app
                    .cheat_workflow
                    .as_mut()
                    .and_then(|workflow| workflow.browser_import.as_mut())
                {
                    state.paste_open = open;
                }
            }
            A::ImportBrowserPastedText => self.app.import_browser_pasted_text(context.clone()),
            A::ImportBrowserClipboard => self.app.import_browser_clipboard(context.clone()),
            A::ChooseBrowserImportKind(kind) => {
                if let Some(state) = self
                    .app
                    .cheat_workflow
                    .as_mut()
                    .and_then(|workflow| workflow.browser_import.as_mut())
                {
                    state.kind = kind;
                }
            }
            A::FetchBsFreeGameCube { search_title } => self
                .app
                .start_bsfree_gamecube_search(context.clone(), search_title),
            A::ConfirmBsFreeGameCubeMatch { upstream_uid } => self
                .app
                .start_bsfree_gamecube_confirm(context.clone(), upstream_uid),
            A::ToggleBsFreeGameCubeCheatSelected { index, selected } => self
                .app
                .update_bsfree_gamecube_cheat_selection(index, selected),
            A::SelectAllBsFreeGameCubeCheats => {
                self.app.update_bsfree_gamecube_cheat_selection_all(true)
            }
            A::ClearAllBsFreeGameCubeCheats => {
                self.app.update_bsfree_gamecube_cheat_selection_all(false)
            }
            A::InstallSelectedBsFreeGameCube => {
                self.app.start_bsfree_gamecube_install_preview();
            }
            A::FetchBsFreeWii { search_title } => self
                .app
                .start_bsfree_wii_search(context.clone(), search_title),
            A::ConfirmBsFreeWiiMatch { upstream_uid } => self
                .app
                .start_bsfree_wii_confirm(context.clone(), upstream_uid),
            A::ToggleBsFreeWiiCheatSelected { index, selected } => {
                self.app.update_bsfree_wii_cheat_selection(index, selected)
            }
            A::SelectAllBsFreeWiiCheats => {
                self.app.update_bsfree_wii_cheat_selection_all(true);
            }
            A::ClearAllBsFreeWiiCheats => {
                self.app.update_bsfree_wii_cheat_selection_all(false);
            }
            A::InstallSelectedBsFreeWii => self.app.start_bsfree_wii_install_preview(),
        }
        None
    }

    fn observe_cheat_activity(&mut self, activity: &mut Activity) {
        let state = cheat_activity_state(self.app.cheat_workflow.as_ref());
        let error = self
            .app
            .cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.transaction_notice.clone())
            .filter(|notice| notice.to_ascii_lowercase().contains("failed"));
        observe_cheat_activity_state(activity, &mut self.cheat_job, state, error);
    }

    fn select(&mut self, path: &Path) {
        if self.selected.as_deref() != Some(path) {
            self.selected = Some(path.to_path_buf());
            self.app.archive_context.select_only(path.to_path_buf());
        }
    }

    fn start_launch_readiness(&mut self, context: &egui::Context) {
        let Some(path) = self.selected.clone() else {
            return;
        };
        let stale = match &self.app.selected_evidence_ui.selected_evidence {
            selected_evidence_page::SelectedEvidenceState::Loading { path: current, .. } => {
                current != &path
            }
            selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                report.path != path
            }
            selected_evidence_page::SelectedEvidenceState::Idle => true,
            selected_evidence_page::SelectedEvidenceState::Error { path: current, .. } => {
                current != &path
            }
        };
        if stale {
            self.app.start_selected_evidence_load(context.clone(), path);
        }
        self.app.maybe_start_selected_evidence_enrichment(context);
        if matches!(
            self.app.emulator_readiness.retroarch_profiles,
            RetroArchProfilesState::NotScanned
        ) {
            self.app.start_retroarch_profile_scan(context.clone());
        }
        if matches!(
            self.app.emulator_readiness.dolphin_local_profiles,
            DolphinLocalProfilesState::NotScanned
        ) {
            self.app.start_dolphin_local_profile_scan(context.clone());
        }
        if matches!(
            self.app.emulator_readiness.pcsx2_launch_profiles,
            Pcsx2LaunchProfilesState::NotScanned
        ) {
            self.app.start_pcsx2_launch_profile_scan(context.clone());
        }
        if matches!(
            self.app.emulator_readiness.pcsx2_firmware_evidence,
            Pcsx2FirmwareEvidenceState::NotLoaded
        ) {
            self.app.start_pcsx2_firmware_evidence_load(context.clone());
        }
        if matches!(
            self.app.emulator_readiness.flycast_profiles,
            FlycastProfilesState::NotScanned
        ) {
            self.app.start_flycast_profile_scan(context.clone());
        }
        if matches!(
            self.app.selected_evidence_ui.scummvm_readiness,
            identity_sources_page::ScummVmReadinessState::NotChecked
        ) {
            self.app.start_scummvm_readiness_check(context.clone());
        }
    }

    fn launch_input(&self) -> launch_readiness_page::LaunchReadinessInput {
        let live = match &self.app.state {
            LoadState::Ready(data) => Some(data.as_ref()),
            LoadState::Loading { previous, .. } => previous.as_deref(),
            LoadState::Error(_) => None,
        };
        self.app.build_launch_readiness_input(live)
    }

    fn launch_active(&self) -> bool {
        self.app.launch_retroarch.is_active()
            || self.app.launch_dolphin.is_active()
            || self.app.launch_pcsx2.is_active()
            || self.app.launch_standalone.is_active()
            || self.app.launch_amiga_whdload.is_active()
    }

    fn launch_failure(&self) -> Option<String> {
        match self.launch_kind {
            Some(LaunchKind::RetroArch) => self.app.launch_retroarch.failure_detail(),
            Some(LaunchKind::Dolphin) => self.app.launch_dolphin.failure_detail(),
            Some(LaunchKind::Pcsx2) => self.app.launch_pcsx2.failure_detail(),
            Some(LaunchKind::Standalone) => self.app.launch_standalone.failure_detail(),
            Some(LaunchKind::AmigaWhdLoad) => self.app.launch_amiga_whdload.failure_detail(),
            None => None,
        }
    }

    fn active_launch_kind(&self) -> Option<LaunchKind> {
        if self.app.launch_retroarch.is_active() {
            Some(LaunchKind::RetroArch)
        } else if self.app.launch_dolphin.is_active() {
            Some(LaunchKind::Dolphin)
        } else if self.app.launch_pcsx2.is_active() {
            Some(LaunchKind::Pcsx2)
        } else if self.app.launch_standalone.is_active() {
            Some(LaunchKind::Standalone)
        } else if self.app.launch_amiga_whdload.is_active() {
            Some(LaunchKind::AmigaWhdLoad)
        } else {
            None
        }
    }

    fn readiness_error(&self) -> Option<String> {
        match &self.app.selected_evidence_ui.selected_evidence {
            selected_evidence_page::SelectedEvidenceState::Error { path, message, .. }
                if self.selected.as_deref() == Some(path.as_path()) =>
            {
                Some(message.clone())
            }
            _ => None,
        }
    }

    fn selected_route(&self) -> Route {
        self.selected_game
            .map_or(Route::Section(super::routes::Section::Launch), |game| {
                Route::Task {
                    section: super::routes::Section::Launch,
                    game,
                }
            })
    }

    #[cfg(test)]
    pub(super) fn selected_path(&self) -> Option<&Path> {
        self.selected.as_deref()
    }
}

fn lifecycle_setup_panel(
    ui: &mut egui::Ui,
    projections: &[archivefs_core::emulator_lifecycle::EmulatorLifecycleProjection],
    app: &mut ArchiveFsApp,
) {
    ui.heading("Emulator lifecycle health");
    ui.label("This read-only view explains which installation is selected and who can update it. EmuWiz never runs package-manager updates here.");
    for projection in projections {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(&projection.emulator_id);
                ui.label(lifecycle_state_label(projection.state));
            });
            if projection.stale_selected {
                ui.colored_label(
                    egui::Color32::from_rgb(235, 120, 100),
                    "Your selected installation can no longer be found.",
                );
            }
            if projection.installations.is_empty() {
                ui.label("Not installed");
            }
            for installation in &projection.installations {
                ui.group(|ui| {
                    let marker = if installation.selected {
                        " · Currently selected"
                    } else {
                        ""
                    };
                    ui.label(format!(
                        "{}{}",
                        installation_type_label(installation.installation_type),
                        marker
                    ));
                    ui.label(format!(
                        "Version: {} · Updates: {}",
                        installation.version.version.as_deref().unwrap_or("unknown"),
                        update_authority_label(installation.update_authority)
                    ));
                    ui.label(format!(
                        "Health: {} · Launch: {}",
                        local_health_label(installation.local_health),
                        installation
                            .launch_readiness
                            .map(launch_readiness_label)
                            .unwrap_or("Not checked")
                    ));
                    ui.label(format!(
                        "Update status: {}",
                        installation
                            .update_status
                            .map(update_status_label)
                            .unwrap_or("Latest version unknown")
                    ));
                    if !installation.selected
                        && let Some(path) = lifecycle_executable_path(&installation.exact_binding)
                        && let Some(emulator) = overridable_emulator(&projection.emulator_id)
                        && ui.button("Use this installation").clicked()
                    {
                        app.emulator_readiness
                            .emulator_setup_overrides
                            .set_executable(emulator, Some(path.to_path_buf()));
                        ui.label("Selection saved through the existing emulator setup backend. Refresh to inspect it again.");
                    }
                    ui.collapsing("Advanced details", |ui| {
                        ui.label(format!("Ownership: {}", ownership_label(installation.ownership_category)));
                        ui.label(format!("Channel: {:?} · Version source: {:?}", installation.channel, installation.version.source));
                        match &installation.exact_binding {
                            archivefs_core::emulator_lifecycle::ExactBinding::FlatpakApp { app_id } => {
                                ui.label(format!("Flatpak app ID: {app_id}"));
                                ui.label("Update using Flatpak; EmuWiz does not run that update.");
                            }
                            archivefs_core::emulator_lifecycle::ExactBinding::ManagedInstall { manifest_path, executable_path } => {
                                ui.label(format!("Manifest: {}", manifest_path.display()));
                                ui.label(format!("Executable: {}", executable_path.display()));
                            }
                            _ => {
                                if let Some(path) = lifecycle_executable_path(&installation.exact_binding) {
                                    ui.label(format!("Executable: {}", path.display()));
                                }
                            }
                        }
                    });
                });
            }
        });
    }
    ui.separator();
}

fn lifecycle_state_label(
    state: archivefs_core::emulator_lifecycle::LifecycleState,
) -> &'static str {
    use archivefs_core::emulator_lifecycle::LifecycleState::*;
    match state {
        InstalledCurrent => "Installed",
        InstalledUpdateAvailable => "Update available",
        InstalledUnknownVersion => "Installed · version unknown",
        InstalledUnsupportedVersion => "Installed · unsupported version",
        Missing => "Not installed",
        Broken => "Needs attention",
        MultipleInstallations => "Multiple installations found",
        ManagedExternally => "Managed by another installer",
    }
}

fn installation_type_label(
    kind: archivefs_core::emulator_inventory::InstallationType,
) -> &'static str {
    use archivefs_core::emulator_inventory::InstallationType::*;
    match kind {
        Flatpak => "Flatpak",
        SystemPackage => "System package",
        AppImage => "AppImage",
        Portable => "Portable installation",
        Managed => "EmuWiz-managed installation",
        Manual => "Installed manually",
        Unknown => "Installation type unknown",
    }
}

fn ownership_label(
    ownership: archivefs_core::emulator_lifecycle::OwnershipCategory,
) -> &'static str {
    use archivefs_core::emulator_lifecycle::OwnershipCategory::*;
    match ownership {
        OfficialManaged => "Managed by EmuWiz",
        OfficialBrowserHandoff => "Official download handoff",
        FlatpakManaged => "Managed by Flatpak",
        SystemPackageManaged => "Managed by your system",
        PortableUserManaged => "Installed manually",
        Unknown => "Installation source unknown",
        DoNotAutomate => "Manual updates only",
    }
}

fn update_authority_label(
    authority: archivefs_core::emulator_lifecycle::UpdateAuthority,
) -> &'static str {
    use archivefs_core::emulator_lifecycle::UpdateAuthority::*;
    match authority {
        EmuWizManaged => "EmuWiz",
        Flatpak => "Flatpak",
        SystemPackageManager => "your system package manager",
        OfficialBrowser => "official download page",
        UserManaged => "manual updates",
        None => "no automatic updater",
        Unknown => "unknown",
    }
}

fn local_health_label(health: archivefs_core::emulator_lifecycle::LocalHealth) -> &'static str {
    use archivefs_core::emulator_lifecycle::LocalHealth::*;
    match health {
        Healthy => "Healthy",
        MissingExecutable => "Selected executable missing",
        ChangedExecutable => "Executable changed",
        ManifestMismatch => "Managed manifest mismatch",
        PermissionsProblem => "Permissions need attention",
        UnknownVersion => "Version unknown",
        StaleConfiguredPath => "Selected path is stale",
        MultipleCandidates => "Multiple candidates",
        BrokenProfile => "Profile needs attention",
        Unknown => "Unknown",
    }
}

fn launch_readiness_label(
    readiness: archivefs_core::launch::readiness::LaunchReadiness,
) -> &'static str {
    use archivefs_core::launch::readiness::LaunchReadiness::*;
    match readiness {
        Ready => "Ready",
        ReadyWithWarnings => "Ready with warnings",
        Blocked => "Blocked",
    }
}

fn update_status_label(status: archivefs_core::emulator_update::UpdateStatus) -> &'static str {
    use archivefs_core::emulator_update::UpdateStatus::*;
    match status {
        UpToDate => "Up to date",
        UpdateAvailable => "Update available",
        InstalledNewer => "Installed version is newer",
        VersionUnknown => "Version unknown",
        LatestUnknown => "Latest version unknown",
        ChannelMismatch => "Channel mismatch",
        ComparisonUnsupported => "Cannot compare versions",
        Offline => "Offline",
    }
}

fn lifecycle_executable_path(
    binding: &archivefs_core::emulator_lifecycle::ExactBinding,
) -> Option<&std::path::Path> {
    match binding {
        archivefs_core::emulator_lifecycle::ExactBinding::NativeExecutable { path }
        | archivefs_core::emulator_lifecycle::ExactBinding::PortableExecutable { path }
        | archivefs_core::emulator_lifecycle::ExactBinding::UnknownExternal { path } => Some(path),
        archivefs_core::emulator_lifecycle::ExactBinding::ManagedInstall {
            executable_path,
            ..
        } => Some(executable_path),
        archivefs_core::emulator_lifecycle::ExactBinding::FlatpakApp { .. } => None,
    }
}

fn overridable_emulator(
    emulator_id: &str,
) -> Option<crate::emulator_setup_overrides::OverridableEmulator> {
    crate::emulator_setup_overrides::OverridableEmulator::from_adapter_id(
        &emulator_id.to_ascii_lowercase(),
    )
}

pub(super) fn observe_dat_activity_state(
    activity: &mut Activity,
    job: &mut Option<u64>,
    state: Option<crate::dat_sources_page::DatBackgroundActivity>,
    error: Option<String>,
) {
    if let Some(state) = state {
        if job.is_none() {
            let id = activity.queue(
                state.title,
                Route::Section(super::routes::Section::Dat),
                false,
            );
            activity.start(id);
            *job = Some(id);
        }
        if let Some(active) = job.and_then(|id| activity.jobs.get_mut(&id)) {
            active.item = Some(state.detail);
        }
    } else if let Some(id) = job.take() {
        activity.finish(
            id,
            if error.is_some() {
                "The verification-data operation stopped safely.".into()
            } else {
                "Verification data is ready to review.".into()
            },
            error,
        );
    }
}

pub(super) fn observe_cheat_activity_state(
    activity: &mut Activity,
    job: &mut Option<u64>,
    state: Option<(&'static str, &'static str)>,
    error: Option<String>,
) {
    if let Some((title, detail)) = state {
        if job.is_none() {
            let id = activity.queue(title, Route::Section(super::routes::Section::Mods), false);
            activity.start(id);
            *job = Some(id);
        }
        if let Some(active) = job.and_then(|id| activity.jobs.get_mut(&id)) {
            active.item = Some(detail.to_string());
        }
    } else if let Some(id) = job.take() {
        activity.finish(
            id,
            if error.is_some() {
                "The cheat operation stopped safely; nothing unapproved was applied.".into()
            } else {
                "Cheat information is ready to review.".into()
            },
            error,
        );
    }
}

fn cheat_activity_state(
    workflow: Option<&crate::CheatWorkflowState>,
) -> Option<(&'static str, &'static str)> {
    let workflow = workflow?;
    use crate::{CheatStepResource as R, CheatTransactionState as T};
    if matches!(workflow.transaction, T::Applying { .. }) {
        return Some(("Applying cheats", "Updating the selected emulator profile."));
    }
    if matches!(workflow.preview, R::Loading { .. }) {
        return Some((
            "Checking cheat compatibility",
            "Building the safe apply preview.",
        ));
    }
    if matches!(workflow.identity, R::Loading { .. })
        || matches!(workflow.source_list, R::Loading { .. })
        || matches!(workflow.source_fetch, R::Loading { .. })
        || matches!(workflow.candidates, R::Loading { .. })
        || matches!(workflow.existing_library, R::Loading { .. })
        || matches!(workflow.pcsx2_inventory, R::Loading { .. })
        || matches!(workflow.dolphin_inventory, R::Loading { .. })
        || matches!(workflow.pcsx2_gamehacking, R::Loading { .. })
        || matches!(workflow.gamecube_gamehacking, R::Loading { .. })
        || matches!(workflow.dolphin_provider, R::Loading { .. })
        || matches!(workflow.xenia_provider, R::Loading { .. })
        || matches!(workflow.bsfree_gamecube, R::Loading { .. })
        || matches!(workflow.bsfree_wii, R::Loading { .. })
    {
        return Some((
            "Reading cheats",
            "Checking identity, sources and compatibility.",
        ));
    }
    None
}
