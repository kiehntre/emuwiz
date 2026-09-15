use std::path::{Path, PathBuf};

use crate::*;

impl ArchiveFsApp {
    fn feature_discovery_context(
        &self,
        selected_path: Option<&std::path::Path>,
    ) -> feature_discovery::FeatureDiscoveryContext {
        use feature_discovery::{FeatureDiscoveryContext, FeatureStatus};

        let cheats = selected_path
            .and_then(|path| {
                self.cheat_workflow
                    .as_ref()
                    .filter(|w| w.archive_path == path)
            })
            .map(|_| FeatureStatus::Available {
                label: "Cheat workflow available".to_string(),
                action_label: Some("Review cheats"),
                action: Some(feature_discovery::FeatureDiscoveryAction::OpenCheats),
            });

        let romm = self.romm_ui.snapshot.as_deref().map(|snapshot| {
            let stale = snapshot
                .verify_summary
                .map(|summary| summary.stale + summary.unmatched)
                .unwrap_or(0);
            use archivefs_core::identity_source::status::ProviderState;
            match (&snapshot.status.state, stale) {
                (ProviderState::Ready | ProviderState::ReadyOffline, 0) => {
                    FeatureStatus::Available {
                        label: "RomM is up to date".to_string(),
                        action_label: Some("Open RomM"),
                        action: Some(feature_discovery::FeatureDiscoveryAction::OpenRomm),
                    }
                }
                (ProviderState::Ready | ProviderState::ReadyOffline, count) => {
                    FeatureStatus::NeedsAttention {
                        label: format!("RomM needs updating ({count} records)"),
                        action_label: Some("Review RomM"),
                        action: Some(feature_discovery::FeatureDiscoveryAction::OpenRomm),
                    }
                }
                (state, _) => FeatureStatus::Unavailable {
                    label: "RomM status".to_string(),
                    reason: format!("RomM is {}.", state.label()),
                },
            }
        });

        let emulator = Some(match setup_check_summary(&self.doctor_scan) {
            home_page::SetupCheckSummary::Healthy => FeatureStatus::Available {
                label: "Emulator setup checks passed".to_string(),
                action_label: Some("Open Emulator Setup"),
                action: Some(feature_discovery::FeatureDiscoveryAction::OpenEmulatorSetup),
            },
            home_page::SetupCheckSummary::Warnings(count)
            | home_page::SetupCheckSummary::NeedsAttention(count) => {
                FeatureStatus::NeedsAttention {
                    label: format!("Emulator setup needs attention ({count})"),
                    action_label: Some("Set up emulator"),
                    action: Some(feature_discovery::FeatureDiscoveryAction::OpenEmulatorSetup),
                }
            }
            home_page::SetupCheckSummary::NeverRun
            | home_page::SetupCheckSummary::Running
            | home_page::SetupCheckSummary::NoChecksRun => FeatureStatus::Unavailable {
                label: "Emulator readiness".to_string(),
                reason: "Emulator setup has not completed a usable check yet.".to_string(),
            },
        });

        let media = selected_path.map(|path| FeatureDiscoveryContext {
            cheats,
            romm,
            emulator,
            cover_available: Some(matches!(
                self.gamer_covers.slot_for(path, None),
                Some(crate::gamer_artwork::CoverSlot::Ready { .. })
            )),
            screenshot_count: self.gamer_screenshots.screenshot_count(path),
            video_available: None,
        });
        media.unwrap_or_default()
    }

