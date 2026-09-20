//! Native v2 host for the established emulator setup and launch coordinator.
//!
//! The coordinator remains the single owner of evidence gathering, adapter
//! discovery, launch planning and process execution. This bridge only gives
//! those existing workflows v2 navigation and Activity integration.

use super::{activity::Activity, routes::Route};
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
        let mut app = ArchiveFsApp::new(context.clone());
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

    pub(super) fn show_setup(&mut self, ui: &mut egui::Ui) {
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
                .add_enabled(self.app.source_action_available(), egui::Button::new("Add source"))
                .clicked()
            {
                self.app.sources_ui.sources_add_dialog =
                    Some(crate::source_controller::SourcesAddDialogState::default());
            }
            ui.label("Add and scan existing folders; source files are never moved or deleted.");
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
    }

    pub(super) fn take_source_library_reload(&mut self) -> bool {
        std::mem::take(&mut self.source_library_reload)
    }

    pub(super) fn take_artwork_reload(&mut self) -> bool {
        std::mem::take(&mut self.artwork_reload)
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
                LoadState::Ready(data) => data.records.iter().find(|record| record.mount_plan.archive.path == path),
                LoadState::Loading { previous, .. } => previous.as_ref().and_then(|data| data.records.iter().find(|record| record.mount_plan.archive.path == path)),
                LoadState::Error(_) => None,
            }.cloned();
            if let Some(record) = record {
                let existing = self.app.database_state.snapshot()
                    .and_then(|snapshot| snapshot.screenscraper_enrichments.get(&game_id))
                    .cloned();
                if let Some(crate::screenscraper_enrichment_page::ScreenScraperEnrichmentAction::Apply { archive_id, values, receipt }) =
                    crate::screenscraper_enrichment_page::show(
                        ui,
                        &mut self.app.screenscraper_enrichment,
                        &self.app.screenscraper_page,
                        &record,
                        game_id,
                        existing.as_ref(),
                    )
                {
                    self.app.apply_screenscraper_enrichment(context, archive_id, values, receipt);
                    changed = self.app.feedback.as_ref().is_some_and(|feedback| feedback.succeeded);
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
            let job = activity.queue(
                "Refreshing metadata",
                self.selected_route(),
                false,
            );
            activity.start(job);
            self.metadata_job = Some(job);
        } else if !running
            && let Some(job) = self.metadata_job.take()
        {
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
        } else if !loading
            && let Some(job) = self.provider_job.take()
        {
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