    pub(crate) fn museum_selected_game(&self) -> Option<museum_page::MuseumSelectedGameView> {
        let path = self.archive_context.focused.as_ref()?;
        let record = match &self.state {
            LoadState::Ready(data) => data
                .records
                .iter()
                .find(|record| record.mount_plan.archive.path == *path)?,
            _ => return None,
        };
        let platform = record
            .identity
            .platform
            .as_deref()
            .or(record.metadata.platform.as_deref())?
            .to_string();
        let video_available = match &self.selected_evidence_ui.selected_evidence {
            selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                if report.path == *path =>
            {
                report.structural_media.as_ref().map(|media| {
                    matches!(
                        media,
                        selected_evidence_page::StructuralMediaDetails::LaserDisc(details)
                            if !details.present_media.is_empty()
                    )
                })
            }
            _ => None,
        };
        let evidence_report = match &self.selected_evidence_ui.selected_evidence {
            selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                if report.path == *path =>
            {
                Some(report)
            }
            _ => None,
        };
        let discovery_context = self.feature_discovery_context(Some(path));
        let feature_view = evidence_report.map(|report| {
            feature_discovery::build_feature_discovery_with_context(report, &discovery_context)
        });
        let mut facts = vec![
            (
                "Media format".to_string(),
                archive_kind_name(record.mount_plan.archive.kind).to_string(),
            ),
            (
                "File size".to_string(),
                format_size(record.identity.size_bytes),
            ),
            (
                "Identity strength".to_string(),
                evidence_report
                    .map(|report| {
                        gamer_identity_status_from_verdict(report.identity.status)
                            .label()
                            .to_string()
                    })
                    .unwrap_or_else(|| "Evidence not loaded".to_string()),
            ),
        ];
        if let Some(region) = record
            .metadata
            .region
            .as_deref()
            .or(record.identity.region.as_deref())
        {
            facts.push(("Region".to_string(), region.to_string()));
        }
        if let Some(version) = record.metadata.version.as_deref() {
            facts.push(("Version".to_string(), version.to_string()));
        }
        if let Some(preferred_emulator) = archivefs_core::platform::PLATFORMS
            .iter()
            .find(|registered| registered.display_name == platform)
            .and_then(|registered| registered.preferred_emulator)
        {
            facts.push((
                "Preferred emulator".to_string(),
                preferred_emulator.to_string(),
            ));
        }
        let evidence_highlights = evidence_report
            .map(|report| {
                let mut highlights = Vec::new();
                if matches!(
                    report.identity.status,
                    archivefs_core::platform_evidence_fusion::identity_presentation::IdentityStatus::VerifiedByDat
                        | archivefs_core::platform_evidence_fusion::identity_presentation::IdentityStatus::ContentAndDatAgree
                ) {
                    highlights.push("DAT identity verified".to_string());
                }
                if report.hashes.is_some() {
                    highlights.push("Checksums computed".to_string());
                }
                if report.tape_analysis.is_some() {
                    highlights.push("Tape analysis available".to_string());
                }
                highlights.extend(report.structural_facts.iter().take(2).map(|fact| {
                    format!("{}: {}", fact.detail, fact.value)
                }));
                highlights
            })
            .unwrap_or_default();
        Some(museum_page::MuseumSelectedGameView {
            archive_path: path.clone(),
            title: gamer_view::gamer_display_title(record),
            platform,
            facts,
            evidence_highlights,
            feature_view,
            screenshot_count: self.gamer_screenshots.screenshot_count(path),
            video_available,
            dat_verified: matches!(
                &self.selected_evidence_ui.selected_evidence,
                selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                    if report.path == *path
                        && matches!(
                            report.identity.status,
                            archivefs_core::platform_evidence_fusion::identity_presentation::IdentityStatus::VerifiedByDat
                                | archivefs_core::platform_evidence_fusion::identity_presentation::IdentityStatus::ContentAndDatAgree
                        )
            ),
        })
    }

    /// Phase 5: "Review" on a Gamer View game whose platform couldn't be
    /// confidently identified. Keeps the exact same game selected (so
    /// Library's Game Details area opens already showing its real
    /// identity/evidence detail, not a blank/generic page) while
    /// switching to the mode that can actually render it - the identical
    /// select-then-switch shape `open_cheats_mods_workspace` and the
    /// Phase 4 Undo fix both already use.
    pub(crate) fn review_identity(&mut self, archive_path: PathBuf) {
        self.archive_context.select_only(archive_path);
        self.ui_mode = GuiMode::AdvancedView;
        save_gui_mode(self.ui_mode);
        self.navigate_to_library_tab(LibraryTab::Archives);
    }

    /// "Open Emulator Setup" from a Gamer View `NeedsSetup` card: keep the
    /// same game selected and switch to Advanced View's Emulator Setup
    /// page. Same select-then-navigate shape as `review_identity`; no new
    /// plumbing and nothing about the game changes. `focus` records which
    /// repair card the page should scroll into view once (consumed by
    /// `show_emulator_setup_page` with `take()`); sidebar/Home navigation
    /// never sets it.
    pub(crate) fn open_emulator_setup_for(
        &mut self,
        archive_path: PathBuf,
        focus: EmulatorSetupFocus,
    ) {
        self.archive_context.select_only(archive_path);
        self.ui_mode = GuiMode::AdvancedView;
        save_gui_mode(self.ui_mode);
        self.emulator_setup_focus = Some(focus);
        self.navigate_to_main_view(MainView::EmulatorSetup);
    }

    /// Render the focused archive's complete Game Details surface.  Library
    /// owns the selection now; this method is shared with the retained
    /// internal Selected compatibility route so there is still only one
    /// renderer and one set of readiness/evidence actions.
    pub(crate) fn show_game_details(
        &mut self,
        context: &egui::Context,
        ui: &mut egui::Ui,
        archive_actions_blocked: bool,
        archive_action_block_reason: Option<&'static str>,
    ) -> Option<MountPageAction> {
        if let Some(path) = self.archive_context.focused.clone() {
            let should_load_evidence = match &self.selected_evidence_ui.selected_evidence {
                selected_evidence_page::SelectedEvidenceState::Loading {
                    path: loading_path,
                    ..
                } => loading_path != &path,
                selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                    report.path != path
                }
                selected_evidence_page::SelectedEvidenceState::Idle => true,
                selected_evidence_page::SelectedEvidenceState::Error {
                    path: error_path, ..
                } => error_path != &path,
            };
            if should_load_evidence {
                self.start_selected_evidence_load(context.clone(), path);
            }
        }
        self.maybe_start_selected_evidence_enrichment(context);
        if matches!(
            self.dolphin_local_profiles,
            DolphinLocalProfilesState::NotScanned
        ) {
            self.start_dolphin_local_profile_scan(context.clone());
        }
        if matches!(
            self.pcsx2_launch_profiles,
            Pcsx2LaunchProfilesState::NotScanned
        ) {
            self.start_pcsx2_launch_profile_scan(context.clone());
        }
        if matches!(
            self.pcsx2_firmware_evidence,
            Pcsx2FirmwareEvidenceState::NotLoaded
        ) {
            self.start_pcsx2_firmware_evidence_load(context.clone());
        }
        if matches!(self.flycast_profiles, FlycastProfilesState::NotScanned) {
            self.start_flycast_profile_scan(context.clone());
        }
        if matches!(
            self.selected_evidence_ui.scummvm_readiness,
            identity_sources_page::ScummVmReadinessState::NotChecked
        ) {
            self.start_scummvm_readiness_check(context.clone());
        }
        let live = match &self.state {
            LoadState::Ready(data) => Some(data.as_ref()),
            _ => None,
        };
        let action = show_selected_page(
            ui,
            live,
            SelectedPageViewState {
                selected_archive: self.archive_context.focused.as_deref(),
                selected_count: self.archive_context.selected.len(),
                retroarch_profiles: &self.retroarch_profiles,
                busy: archive_actions_blocked,
                block_reason: archive_action_block_reason,
            },
        );
        ui.add_space(crate::ui::theme::SECTION_GAP);
        self.show_romm_game_panel(context, ui);
        ui.add_space(crate::ui::theme::SECTION_GAP);
        let evidence_action = selected_evidence_page::show_selected_evidence_panel(
            ui,
            self.ui_mode == GuiMode::AdvancedView,
            self.archive_context.focused.as_deref(),
            &self.selected_evidence_ui.selected_evidence,
        );
        self.handle_selected_evidence_action(context, evidence_action);
        ui.add_space(crate::ui::theme::SECTION_GAP);
        if let Some(path) = self
            .archive_context
            .focused
            .as_deref()
            .map(Path::to_path_buf)
        {
            if let Some(snapshot) = self.database_state.snapshot() {
                self.media_sets_page
                    .refresh(&snapshot.archives, self.database_generation.0);
            }
            if media_sets_page::show_selected_item_link(ui, &self.media_sets_page, &path) {
                self.navigate_to_main_view(MainView::MediaSets);
            }
            ui.add_space(crate::ui::theme::SECTION_GAP);
        }
        let live_for_launch_readiness = match &self.state {
            LoadState::Ready(data) => Some(data.as_ref()),
            _ => None,
        };
        let scummvm_candidate_count = live_for_launch_readiness
            .map(|data| identity_sources_page::scummvm_candidates_from_rows(&data.rows).len())
            .unwrap_or(0);
        match &self.flycast_profiles {
            FlycastProfilesState::Scanning { .. } => {
                ui.label("Checking Flycast installation and Dreamcast BIOS readiness…");
            }
            FlycastProfilesState::Error(message) => {
                widgets::banner(
                    ui,
                    "Flycast readiness could not be checked",
                    message,
                    widgets::StatusTone::Warning,
                );
            }
            FlycastProfilesState::NotScanned | FlycastProfilesState::Ready(_) => {}
        }
        let launch_readiness_input = self.build_launch_readiness_input(live_for_launch_readiness);
        if self.launch_retroarch.poll() || self.launch_retroarch.is_active() {
            ui.ctx().request_repaint();
        }
        if self.launch_dolphin.poll() || self.launch_dolphin.is_active() {
            ui.ctx().request_repaint();
        }
        if self.launch_pcsx2.poll() || self.launch_pcsx2.is_active() {
            ui.ctx().request_repaint();
        }
        if self.launch_standalone.poll() || self.launch_standalone.is_active() {
            ui.ctx().request_repaint();
        }
        if self.launch_amiga_whdload.poll() || self.launch_amiga_whdload.is_active() {
            ui.ctx().request_repaint();
        }
        let launch_readiness_action = launch_readiness_page::show_launch_readiness_panel(
            ui,
            &launch_readiness_input,
            &mut self.launch_retroarch,
            &mut self.launch_dolphin,
            &mut self.launch_pcsx2,
            &mut self.launch_standalone,
            &mut self.launch_amiga_whdload,
        );
        if matches!(
            launch_readiness_action,
            Some(launch_readiness_page::LaunchReadinessPageAction::OpenDoctor)
        ) {
            self.navigate_to_main_view(MainView::Doctor);
        }
        ui.add_space(crate::ui::theme::SECTION_GAP);
        let identity_sources_action = identity_sources_page::show_identity_sources_panel(
            ui,
            self.ui_mode == GuiMode::AdvancedView,
            &self.selected_evidence_ui.identity_sources,
        );
        self.handle_identity_sources_action(context, identity_sources_action);
        ui.add_space(crate::ui::theme::SECTION_GAP);
        self.poll_scummvm_readiness();
        self.poll_scummvm_check();
        let scummvm_action = identity_sources_page::show_scummvm_detection_panel(
            ui,
            &self.selected_evidence_ui.scummvm_readiness,
            scummvm_candidate_count,
            &self.selected_evidence_ui.scummvm_check,
        );
        self.handle_scummvm_action(context, scummvm_action);
        ui.add_space(crate::ui::theme::SECTION_GAP);
        let plan_preview_action = plan_preview_page::show_plan_preview_panel(
            ui,
            self.ui_mode == GuiMode::AdvancedView,
            self.archive_context.focused.as_deref(),
            &self.selected_evidence_ui.plan_preview,
        );
        self.handle_plan_preview_action(context, plan_preview_action);
        ui.add_space(crate::ui::theme::SECTION_GAP);
        let rpcs3_action = rpcs3_page::show_rpcs3_panel(
            ui,
            self.ui_mode == GuiMode::AdvancedView,
            None,
            &self.rpcs3_status,
        );
        self.handle_rpcs3_action(context, rpcs3_action);
        ui.add_space(crate::ui::theme::SECTION_GAP);
        let focused_archive = self.archive_context.focused.clone();
        self.invalidate_pcsx2_status_if_selection_changed(focused_archive.as_deref());
        let verified_ps2_serial = self
            .cheat_workflow
            .as_ref()
            .and_then(pcsx2_identity_for_workflow)
            .and_then(|id| id.serial);
        let pcsx2_action = pcsx2_page::show_pcsx2_panel(
            ui,
            self.ui_mode == GuiMode::AdvancedView,
            verified_ps2_serial.as_deref(),
            &self.pcsx2_status,
        );
        self.handle_pcsx2_action(context, pcsx2_action);
        Some(action).flatten()
    }
}
