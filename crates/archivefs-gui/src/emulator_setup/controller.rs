use crate::*;

use super::profile_resolution::ppsspp::{
    PpssppResolutionInputs, PpssppResolutionState, spawn_ppsspp_resolution,
};
use super::profile_resolution::{
    DuckStationLaunchAuthority, DuckStationLaunchSelection, DuckStationResolutionInputs,
    DuckStationResolutionState, DuckStationResolutionView, ResolutionView, ResolverSubject,
    present_resolution, present_resolution_for, select_for_launch, show_profile_resolution,
    spawn_duckstation_resolution,
};

impl ArchiveFsApp {
    /// The persisted catalogue row for `path` when the scan's detector catalogued
    /// it as a ScummVM game folder. The scan, not the filesystem scanner that
    /// feeds the live record list, is what discovers these.
    /// The runnable content for `path` when the page's persisted catalogue
    /// row is never-mounted content that is still a regular file (see
    /// `catalogued_loose_content`), by core's own per-kind rule; else `None`.
    pub(crate) fn catalogued_loose_content_ref(
        &self,
        path: &Path,
    ) -> Option<archivefs_core::launch::LaunchContentRef> {
        let (row_path, kind) = self.catalogued_loose_content.as_ref()?;
        if row_path != path || !std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()) {
            return None;
        }
        archivefs_core::launch::launch_content_ref_for_unmounted(*kind, path)
    }

    pub(crate) fn persisted_scummvm_folder(
        &self,
        path: &Path,
    ) -> Option<&archivefs_core::PersistedArchive> {
        let matches = |archive: &&archivefs_core::PersistedArchive| {
            archive.archive_kind == "scummvm_directory" && archive.absolute_path == path
        };
        self.catalogued_scummvm_folder
            .as_ref()
            .filter(matches)
            .or_else(|| {
                self.database_state
                    .snapshot()?
                    .archives
                    .iter()
                    .find(matches)
            })
    }

    /// Emulator Adapter Batch B: starts the one real, background RPCS3
    /// environment check. Explicit only (a button press); never called
    /// automatically. `verified_title_id` must already be authoritative -
    /// this method never derives one; see `rpcs3_page`'s own module doc.
    pub(crate) fn start_rpcs3_status_load(
        &mut self,
        context: egui::Context,
        verified_title_id: Option<String>,
    ) {
        self.emulator_readiness.rpcs3_status_generation += 1;
        let generation = self.emulator_readiness.rpcs3_status_generation;
        let (sender, receiver) = mpsc::channel();
        self.emulator_readiness.rpcs3_status = rpcs3_page::Rpcs3State::Loading {
            generation,
            receiver,
        };
        thread::spawn(move || {
            let outcome = rpcs3_page::gather_rpcs3_status(verified_title_id);
            let _ = sender.send((generation, outcome));
            context.request_repaint();
        });
    }

    /// Emulator Adapter Batch B: applies the pure
    /// [`rpcs3_page::Rpcs3Action`] the panel returned this frame - the
    /// only thing it can ever ask for, and read-only.
    pub(crate) fn handle_rpcs3_action(
        &mut self,
        context: &egui::Context,
        action: Option<rpcs3_page::Rpcs3Action>,
    ) {
        if let Some(rpcs3_page::Rpcs3Action::Load) = action {
            // No selected-ROM identity pipeline reaches this branch of the
            // GUI yet (see `rpcs3_page`'s own module doc) - always `None`
            // for now, so this panel only ever shows RPCS3's own
            // environment health, never a game mapping. Once a verified
            // PS3 title ID becomes available here, thread it through
            // unchanged rather than deriving one in this method.
            self.start_rpcs3_status_load(context.clone(), None);
        }
    }

    /// Emulator Adapter Batch B: drains a completed RPCS3 status check,
    /// discarding anything whose generation is no longer current - the
    /// same stale-result guard every other background loader in this app
    /// uses.
    pub(crate) fn poll_rpcs3_status(&mut self) {
        if let rpcs3_page::Rpcs3State::Loading {
            generation,
            receiver,
        } = &self.emulator_readiness.rpcs3_status
            && let Ok((message_generation, outcome)) = receiver.try_recv()
            && message_generation == *generation
        {
            self.emulator_readiness.rpcs3_status = rpcs3_page::Rpcs3State::Ready {
                generation: message_generation,
                outcome,
            };
        }
    }

    pub(crate) fn start_pcsx2_status_load(
        &mut self,
        context: egui::Context,
        archive_path: Option<PathBuf>,
        verified_ps2_serial: Option<String>,
        verified_executable_crc: Option<String>,
    ) {
        self.emulator_readiness.pcsx2_status_generation += 1;
        let generation = self.emulator_readiness.pcsx2_status_generation;
        self.emulator_readiness.pcsx2_status_archive_path = archive_path;
        let (sender, receiver) = mpsc::channel();
        self.emulator_readiness.pcsx2_status = pcsx2_page::Pcsx2StatusState::Loading {
            generation,
            receiver,
        };
        thread::spawn(move || {
            let outcome =
                pcsx2_page::gather_pcsx2_status(verified_ps2_serial, verified_executable_crc);
            let _ = sender.send((generation, outcome));
            context.request_repaint();
        });
    }

    /// PCSX2 GUI Integration Batch H2: resets the PCSX2 status panel to
    /// `Idle` whenever the focused selected-ROM archive has changed since
    /// the current (or in-flight) result was loaded for, so a stale
    /// per-title mapping for the previous selection is never shown against
    /// the new one. Cheap no-op when the selection has not changed. Called
    /// once per frame right before rendering the panel.
    pub(crate) fn invalidate_pcsx2_status_if_selection_changed(
        &mut self,
        focused_archive: Option<&Path>,
    ) {
        if self.emulator_readiness.pcsx2_status_archive_path.as_deref() != focused_archive {
            self.emulator_readiness.pcsx2_status = pcsx2_page::Pcsx2StatusState::Idle;
            self.emulator_readiness.pcsx2_status_archive_path =
                focused_archive.map(Path::to_path_buf);
        }
    }

    /// PCSX2 GUI Integration Batch H2: applies the pure
    /// [`pcsx2_page::Pcsx2StatusAction`] the panel returned this frame -
    /// the only thing it can ever ask for, and read-only. The verified PS2
    /// serial/CRC come exclusively from `pcsx2_identity_for_workflow`,
    /// which itself only ever surfaces a value core already marked
    /// `IdentityStatus::Verified` (see that helper and
    /// `Pcsx2GameIdentity::from_report`) - an unresolved, ambiguous, or
    /// conflicting selection, or one for which no cheat workflow has been
    /// opened yet, yields `None` here, exactly like RPCS3's still-pending
    /// wiring above.
    pub(crate) fn handle_pcsx2_action(
        &mut self,
        context: &egui::Context,
        action: Option<pcsx2_page::Pcsx2StatusAction>,
    ) {
        if let Some(pcsx2_page::Pcsx2StatusAction::ChooseMemoryCard) = action {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose PS2 memory-card image to inspect")
                .pick_file()
            {
                pcsx2_page::save_saved_ps2_card_path(&path);
                self.start_pcsx2_manual_card_load(context.clone(), path);
            }
        } else if let Some(pcsx2_page::Pcsx2StatusAction::UsePcsx2MemoryCard) = action {
            self.emulator_readiness.pcsx2_save_vault.source =
                pcsx2_page::Pcsx2SaveCardSource::Pcsx2;
            self.emulator_readiness.pcsx2_save_vault.manual_inventory = None;
            self.emulator_readiness.pcsx2_save_vault.manual_loading = None;
        } else if let Some(pcsx2_page::Pcsx2StatusAction::Load) = action {
            let identity = self
                .cheat_workflow
                .as_ref()
                .and_then(pcsx2_identity_for_workflow);
            let verified_ps2_serial = identity.as_ref().and_then(|id| id.serial.clone());
            let verified_executable_crc = identity
                .as_ref()
                .and_then(|id| id.verified_crc())
                .map(str::to_string);
            let archive_path = self.archive_context.focused.clone();
            self.start_pcsx2_status_load(
                context.clone(),
                archive_path,
                verified_ps2_serial,
                verified_executable_crc,
            );
        }
    }

    pub(crate) fn start_pcsx2_manual_card_load(&mut self, context: egui::Context, path: PathBuf) {
        let (sender, receiver) = mpsc::channel();
        let inspection_path = path.clone();
        self.emulator_readiness.pcsx2_save_vault.source = pcsx2_page::Pcsx2SaveCardSource::Manual;
        self.emulator_readiness.pcsx2_save_vault.manual_path = Some(path);
        self.emulator_readiness.pcsx2_save_vault.manual_inventory = None;
        self.emulator_readiness.pcsx2_save_vault.manual_loading = Some(receiver);
        thread::spawn(move || {
            let result =
                archivefs_core::memory_card_inventory::inspect_memory_card(&inspection_path)
                    .map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_pcsx2_manual_card(&mut self) {
        let Some(receiver) = self
            .emulator_readiness
            .pcsx2_save_vault
            .manual_loading
            .as_ref()
        else {
            return;
        };
        if let Ok(result) = receiver.try_recv() {
            self.emulator_readiness.pcsx2_save_vault.manual_loading = None;
            self.emulator_readiness.pcsx2_save_vault.manual_inventory = Some(result);
        }
    }

    /// PCSX2 GUI Integration Batch H2: drains a completed PCSX2 status
    /// check, discarding anything whose generation is no longer current -
    /// the same stale-result guard every other background loader in this
    /// app uses.
    pub(crate) fn poll_pcsx2_status(&mut self) {
        if let pcsx2_page::Pcsx2StatusState::Loading {
            generation,
            receiver,
        } = &self.emulator_readiness.pcsx2_status
            && let Ok((message_generation, outcome)) = receiver.try_recv()
            && message_generation == *generation
        {
            self.emulator_readiness.pcsx2_status = pcsx2_page::Pcsx2StatusState::Ready {
                generation: message_generation,
                outcome,
            };
        }
    }

    /// Applies the pure [`emulator_setup_page::EmulatorSetupAction`] the page
    /// returned this frame - the only thing it can ever ask for. An override
    /// change is persisted through `emulator_setup_overrides` and always
    /// followed by the same read-only Doctor rescan a manual "Check
    /// emulators" click starts, so remediation and manual checks share one
    /// code path and one notion of "up to date".
    pub(crate) fn handle_emulator_setup_action(
        &mut self,
        action: Option<emulator_setup_page::EmulatorSetupAction>,
        context: &egui::Context,
    ) {
        match action {
            Some(emulator_setup_page::EmulatorSetupAction::CheckEmulators) => {
                self.start_doctor_scan(context.clone());
            }
            Some(emulator_setup_page::EmulatorSetupAction::SetExecutableOverride(
                emulator,
                path,
            )) => {
                self.emulator_readiness
                    .emulator_setup_overrides
                    .set_executable(emulator, Some(path));
                #[cfg(target_os = "linux")]
                if emulator == crate::emulator_setup_overrides::OverridableEmulator::ShadPs4 {
                    self.emulator_readiness.shadps4.lock().unwrap().invalidate();
                    context.request_repaint();
                    return;
                }
                self.start_doctor_scan(context.clone());
            }
            Some(emulator_setup_page::EmulatorSetupAction::SetConfigurationFolderOverride(
                emulator,
                path,
            )) => {
                self.emulator_readiness
                    .emulator_setup_overrides
                    .set_configuration_folder(emulator, Some(path));
                self.start_doctor_scan(context.clone());
            }
            Some(emulator_setup_page::EmulatorSetupAction::ResetExecutableOverride(emulator)) => {
                self.emulator_readiness
                    .emulator_setup_overrides
                    .set_executable(emulator, None);
                #[cfg(target_os = "linux")]
                if emulator == crate::emulator_setup_overrides::OverridableEmulator::ShadPs4 {
                    self.emulator_readiness.shadps4.lock().unwrap().invalidate();
                    context.request_repaint();
                    return;
                }
                self.start_doctor_scan(context.clone());
            }
            Some(emulator_setup_page::EmulatorSetupAction::ResetConfigurationFolderOverride(
                emulator,
            )) => {
                self.emulator_readiness
                    .emulator_setup_overrides
                    .set_configuration_folder(emulator, None);
                self.start_doctor_scan(context.clone());
            }
            None => {}
        }
    }

    /// The dedicated "Emulator Setup" destination: the same read-only Doctor
    /// readiness check the Diagnostics tab runs, presented as its own
    /// clearly-named page so emulator setup is discoverable without entering
    /// "Problems & Repair". Per-emulator rows appear in the scan's
    /// "Emulators" / "Emulator profiles" categories once the check has run.
    /// `emulator_setup_page::show` also renders a small "Frontends" section
    /// (currently just ES-DE) below the emulator candidates - frontends are
    /// not emulators, so they are kept out of the `LAUNCH_COMPATIBILITY`
    /// candidate grid and shown separately instead. See "ES-DE INTEGRATION
    /// VISIBILITY + SETUP FIX V1".
    pub(crate) fn show_emulator_setup_page(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        self.emulator_readiness
            .emulator_setup_overrides
            .refresh_shadps4_from_disk();
        // One-shot navigation hint: `take()` here means the first frame
        // after a repair-action navigation may scroll the relevant card
        // into view, and every later frame (and any manual scroll) is left
        // alone. Sidebar/Home navigation never sets this, so it is `None`.
        let (focus_retroarch, focus_emulator) =
            match self.emulator_readiness.emulator_setup_focus.take() {
                Some(EmulatorSetupFocus::RetroArch) => (true, None),
                Some(EmulatorSetupFocus::Emulator(name)) => (false, Some(name)),
                None => (false, None),
            };
        // RetroArch has a dedicated, cached discovery lane because its
        // profile/environment scan is also used by Cheats & Mods. Starting
        // it here makes Emulator Setup truthful on first use without moving
        // filesystem work into the pure Doctor runner.
        if matches!(
            self.emulator_readiness.retroarch_profiles,
            RetroArchProfilesState::NotScanned
        ) {
            self.start_retroarch_profile_scan(context.clone());
        }
        widgets::workflow_header(
            ui,
            "Emulators",
            "Check which emulators are installed and ready.",
        );
        let setup_action = emulator_setup_page::show(
            ui,
            &mut self.emulator_readiness.emulator_setup_page,
            self.doctor_repair
                .doctor_scan
                .displayed()
                .map(|outcome| outcome.scan.findings.as_slice()),
            self.doctor_repair.doctor_scan.is_running(),
            match &self.emulator_readiness.retroarch_profiles {
                RetroArchProfilesState::NotScanned => {
                    emulator_setup_page::RetroArchSetupStatus::NotChecked
                }
                RetroArchProfilesState::Scanning { .. } => {
                    emulator_setup_page::RetroArchSetupStatus::Checking
                }
                RetroArchProfilesState::Error(_) => {
                    emulator_setup_page::RetroArchSetupStatus::Blocked
                }
                RetroArchProfilesState::Ready(discovery) => {
                    if discovery.profiles.iter().any(|profile| profile.eligible) {
                        emulator_setup_page::RetroArchSetupStatus::Ready
                    } else {
                        emulator_setup_page::RetroArchSetupStatus::NeedsSetup
                    }
                }
            },
            focus_emulator.as_deref(),
            &self.emulator_readiness.emulator_setup_overrides,
        );
        // Any setup action may have changed what the resolver would see
        // (an override, a re-check); the read-only view re-resolves.
        let setup_changed = setup_action.is_some();
        self.handle_emulator_setup_action(setup_action, context);
        if setup_changed {
            self.invalidate_duckstation_resolution();
            self.invalidate_ppsspp_resolution();
        }
        // Below the main controls on purpose: neither resolver panel may push
        // the page's primary actions further down.
        self.show_duckstation_profile_resolution(ui, context);
        self.show_ppsspp_profile_resolution(ui, context);
        ui.add_space(theme::SECTION_GAP);
        // The managed-emulator download catalogue is always shown: a beginner
        // must be able to see whether an emulator is installed, available to
        // download automatically, or manual-install-only without first running
        // Full diagnostics. The section runs its own bounded, read-only on-disk
        // discovery (`EmulatorDownloadPageState::poll` -> `refresh`); it never
        // depends on `doctor_scan`, and the readiness checks above (and Doctor)
        // remain authoritative for whether Play is available.
        if let Some(action) = self.emulator_download_page.show_bootstrap(ui) {
            self.emulator_download_page.handle(action, context.clone());
        }
        ui.add_space(theme::SECTION_GAP);
        self.show_retroarch_core_folder_card(ui, context, focus_retroarch);
        ui.add_space(theme::SECTION_GAP);
        #[cfg(any())]
        fn show_emulator_setup_summary(
            ui: &mut egui::Ui,
            outcome: Option<&DoctorScanOutcome>,
            checking: bool,
            retroarch_profiles: &RetroArchProfilesState,
            focused_emulator: Option<&str>,
        ) -> bool {
            let mut check_emulators = false;
            widgets::card(ui, |ui| {
                ui.heading("Emulator readiness");
                let Some(outcome) = outcome else {
                    if checking {
                        widgets::empty_state(
                            ui,
                            "Checking emulators…",
                            "EmuWiz is checking the supported emulators on this computer.",
                            None,
                        );
                    } else if widgets::empty_state(
                        ui,
                        "Emulators have not been checked",
                        "Check the supported emulators on this computer before trying to play.",
                        Some("Check emulators"),
                    ) {
                        check_emulators = true;
                    }
                    return;
                };
                let emulators = [
                    "Dolphin",
                    "PCSX2",
                    "PPSSPP",
                    "RPCS3",
                    "xemu",
                    "Xenia",
                    "DuckStation",
                    "RetroArch",
                    "ScummVM",
                    "shadPS4",
                ];
                let mut not_found = Vec::new();
                for name in emulators {
                    let matching = outcome.scan.findings.iter().find(|finding| {
                        matches!(
                            finding.category,
                            DoctorCategory::Emulators | DoctorCategory::EmulatorProfiles
                        ) && (finding.title.contains(name)
                            || finding.explanation.contains(name)
                            || finding.evidence.iter().any(|line| line.contains(name)))
                    });
                    let installation = outcome.scan.findings.iter().find(|finding| {
                        finding.category == DoctorCategory::EmulatorProfiles
                            && finding.title == format!("{name} installation found")
                    });
                    if matching.is_none() && name != "RetroArch" {
                        not_found.push(name);
                        continue;
                    }
                    let focused = focused_emulator == Some(name);
                    if focused {
                        ui.scroll_to_cursor(Some(egui::Align::Center));
                    }
                    ui.group(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(name).strong());
                            match matching {
                                // Integration (Merge Rule 3): a discovered shadPS4
                                // is explicitly "Needs setup" - PS4 launch support
                                // is not claimed, so it never reads "Ready" or the
                                // generic "Setup incomplete".
                                Some(finding)
                                    if name == "shadPS4"
                                        && finding.title.contains("installation found") =>
                                {
                                    widgets::status_badge(
                                        ui,
                                        "Needs setup",
                                        widgets::StatusTone::Pending,
                                    );
                                }
                                Some(finding)
                                    if finding.title.contains("ready to launch")
                                        && finding.severity == DoctorSeverity::Info =>
                                {
                                    widgets::status_badge(
                                        ui,
                                        "Ready",
                                        widgets::StatusTone::Success,
                                    );
                                }
                                Some(_) => {
                                    widgets::status_badge(
                                        ui,
                                        "Setup incomplete",
                                        widgets::StatusTone::Warning,
                                    );
                                }
                                None => {
                                    let (label, tone) =
                                        retroarch_integration_presentation(retroarch_profiles);
                                    widgets::status_badge(ui, label, tone);
                                }
                            }
                        });
                        match matching {
                            Some(finding)
                                if name == "shadPS4"
                                    && finding.title.contains("installation found") =>
                            {
                                // Plain explanation inline; the raw discovery
                                // path stays under Technical details below.
                                ui.label(
                                    "shadPS4 is installed, but PS4 game identity and launch \
                                     support are not enabled yet.",
                                );
                            }
                            Some(finding)
                                if finding.title.contains("ready to launch")
                                    && finding.severity == DoctorSeverity::Info =>
                            {
                                ui.label("Ready to launch games.");
                            }
                            Some(finding) => {
                                let next_step = if finding.severity == DoctorSeverity::Info {
                                    "EmuWiz found this emulator, but setup is not complete."
                                } else {
                                    "Open Full diagnostics for the recommended next step."
                                };
                                // Integration: 1c825e7's flattened body wins here.
                                // The PS4 branch's discovered-but-unconfigured
                                // shadPS4 state is preserved by the dedicated
                                // `Some(finding) if name == "shadPS4"` arm above
                                // (rewoven into this structure); a shadPS4 that is
                                // *not* discovered falls to the beginner-consistent
                                // "Other supported emulators" list like every other
                                // undetected emulator (Merge Rule 3).
                                ui.label(next_step);
                            }
                            None => match retroarch_profiles {
                                RetroArchProfilesState::Scanning { .. } => {
                                    ui.label("Checking RetroArch setup…");
                                }
                                RetroArchProfilesState::Ready(_) => {
                                    ui.label("See the RetroArch card below for current setup.");
                                }
                                _ => {
                                    ui.label("Try the RetroArch check below.");
                                }
                            },
                        }
                        if let Some(finding) = matching {
                            widgets::technical_details(ui, ("emulator-finding", name), |ui| {
                                ui.label(&finding.explanation);
                                if let Some(evidence) = installation {
                                    ui.label(egui::RichText::new("Installation evidence").strong());
                                    for line in &evidence.evidence {
                                        ui.label(line);
                                    }
                                }
                            });
                        } else if let Some(evidence) = installation {
                            widgets::technical_details(ui, ("emulator-installation", name), |ui| {
                                for line in &evidence.evidence {
                                    ui.label(line);
                                }
                            });
                        }
                    });
                }
                let focus_is_not_found =
                    focused_emulator.is_some_and(|focused| not_found.contains(&focused));
                if !not_found.is_empty() {
                    egui::CollapsingHeader::new("Other supported emulators")
                        .id_salt("emulator-setup-not-found")
                        .default_open(focus_is_not_found)
                        .show(ui, |ui| {
                            for name in not_found {
                                if focused_emulator == Some(name) {
                                    ui.scroll_to_cursor(Some(egui::Align::Center));
                                }
                                ui.horizontal(|ui| {
                                    ui.label(name);
                                    widgets::status_badge(
                                        ui,
                                        "Not found",
                                        widgets::StatusTone::Pending,
                                    );
                                });
                            }
                        });
                }
            });
            check_emulators
        }
        egui::CollapsingHeader::new("Full diagnostics")
            .id_salt("emulator-setup-full-diagnostics")
            .default_open(false)
            .show(ui, |ui| self.show_doctor_page_body(ui, context));
    }

    /// The normal-user RetroArch layer for Emulator Setup: one badge, one
    /// sentence, the three repair actions, and a Technical details
    /// expander. Every readiness word comes from
    /// [`retroarch_core_setup::core_folder_readiness`], a pure projection
    /// of the shared discovery state - this method never re-derives core
    /// availability or launch readiness. "Check again" and the post-pick
    /// / post-reset refresh all go through the one existing
    /// [`Self::start_retroarch_profile_scan`] lane.
    ///
    /// `focus_retroarch` is the one-shot navigation hint from
    /// `show_emulator_setup_page`: when `true` (a repair-action arrival)
    /// this card is scrolled into view once, aligned to the top. It never
    /// expands Technical details and never re-scrolls on later frames.
    pub(crate) fn show_retroarch_core_folder_card(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        focus_retroarch: bool,
    ) {
        use retroarch_core_setup::{
            CoreFolderMode, CoreFolderReadinessKind, CoreFolderScan, core_folder_readiness,
        };

        let mode = CoreFolderMode::from_override(
            self.emulator_readiness
                .retroarch_core_directory_override
                .as_deref(),
        );
        let scanning = matches!(
            self.emulator_readiness.retroarch_profiles,
            RetroArchProfilesState::Scanning { .. }
        );
        let readiness = {
            let scan = match &self.emulator_readiness.retroarch_profiles {
                RetroArchProfilesState::NotScanned => CoreFolderScan::NotScanned,
                RetroArchProfilesState::Scanning { .. } => CoreFolderScan::Scanning,
                RetroArchProfilesState::Error(message) => CoreFolderScan::Failed(message.as_str()),
                RetroArchProfilesState::Ready(discovery) => CoreFolderScan::Ready(discovery),
            };
            core_folder_readiness(scan, &mode)
        };
        let rejected_pick = self
            .emulator_readiness
            .retroarch_core_folder_rejected_pick
            .clone();

        enum CoreFolderAction {
            Rescan,
            ChooseFolder(PathBuf),
            Reset,
        }
        let mut pending: Option<CoreFolderAction> = None;

        // Record the card's vertical span so a one-shot focus arrival can
        // scroll exactly this card into view without wrapping (and
        // re-indenting) the card body.
        let card_top = ui.cursor().top();
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("RetroArch");
                widgets::status_badge(ui, readiness.badge_label, readiness.badge_tone);
            });
            if let Some(headline) = readiness.headline {
                ui.label(egui::RichText::new(headline).strong());
            }
            ui.label(readiness.sentence.as_str());
            if scanning {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking RetroArch support files…");
                });
            }
            ui.add_space(theme::SECTION_GAP);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !scanning,
                        egui::Button::new(if readiness.kind == CoreFolderReadinessKind::Error {
                            "Try again"
                        } else {
                            "Check again"
                        }),
                    )
                    .clicked()
                {
                    pending = Some(CoreFolderAction::Rescan);
                }
                if ui
                    .add_enabled(
                        !scanning,
                        egui::Button::new(if readiness.kind == CoreFolderReadinessKind::Error {
                            "Choose folder"
                        } else {
                            "Choose game-support folder"
                        }),
                    )
                    .clicked()
                    && let Some(folder) = rfd::FileDialog::new().pick_folder()
                {
                    pending = Some(CoreFolderAction::ChooseFolder(folder));
                }
                if mode.is_custom()
                    && ui
                        .add_enabled(!scanning, egui::Button::new("Reset to automatic"))
                        .clicked()
                {
                    pending = Some(CoreFolderAction::Reset);
                }
            });
            if rejected_pick.is_some() {
                widgets::banner(
                    ui,
                    "Folder not usable",
                    "This folder could not be used for RetroArch game support. Choose another \
                     folder or reset to automatic detection.",
                    widgets::StatusTone::Blocked,
                );
            }
            egui::CollapsingHeader::new("Technical details")
                .id_salt("retroarch-core-folder-technical")
                .default_open(false)
                .show(ui, |ui| {
                    show_retroarch_core_folder_technical(
                        ui,
                        &self.emulator_readiness.retroarch_profiles,
                        &mode,
                        rejected_pick.as_deref(),
                    );
                });
        });

        // First frame after a repair-action arrival: bring the card into
        // view, aligned near the top. `focus_retroarch` came from a
        // one-shot `take()`, so this fires once and never fights a manual
        // scroll afterward. Technical details is untouched - it stays
        // collapsed.
        if focus_retroarch {
            let card_rect = egui::Rect::from_min_max(
                egui::pos2(ui.min_rect().left(), card_top),
                egui::pos2(ui.min_rect().right(), ui.cursor().top()),
            );
            ui.scroll_to_rect(card_rect, Some(egui::Align::TOP));
        }

        match pending {
            Some(CoreFolderAction::Rescan) => {
                self.emulator_readiness.retroarch_core_folder_rejected_pick = None;
                if !scanning {
                    self.start_retroarch_profile_scan(context.clone());
                }
            }
            Some(CoreFolderAction::ChooseFolder(folder)) => {
                self.apply_picked_retroarch_core_folder(folder, context.clone());
            }
            Some(CoreFolderAction::Reset) => {
                self.emulator_readiness.retroarch_core_folder_rejected_pick = None;
                self.clear_retroarch_core_directory_override();
                self.start_retroarch_profile_scan(context.clone());
            }
            None => {}
        }
    }

    /// Gathers exactly what [`launch_readiness_page::show_launch_readiness_panel`]
    /// needs from state this app already has - never adds any new
    /// app-level state. RetroArch discovery is optional here: a standalone
    /// adapter can make a complete plan while the independent RetroArch lane
    /// is still pending.
    ///
    /// The `GameIdentityReport` comes from the selected evidence worker and
    /// is only trusted when its exact path matches the focused archive. This
    /// keeps launch planning on the core identity bridge while making it
    /// available without opening Cheats & Mods first.
    pub(crate) fn build_launch_readiness_input(
        &self,
        live: Option<&LoadedData>,
    ) -> launch_readiness_page::LaunchReadinessInput {
        use launch_readiness_page::LaunchReadinessInput;

        if !matches!(
            self.selected_evidence_ui.selected_evidence,
            selected_evidence_page::SelectedEvidenceState::Ready { .. }
        ) {
            return LaunchReadinessInput::EvidenceNotLoaded;
        }
        let retroarch_scanned = matches!(
            self.emulator_readiness.retroarch_profiles,
            RetroArchProfilesState::Ready(_)
        );
        let empty_retroarch =
            archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
                format_version: 2,
                profiles: Vec::new(),
                diagnostics: Vec::new(),
            };
        let retroarch_environment = match &self.emulator_readiness.retroarch_profiles {
            RetroArchProfilesState::Ready(discovery) => &discovery.environment,
            RetroArchProfilesState::NotScanned
            | RetroArchProfilesState::Scanning { .. }
            | RetroArchProfilesState::Error(_) => &empty_retroarch,
        };

        let focused = self.archive_context.focused.as_deref();
        let selected_game_identity_report =
            focused.and_then(
                |focused| match &self.selected_evidence_ui.selected_evidence {
                    selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                        if report.path == focused =>
                    {
                        Some(&report.game_identity_report)
                    }
                    _ => None,
                },
            );
        // Keep older in-process test/compatibility fixtures usable when they
        // have not yet been given the new selected-evidence report. A real
        // selected game always takes the evidence-worker report above, so
        // opening Cheats & Mods is no longer required for launch planning.
        let game_identity_report = selected_game_identity_report
            .filter(|report| {
                !matches!(
                    archivefs_core::launch::canonical_identity_from_game_report(report).0,
                    archivefs_core::launch::CanonicalIdentityStatus::Unknown
                )
            })
            .or_else(|| {
                focused
                    .and_then(|focused| {
                        self.cheat_workflow
                            .as_ref()
                            .filter(|workflow| workflow.archive_path == focused)
                    })
                    .and_then(ready_game_identity)
            });

        let (identity_status, verified_facts) = match game_identity_report {
            Some(report) => archivefs_core::launch::canonical_identity_from_game_report(report),
            None => (
                archivefs_core::launch::CanonicalIdentityStatus::Unknown,
                Vec::new(),
            ),
        };
        // PCSX2's `Pcsx2LaunchRequest` needs a genuinely verified PS2
        // serial specifically - never `plan.game_key` alone, which for PS2
        // may instead be a verified executable CRC (see
        // `evidence_bridge::resolved_identity_for_platform`'s `Ps2Serial`/
        // `Pcsx2ExecutableCrc` handling) when no serial was verified.
        let verified_ps2_serial = verified_facts.iter().find_map(|fact| match fact {
            archivefs_core::launch::VerifiedIdentityFact::Ps2Serial(serial) => Some(serial.clone()),
            _ => None,
        });

        // Arcade directory archives are logical MAME sets, not ordinary
        // files, so their identity comes from the persisted, SHA-bound MAME
        // join. This is the narrow projection bridge that keeps raw members
        // (for example ru_04b.img) out of launch identity.
        let focused_record = live.and_then(|data| {
            focused.and_then(|path| {
                data.records
                    .iter()
                    .find(|record| record.mount_plan.archive.path == path)
            })
        });
        let mut identity_status = identity_status;
        let mut mame_set_resolutions = Vec::new();
        if let Some(focused) = focused
            && let Ok(database_path) = archivefs_core::default_database_path()
            && let Ok(database) = archivefs_core::Database::open_read_only(database_path)
            && let Ok(Some(evidence)) = database.mame_arcade_join_for_archive_path(
                archivefs_core::dat::mame_arcade_join::MAME_0174_SHA256,
                focused,
            )
            && let Some(resolution) =
                archivefs_core::dat::mame_arcade_join::launch_resolution_for_join(
                    &evidence, focused,
                )
        {
            identity_status = archivefs_core::launch::CanonicalIdentityStatus::Resolved(
                archivefs_core::launch::ResolvedIdentity {
                    platform_id: "Arcade".to_string(),
                    game_key: resolution.identity.game_name.clone(),
                },
            );
            mame_set_resolutions.push(resolution);
        }

        // Read-only Atari ST setup/media projection has no typed launch request.
        // Preserve stronger/conflicting identities and keep this separate from
        // canonical game identity, verification and the executable Plan lane.
        if matches!(
            identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Unknown
        ) && let Some(path) = focused
            && (path
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "st" | "stx"))
                || selected_game_identity_report.is_some_and(|r| {
                    r.platform == archivefs_core::game_identity::IdentityPlatform::AtariST
                }))
        {
            return LaunchReadinessInput::AtariSt(
                crate::emulator_setup::hatari::Controller::snapshot(
                    &self.emulator_readiness.hatari,
                    crate::emulator_setup::hatari::Selection {
                        path: path.into(),
                        source_root: focused_record
                            .map(|r| r.mount_plan.archive.identity.source_root.clone())
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or_else(|| path.parent().unwrap_or(path).into()),
                        platform: focused_record
                            .and_then(|r| r.mount_plan.archive.identity.platform.clone()),
                    },
                ),
            );
        }

        let flycast_context = self.emulator_readiness.flycast_gdi.context(focused);
        let flycast_inputs = flycast_context
            .as_ref()
            .and_then(|c| c.inputs(&identity_status));
        if let Some((identity, _)) = &flycast_inputs {
            identity_status = identity.clone();
        }
        let cemu_context = self.emulator_readiness.cemu.launch_context(focused);
        let cemu_inputs = cemu_context
            .as_ref()
            .filter(|_| {
                selected_game_identity_report.is_none_or(|r| {
                    matches!(
                        r.platform,
                        archivefs_core::game_identity::IdentityPlatform::WiiU
                            | archivefs_core::game_identity::IdentityPlatform::Other
                    )
                }) && focused_record.is_none_or(|r| {
                    r.mount_plan
                        .archive
                        .identity
                        .platform
                        .as_deref()
                        .is_none_or(|p| {
                            archivefs_core::canonical_platform_for_alias(p) == Some("WiiU")
                        })
                })
            })
            .and_then(|context| context.inputs(&identity_status));
        if let Some((identity, _)) = &cemu_inputs {
            identity_status = identity.clone();
        }

        match identity_status {
            archivefs_core::launch::CanonicalIdentityStatus::Unknown => {
                // The worker's own report already says which track of a CUE
                // disc could not be resolved; carry it instead of hiding it.
                if let Some(detail) = selected_game_identity_report
                    .and_then(|report| disc_files_diagnostic(report, focused))
                {
                    return LaunchReadinessInput::DiscFilesUnresolved(detail);
                }
                return LaunchReadinessInput::IdentityUnknown;
            }
            archivefs_core::launch::CanonicalIdentityStatus::Conflicting => {
                return LaunchReadinessInput::IdentityConflicting;
            }
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(_) => {}
        }

        // Archive safety: only the transient, selection-bound preparation
        // state may provide an inner member. The bridge still refuses it
        // unless the record is genuinely mounted.
        let mut content = match focused_record {
            Some(record) => {
                let member = self.resolved_archive_member_path(record);
                archivefs_core::launch::launch_content_ref_from_archive_record(
                    record,
                    member.as_deref(),
                )
            }
            // Live library data isn't loaded, so content cannot be honestly
            // classified either way - this reads exactly like an
            // unresolved bridge result, never a fabricated "found" or a
            // misleading reuse of the evidence-not-loaded message (evidence
            // genuinely was loaded; only the live library data was not).
            // A ScummVM game folder is catalogued by the scan's detector and
            // has no live archive record: its own path is the runnable
            // content, exactly as for any other loose, directly launched item.
            None if focused.is_some_and(|path| self.persisted_scummvm_folder(path).is_some()) => {
                archivefs_core::launch::LaunchContentRef {
                    kind: None,
                    container: Some(archivefs_core::launch::LaunchContainerKind::PlainFile),
                    resolved_path: focused.map(Path::to_path_buf),
                    requires_mount: false,
                    provenance: "extracted ScummVM game folder from the persisted catalogue"
                        .to_string(),
                }
            }
            // Blue does not load live library data. The page's persisted
            // row for this exact path is plain loose content, which core's
            // bridge would resolve to its own path; a vanished or replaced
            // file is still refused by launch preflight.
            None if let Some(content) =
                focused.and_then(|path| self.catalogued_loose_content_ref(path)) =>
            {
                content
            }
            None => archivefs_core::launch::LaunchContentRef {
                kind: None,
                container: None,
                resolved_path: None,
                requires_mount: false,
                provenance: "live library data is not loaded; content readiness cannot be \
                             checked"
                    .to_string(),
            },
        };
        if !mame_set_resolutions.is_empty() {
            content.resolved_path = focused.map(Path::to_path_buf);
            content.container = Some(archivefs_core::launch::LaunchContainerKind::PlainFile);
            content.requires_mount = false;
            content.provenance =
                "verified MAME logical-set directory from the persisted DAT join".into();
        }

        if let Some((_, selected_content)) = flycast_inputs {
            content = selected_content;
        }
        if let Some((_, selected_content)) = cemu_inputs {
            content = selected_content;
        }

        // WHDLoad is an additive, content-bound launch seam.  The package
        // and slave are accepted only after the existing bounded LHA
        // inspector found exactly one valid slave; ADF/HDF and arbitrary
        // paths never enter this branch.
        let mut amiga_whdload_profiles = Vec::new();
        let mut amiga_whdload_requests = Vec::new();
        let mut verified_whdload_content = None;
        if let Some(record) = focused_record
            && matches!(
                identity_status,
                archivefs_core::launch::CanonicalIdentityStatus::Resolved(_)
            )
            && record
                .mount_plan
                .archive
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(extension.to_ascii_lowercase().as_str(), "lha" | "lzh")
                })
        {
            let cancel = AtomicBool::new(false);
            if let Ok(discovery) =
                archivefs_core::amiga_whdload_archive::discover_whdload_slaves_in_archive(
                    &record.mount_plan.archive.path,
                    &cancel,
                )
                && discovery.candidates.len() == 1
            {
                let candidate = &discovery.candidates[0];
                let mut artifact = candidate.artifact.clone();
                artifact.path = record.mount_plan.archive.path.clone();
                let selected_name = Path::new(&candidate.member_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned);
                if let Some(selected_name) = selected_name {
                    artifact.name = selected_name.clone();
                    let target = archivefs_core::launch::VerifiedWHDLoadTarget {
                        package_path: record.mount_plan.archive.path.clone(),
                        format: archivefs_core::launch::WhdloadPackageFormat::Lha,
                    };
                    let selected_slave = archivefs_core::launch::SelectedWHDLoadSlave {
                        artifact_path: record.mount_plan.archive.path.clone(),
                        name: selected_name.clone(),
                        parsed: artifact.parsed.clone(),
                    };
                    verified_whdload_content =
                        Some(archivefs_core::launch::VerifiedWHDLoadContent {
                            target: target.clone(),
                            selected_slave: selected_slave.clone(),
                        });
                    if let Some(roots) =
                        archivefs_core::patch_manager::AmigaProfileDiscoveryRoots::from_environment(
                        )
                    {
                        let discovery =
                            archivefs_core::patch_manager::discover_amiga_profiles(&roots);
                        let identity = match &identity_status {
                            archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) => {
                                identity
                            }
                            _ => unreachable!(),
                        };
                        for profile in &discovery.profiles {
                            let emulator = profile.emulator;
                            let candidate_count = discovery
                                .profiles
                                .iter()
                                .filter(|other| other.emulator == emulator)
                                .count();
                            let inspection =
                                archivefs_core::patch_manager::inspect_amiga_whdload_game(
                                    profile,
                                    &archivefs_core::patch_manager::AmigaGameRequest {
                                        verified_amiga_identity: Some(identity.game_key.clone()),
                                        bare_slaves: vec![artifact.clone()],
                                        ..Default::default()
                                    },
                                );
                            let executable = match profile.executable_candidates.as_slice() {
                                [executable] => Some(executable.path.clone()),
                                _ => None,
                            };
                            let request = archivefs_core::launch::WHDLoadLaunchInput {
                                identity: identity_status.clone(),
                                target: Some(target.clone()),
                                slave: Some(selected_slave.clone()),
                                profile: archivefs_core::launch::WHDLoadProfileInput {
                                    emulator,
                                    profile_id: profile.profile_id.clone(),
                                    executable,
                                    configuration: profile.global_config_path.clone(),
                                    candidate_count,
                                    eligible: profile.eligible,
                                    kickstart: inspection.health.kickstart.state,
                                    verified_identity: identity.game_key.clone(),
                                },
                            };
                            let adapter_id = match emulator {
                                archivefs_core::patch_manager::AmigaEmulatorKind::Amiberry => {
                                    "amiberry"
                                }
                                archivefs_core::patch_manager::AmigaEmulatorKind::FsUae => "fsuae",
                            };
                            amiga_whdload_profiles.push(
                                archivefs_core::launch::StandaloneProfileInput {
                                    adapter_id,
                                    profile_id: profile.profile_id.clone(),
                                    profile_path: profile.global_config_path.clone(),
                                    eligible: profile.eligible,
                                    firmware:
                                        archivefs_core::launch::FirmwareReadiness::NotRequired,
                                },
                            );
                            amiga_whdload_requests.push((
                                adapter_id.to_string(),
                                profile.profile_id.clone(),
                                request,
                            ));
                        }
                    }
                }
            }
        }
        let amiga_whdload_context = (!amiga_whdload_requests.is_empty()).then_some(
            launch_readiness_page::AmigaWHDLoadLaunchContext {
                requests: amiga_whdload_requests,
            },
        );
        if let Some(binding) = verified_whdload_content {
            content.kind = Some(archivefs_core::launch::LaunchContentKind::Whdload(binding));
        }

        // Real, already-discovered Dolphin profiles only - never a
        // fabricated `StandaloneProfileInput`. `DolphinLocalProfilesState`
        // starting `NotScanned`/`Scanning`/`Error` simply contributes no
        // Dolphin candidate yet (the same honest, fail-closed shape as an
        // empty slice), rather than blocking the whole panel the way a
        // missing RetroArch scan does - Dolphin readiness is additive here.
        let dolphin_context = match &self.emulator_readiness.dolphin_local_profiles {
            DolphinLocalProfilesState::Ready(ready) => {
                Some(launch_readiness_page::DolphinLaunchContext {
                    discovery: ready.discovery.clone(),
                    roots: ready.roots.clone(),
                })
            }
            DolphinLocalProfilesState::NotScanned
            | DolphinLocalProfilesState::Scanning { .. }
            | DolphinLocalProfilesState::Error(_) => None,
        };
        let dolphin_standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            dolphin_context
                .as_ref()
                .map(|context| {
                    context
                        .discovery
                        .profiles
                        .iter()
                        .map(|profile| archivefs_core::launch::StandaloneProfileInput {
                            adapter_id: "dolphin",
                            profile_id: profile.profile_id.clone(),
                            profile_path: Some(profile.configuration_root.clone()),
                            eligible: profile.eligible,
                            firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                        })
                        .collect()
                })
                .unwrap_or_default();

        // Real, already-discovered PCSX2 profiles and already-resolved PS2
        // firmware evidence only - never fabricated. Both
        // `Pcsx2LaunchProfilesState` and `Pcsx2FirmwareEvidenceState`
        // starting anything other than `Ready` simply contribute no PCSX2
        // candidate yet, the same additive-not-blocking shape as `dolphin`
        // above - a missing/incomplete DAT scan never widens readiness, it
        // only means no PCSX2 candidate is offered until it completes.
        let pcsx2_firmware_evidence: &[archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord] =
            match &self.emulator_readiness.pcsx2_firmware_evidence {
                Pcsx2FirmwareEvidenceState::Ready(evidence) => evidence,
                Pcsx2FirmwareEvidenceState::NotLoaded
                | Pcsx2FirmwareEvidenceState::Loading { .. }
                | Pcsx2FirmwareEvidenceState::Error(_) => &[],
            };
        let pcsx2_context = match &self.emulator_readiness.pcsx2_launch_profiles {
            Pcsx2LaunchProfilesState::Ready(ready) => {
                Some(launch_readiness_page::Pcsx2LaunchContext {
                    discovery: ready.discovery.clone(),
                    roots: ready.roots.clone(),
                    firmware_evidence: pcsx2_firmware_evidence.to_vec(),
                    verified_ps2_serial: verified_ps2_serial.clone(),
                })
            }
            Pcsx2LaunchProfilesState::NotScanned
            | Pcsx2LaunchProfilesState::Scanning { .. }
            | Pcsx2LaunchProfilesState::Error(_) => None,
        };
        // A profile's BIOS readiness never depends on the currently
        // selected game - `resolve_pcsx2_bios`/`inspect_pcsx2_bios` only
        // ever read the profile's own global config and `bios/` directory
        // - so an all-`None` `Pcsx2GameRequest` here is a genuine,
        // honest inspection, not a placeholder.
        let pcsx2_standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            pcsx2_context
                .as_ref()
                .map(|context| {
                    context
                        .discovery
                        .profiles
                        .iter()
                        .map(|profile| {
                            let inspection =
                                archivefs_core::patch_manager::inspect_pcsx2_game_with_firmware_evidence(
                                    profile,
                                    &archivefs_core::patch_manager::Pcsx2GameRequest {
                                        verified_ps2_serial: None,
                                        verified_executable_crc: None,
                                        emulator_serial: None,
                                    },
                                    &context.firmware_evidence,
                                );
                            archivefs_core::launch::StandaloneProfileInput {
                                adapter_id: "pcsx2",
                                profile_id: profile.profile_id.clone(),
                                profile_path: Some(profile.configuration_path.clone()),
                                eligible: profile.eligible,
                                firmware: archivefs_core::launch::pcsx2_firmware_readiness(
                                    inspection.inspection.bios.verification,
                                ),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

        // Flycast is additive to the existing readiness view. Its adapter
        // already owns configuration, executable and firmware inspection;
        // this page only projects those read-only results into the shared
        // launch planner. A binding is resolved here solely so a missing,
        // unsafe or ambiguous executable is shown as blocked rather than
        // advertised as launch-ready. Core preflight repeats the same check.
        let flycast_standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            match &self.emulator_readiness.flycast_profiles {
                FlycastProfilesState::Ready(ready) => ready
                    .discovery
                    .profiles
                    .iter()
                    .map(|profile| {
                        let verified_product_code = verified_facts.iter().find_map(|fact| match fact {
                            archivefs_core::launch::VerifiedIdentityFact::DreamcastProductCode(code) => {
                                Some(code.clone())
                            }
                            _ => None,
                        });
                        let inspection = archivefs_core::patch_manager::inspect_flycast_game(
                            profile,
                            &archivefs_core::patch_manager::FlycastGameRequest {
                                canonical_platform: Some("Dreamcast".to_string()),
                                flycast_platform: Some(
                                    archivefs_core::patch_manager::FlycastPlatform::Dreamcast,
                                ),
                                verified_dreamcast_product_code: verified_product_code,
                                ..Default::default()
                            },
                        );
                        archivefs_core::launch::StandaloneProfileInput {
                            adapter_id: "flycast",
                            profile_id: profile.profile_id.clone(),
                            profile_path: Some(profile.configuration_path.clone()),
                            eligible: profile.eligible
                                && archivefs_core::patch_manager::resolve_flycast_native_launch_binding(
                                    profile,
                                )
                                .is_ok(),
                            firmware: archivefs_core::launch::flycast_firmware_readiness(
                                inspection.health.system.dreamcast_bios,
                            ),
                        }
                    })
                    .collect(),
                FlycastProfilesState::NotScanned
                | FlycastProfilesState::Scanning { .. }
                | FlycastProfilesState::Error(_) => Vec::new(),
            };

        let mut standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            dolphin_standalone_profiles
                .into_iter()
                .chain(pcsx2_standalone_profiles)
                .chain(flycast_standalone_profiles)
                .chain(amiga_whdload_profiles)
                .collect();

        // MAME has no profile directory; its lifecycle binding is the exact
        // executable selected through Emulator Setup, while its game
        // eligibility comes only from the persisted MAME join above.
        if let Some(executable) = self
            .emulator_readiness
            .emulator_setup_overrides
            .executable(crate::emulator_setup_overrides::OverridableEmulator::Mame)
            && !mame_set_resolutions.is_empty()
        {
            standalone_profiles.push(archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "mame",
                profile_id: format!("mame:{}", executable.display()),
                profile_path: Some(executable.to_path_buf()),
                eligible: true,
                firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
            });
        }

        // Fuse is a narrow, read-only ZX Spectrum adapter.  Discovery is
        // additive: no executable means no Fuse candidate, and the generic
        // planner still requires an independently resolved ZX Spectrum
        // identity plus a supported direct content path.
        let fuse_roots =
            archivefs_core::patch_manager::FuseProfileDiscoveryRoots::from_environment();
        let fuse_discovery = archivefs_core::patch_manager::discover_fuse_profiles(&fuse_roots);
        standalone_profiles.extend(fuse_discovery.profiles.iter().map(|profile| {
            archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "fuse",
                profile_id: profile.profile_id.clone(),
                profile_path: Some(profile.executable.clone()),
                eligible: profile.eligible,
                firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
            }
        }));

        // Tsugaru is an additive FM Towns candidate.  Its discovery remains
        // read-only and only becomes launchable when an explicit ROM
        // directory is configured; the shared planner still requires a
        // separately resolved FM Towns identity.
        let tsugaru_roots =
            archivefs_core::patch_manager::TsugaruProfileDiscoveryRoots::from_environment();
        let tsugaru_discovery =
            archivefs_core::patch_manager::discover_tsugaru_profiles(&tsugaru_roots);
        standalone_profiles.extend(tsugaru_discovery.profiles.iter().map(|profile| {
            archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "tsugaru",
                profile_id: profile.profile_id.clone(),
                profile_path: Some(profile.executable.path.clone()),
                eligible: profile.eligible,
                firmware: match profile.firmware {
                    archivefs_core::patch_manager::TsugaruFirmwareState::Verified => {
                        archivefs_core::launch::FirmwareReadiness::Verified
                    }
                    archivefs_core::patch_manager::TsugaruFirmwareState::PresentUnverified => {
                        archivefs_core::launch::FirmwareReadiness::PresentUnverified
                    }
                    archivefs_core::patch_manager::TsugaruFirmwareState::Missing => {
                        archivefs_core::launch::FirmwareReadiness::Missing
                    }
                    archivefs_core::patch_manager::TsugaruFirmwareState::Unknown => {
                        archivefs_core::launch::FirmwareReadiness::Unknown
                    }
                },
            }
        }));

        let xroar_roots =
            archivefs_core::patch_manager::XRoarProfileDiscoveryRoots::from_environment();
        let xroar_discovery = archivefs_core::patch_manager::discover_xroar_profiles(&xroar_roots);
        standalone_profiles.extend(xroar_discovery.profiles.iter().map(|profile| {
            archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "xroar",
                profile_id: profile.profile_id.clone(),
                profile_path: Some(profile.executable.path.clone()),
                eligible: profile.eligible,
                firmware: match profile.firmware {
                    archivefs_core::patch_manager::XRoarFirmwareState::Verified => {
                        archivefs_core::launch::FirmwareReadiness::Verified
                    }
                    archivefs_core::patch_manager::XRoarFirmwareState::PresentUnverified => {
                        archivefs_core::launch::FirmwareReadiness::PresentUnverified
                    }
                    archivefs_core::patch_manager::XRoarFirmwareState::Missing => {
                        archivefs_core::launch::FirmwareReadiness::Missing
                    }
                    archivefs_core::patch_manager::XRoarFirmwareState::Unknown => {
                        archivefs_core::launch::FirmwareReadiness::Unknown
                    }
                },
            }
        }));

        // The remaining native adapters are additive inputs to the same
        // shared planner. Discovery and inspection are kept read-only and
        // bounded by their existing adapters; this block does not rebuild a
        // command, infer identity, or turn an executable's presence into a
        // Ready result. A later launch click still invokes that adapter's
        // own preflight and execution path.
        let empty_firmware: &[archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord] = &[];

        // Already-validated EmuWiz-managed AppImages (if any) - caller-
        // confirmed explicit executables fed into the *existing* per-adapter
        // discovery. The `install.json`-backed managed slice is swept once
        // here (bounded, read-only, no shell) and each adapter's executable
        // is projected from it by exact catalogue display name. Empty on any
        // machine without a managed install, so launch readiness there is
        // byte-for-byte unchanged. This adds no app-level state - it is
        // derived here, exactly like every other discovery call in this
        // function.
        let managed_appimage_installs = discover_managed_appimage_installations();
        let managed_ppsspp_appimages: Vec<PathBuf> =
            managed_appimage_explicit_executables(&managed_appimage_installs, "PPSSPP");
        let managed_rpcs3_appimages: Vec<PathBuf> =
            managed_appimage_explicit_executables(&managed_appimage_installs, "RPCS3");
        // The user's persisted RPCS3 executable choice (Emulator Setup). It
        // decides between several discovered AppImages; it never adds a
        // guessed candidate and is refused with an explanation when stale.
        let rpcs3_selected_executable: Option<PathBuf> = self
            .emulator_readiness
            .emulator_setup_overrides
            .executable(crate::emulator_setup_overrides::OverridableEmulator::Rpcs3)
            .map(Path::to_path_buf);
        let managed_duckstation_appimages: Vec<PathBuf> =
            managed_appimage_explicit_executables(&managed_appimage_installs, "DuckStation");
        let managed_xemu_appimages: Vec<PathBuf> =
            managed_appimage_explicit_executables(&managed_appimage_installs, "xemu");
        // Launch consumes the shared resolver's answer for DuckStation: only
        // the pair it authorises is offered, and a refusal blocks DuckStation
        // outright instead of falling back to a separate discovery.
        let duckstation_selection: Option<DuckStationLaunchSelection> =
            DuckStationResolutionInputs::from_environment(
                &self.emulator_readiness.emulator_setup_overrides,
                &managed_duckstation_appimages,
            )
            .ok()
            .map(|inputs| select_for_launch(&inputs));
        if let Some(selection) = &duckstation_selection {
            let discovery = &selection.discovery;
            let request = archivefs_core::patch_manager::DuckStationGameRequest {
                verified_ps1_serial: verified_facts.iter().find_map(|fact| match fact {
                    archivefs_core::launch::VerifiedIdentityFact::Ps1Serial(serial) => {
                        Some(serial.clone())
                    }
                    _ => None,
                }),
                ..Default::default()
            };
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                let inspection =
                    archivefs_core::patch_manager::inspect_duckstation_game_with_firmware_evidence(
                        profile,
                        &request,
                        pcsx2_firmware_evidence,
                    );
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "duckstation",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::duckstation_firmware_readiness(
                        inspection.inspection.health.bios,
                    ),
                }
            }));
            if matches!(
                selection.authority,
                DuckStationLaunchAuthority::Refused { .. }
            ) {
                // A visible, ineligible DuckStation entry: Launch is blocked
                // by the resolver's decision, not "no emulator found".
                standalone_profiles.push(archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "duckstation",
                    profile_id: "duckstation:shared-resolver-refused".to_string(),
                    profile_path: None,
                    eligible: false,
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                });
            }
        }
        if let Ok(mut roots) =
            archivefs_core::patch_manager::PpssppProfileDiscoveryRoots::from_environment()
        {
            roots
                .explicit_executables
                .extend(managed_ppsspp_appimages.iter().cloned());
            let discovery = archivefs_core::patch_manager::discover_ppsspp_profiles(&roots);
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "ppsspp",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
        }
        if let Ok(mut roots) =
            archivefs_core::patch_manager::Rpcs3ProfileDiscoveryRoots::from_environment()
        {
            roots.selected_executable = rpcs3_selected_executable.clone();
            let discovery = archivefs_core::patch_manager::discover_rpcs3_profiles(&roots);
            let request = archivefs_core::patch_manager::Rpcs3GameRequest {
                verified_ps3_title_id: verified_facts.iter().find_map(|fact| match fact {
                    archivefs_core::launch::VerifiedIdentityFact::Ps3TitleId(id) => {
                        Some(id.clone())
                    }
                    _ => None,
                }),
                ..Default::default()
            };
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                let inspection =
                    archivefs_core::patch_manager::inspect_rpcs3_game(profile, &request);
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "rpcs3",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::rpcs3_firmware_readiness(
                        &inspection.health.firmware,
                    ),
                }
            }));
        }
        if let Ok(roots) =
            archivefs_core::patch_manager::XemuProfileDiscoveryRoots::from_environment()
        {
            let discovery = archivefs_core::patch_manager::discover_xemu_profiles(&roots);
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "xemu",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    // xemu's command planner performs its own MCPX/BIOS/
                    // EEPROM/HDD health validation during preflight.
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
        }
        if let Some(inspection) = self.emulator_readiness.cemu.inspection_for(focused) {
            standalone_profiles.extend(inspection.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "cemu",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.profile_path.clone()),
                    eligible: profile.command.is_some()
                        && cemu_context
                            .as_ref()
                            .is_some_and(|context| context.profile_is_bound(profile)),
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
        }
        if let Ok(roots) =
            archivefs_core::patch_manager::MelonDsProfileDiscoveryRoots::from_environment()
        {
            let discovery = archivefs_core::patch_manager::discover_melonds_profiles(&roots);
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "melonds",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::melonds_firmware_readiness(&profile.firmware),
                }
            }));
        }
        if let Ok(roots) =
            archivefs_core::patch_manager::MgbaProfileDiscoveryRoots::from_environment()
        {
            let discovery = archivefs_core::patch_manager::discover_mgba_profiles(&roots);
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "mgba",
                    profile_id: profile.profile_id.clone(),
                    profile_path: profile.config_path.clone(),
                    eligible: profile.eligible,
                    // A BIOS is optional for ordinary GBA/GB/GBC launch;
                    // mGBA's own preflight retains any configured BIOS
                    // evidence without turning absence into a blocker.
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
        }
        if let Some(home) = std::env::var_os("HOME") {
            let roots = archivefs_core::patch_manager::AzaharDiscoveryRoots {
                explicit_executable: None,
                path: std::env::var_os("PATH"),
                config_root: Some(PathBuf::from(home).join(".config/azahar")),
            };
            if let Some(executable) =
                archivefs_core::patch_manager::discover_azahar_executable(&roots)
            {
                let profile =
                    archivefs_core::patch_manager::discover_azahar_profile(&roots, executable);
                standalone_profiles.push(archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "azahar",
                    profile_id: format!("azahar:{}", profile.executable.display()),
                    profile_path: profile.config.clone(),
                    eligible: archivefs_core::launch::azahar_profile_eligible(&profile),
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                });
            }
        }
        if let XeniaProfilesState::Ready(discovery) = &self.emulator_readiness.xenia_profiles {
            let (title_id, media_id) = game_identity_report
                .map(|report| {
                    (
                        report.verified_xex_title_id().map(str::to_owned),
                        report.verified_xex_media_id().map(str::to_owned),
                    )
                })
                .unwrap_or((None, None));
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "xenia",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
            let _ = (title_id, media_id);
        }

        // The focused game's verified media set(s), from the cached library
        // snapshot, for the shared multi-disc launch decision.
        let multidisc = focused.and_then(|path| {
            self.database_state.snapshot().and_then(|snapshot| {
                let sets = crate::multidisc_launch_view::library_sets(
                    snapshot,
                    self.database_generation.0,
                );
                crate::multidisc_launch_view::MultiDiscLaunchContext::for_content(
                    &sets,
                    path,
                    &snapshot.archives,
                )
                .map(|context| context.with_disc_members(&snapshot.disc_members))
            })
        });
        let duckstation_context = duckstation_selection.map(|selection| {
            launch_readiness_page::DuckStationLaunchContext {
                multidisc: multidisc.clone(),
                discovery: selection.discovery,
                roots: selection.roots,
                firmware_evidence: pcsx2_firmware_evidence.to_vec(),
                authority: selection.authority,
                verified_ps1_serial: verified_facts.iter().find_map(|fact| match fact {
                    archivefs_core::launch::VerifiedIdentityFact::Ps1Serial(serial) => {
                        Some(serial.clone())
                    }
                    _ => None,
                }),
            }
        });
        let ppsspp_context =
            archivefs_core::patch_manager::PpssppProfileDiscoveryRoots::from_environment()
                .ok()
                .map(|mut roots| {
                    // Same validated managed-AppImage evidence as the
                    // standalone-profile projection above; the launch
                    // context re-runs discovery at preflight time, so its
                    // roots must carry the exact same explicit executable.
                    roots
                        .explicit_executables
                        .extend(managed_ppsspp_appimages.iter().cloned());
                    launch_readiness_page::PpssppLaunchContext {
                        discovery: archivefs_core::patch_manager::discover_ppsspp_profiles(&roots),
                        roots,
                        verified_psp_disc_id: verified_facts.iter().find_map(|fact| match fact {
                            archivefs_core::launch::VerifiedIdentityFact::PspDiscId(id) => {
                                Some(id.clone())
                            }
                            _ => None,
                        }),
                    }
                });
        let rpcs3_context =
            archivefs_core::patch_manager::Rpcs3ProfileDiscoveryRoots::from_environment()
                .ok()
                .map(|mut roots| {
                    // Already-validated EmuWiz-managed RPCS3 AppImage (if
                    // any) as a caller-confirmed explicit executable. Empty
                    // -> no-op, behaviour unchanged.
                    roots
                        .explicit_executables
                        .extend(managed_rpcs3_appimages.iter().cloned());
                    roots.selected_executable = rpcs3_selected_executable.clone();
                    launch_readiness_page::Rpcs3LaunchContext {
                        discovery: archivefs_core::patch_manager::discover_rpcs3_profiles(&roots),
                        roots,
                        verified_ps3_title_id: verified_facts.iter().find_map(|fact| match fact {
                            archivefs_core::launch::VerifiedIdentityFact::Ps3TitleId(id) => {
                                Some(id.clone())
                            }
                            _ => None,
                        }),
                    }
                });
        let xemu_context =
            archivefs_core::patch_manager::XemuProfileDiscoveryRoots::from_environment()
                .ok()
                .map(|mut roots| {
                    // Already-validated EmuWiz-managed xemu AppImage (if any)
                    // as a caller-confirmed explicit executable. The Xbox
                    // BIOS/MCPX/EEPROM/HDD readiness stays independent - it
                    // is validated by the xemu command planner at preflight,
                    // not here. Empty -> no-op, behaviour unchanged.
                    roots
                        .explicit_executables
                        .extend(managed_xemu_appimages.iter().cloned());
                    launch_readiness_page::XemuLaunchContext {
                        discovery: archivefs_core::patch_manager::discover_xemu_profiles(&roots),
                        roots,
                        verified_xbox_title_id: verified_facts.iter().find_map(|fact| match fact {
                            archivefs_core::launch::VerifiedIdentityFact::XboxTitleId(id) => {
                                Some(id.clone())
                            }
                            _ => None,
                        }),
                    }
                });
        let melonds_context =
            archivefs_core::patch_manager::MelonDsProfileDiscoveryRoots::from_environment()
                .ok()
                .map(|roots| launch_readiness_page::MelonDsLaunchContext {
                    discovery: archivefs_core::patch_manager::discover_melonds_profiles(&roots),
                    roots,
                    verified_game_key: verified_facts.iter().find_map(|fact| match fact {
                        archivefs_core::launch::VerifiedIdentityFact::NintendoDsGameKey(key) => {
                            Some(key.clone())
                        }
                        _ => None,
                    }),
                });
        let xenia_context =
            if let XeniaProfilesState::Ready(discovery) = &self.emulator_readiness.xenia_profiles {
                let roots = archivefs_core::patch_manager::XeniaProfileDiscoveryRoots {
                    explicit_configuration_roots: discovery
                        .profiles
                        .iter()
                        .map(|profile| profile.configuration_path.clone())
                        .collect(),
                };
                game_identity_report.map(|report| launch_readiness_page::XeniaLaunchContext {
                    discovery: discovery.clone(),
                    roots,
                    verified_xex_title_id: report.verified_xex_title_id().map(str::to_owned),
                    verified_xex_media_id: report.verified_xex_media_id().map(str::to_owned),
                })
            } else {
                None
            };

        let standalone_scans_complete = match &identity_status {
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) => {
                match identity.platform_id.as_str() {
                    "GameCube" | "Wii" => {
                        matches!(
                            self.emulator_readiness.dolphin_local_profiles,
                            DolphinLocalProfilesState::Ready(_)
                        )
                    }
                    "PS2" => matches!(
                        self.emulator_readiness.pcsx2_launch_profiles,
                        Pcsx2LaunchProfilesState::Ready(_)
                    ),
                    "Xbox360" => matches!(
                        self.emulator_readiness.xenia_profiles,
                        XeniaProfilesState::Ready(_)
                    ),
                    // These lanes are already discovered synchronously by
                    // this read-only input builder using their existing
                    // adapter roots.
                    "PSX" | "PSP" | "PS3" | "Xbox" => true,
                    "Amiga" => amiga_whdload_context.is_some(),
                    _ => true,
                }
            }
            archivefs_core::launch::CanonicalIdentityStatus::Unknown
            | archivefs_core::launch::CanonicalIdentityStatus::Conflicting => false,
        };

        let remembered: Vec<archivefs_core::launch::RememberedPreference> = self
            .emulator_readiness
            .remembered_emulator_profiles
            .iter()
            .map(|profile| archivefs_core::launch::RememberedPreference {
                adapter_id: profile.adapter.clone(),
                profile_id: profile.profile_id.clone(),
            })
            .collect();
        #[cfg(target_os = "linux")]
        let shadps4_context = if matches!(&identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) if identity.platform_id == "PS4")
        {
            focused.map(|path| {
                crate::emulator_setup::shadps4::Controller::snapshot(
                    &self.emulator_readiness.shadps4,
                    crate::emulator_setup::shadps4::Selection {
                        path: path.to_path_buf(),
                        identity: identity_status.clone(),
                    },
                )
            })
        } else {
            None
        };
        // ScummVM has no profile directory: the candidate is the one safe native
        // executable the detector already resolves, offered only for a resolved
        // ScummVM identity. Core re-detects and re-checks it at launch.
        if matches!(&identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) if identity.platform_id == "ScummVM")
            // The same verified engine:game gate the shared integration uses.
            && matches!(
                archivefs_core::launch::project_scummvm_launch_input(&verified_facts),
                archivefs_core::launch::LaunchInputProjection::Authorized(_)
            )
            && let Some(executable) =
                archivefs_core::scummvm_detection::resolve_scummvm_executable()
        {
            standalone_profiles.push(archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "scummvm",
                profile_id: format!("scummvm:{}", executable.display()),
                profile_path: Some(executable),
                eligible: true,
                firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
            });
        }
        let mesen_context = match (&identity_status, focused) {
            (archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity), Some(path))
                if identity.platform_id == "NES" =>
            {
                let snapshot = crate::emulator_setup::mesen::Controller::snapshot(
                    &self.emulator_readiness.mesen,
                    crate::emulator_setup::mesen::Selection {
                        path: path.into(),
                        identity: identity_status.clone(),
                    },
                );
                standalone_profiles.extend(snapshot.inputs());
                Some(snapshot)
            }
            _ => None,
        };
        let mut plan = archivefs_core::launch::build_launch_plan(
            &identity_status,
            &content,
            &standalone_profiles,
            retroarch_environment,
            &remembered,
        );
        archivefs_core::launch::nkit_readiness::inspect_and_apply(&mut plan);
        archivefs_core::launch::cemu_readiness::apply_cemu_readiness(
            &mut plan,
            self.emulator_readiness.cemu.inspection_for(focused),
        );
        if let Some(context) = &flycast_context {
            context.apply(&mut plan);
        }
        if let Some(context) = &cemu_context {
            context.apply(&mut plan);
        }
        launch_readiness_page::apply_mame_strict_preflight(&mut plan);
        if let Some(snapshot) = &mesen_context {
            snapshot.apply(&mut plan);
        }
        #[cfg(target_os = "linux")]
        if let Some(snapshot) = &shadps4_context {
            snapshot.apply(&mut plan, &content);
        }
        #[cfg(target_os = "linux")]
        let standalone_scans_complete = shadps4_context
            .as_ref()
            .map_or(standalone_scans_complete, |snapshot| {
                snapshot.inspection.is_some() || snapshot.error.is_some()
            });
        LaunchReadinessInput::Plan {
            plan,
            retroarch: match &self.emulator_readiness.retroarch_profiles {
                RetroArchProfilesState::Ready(discovery) => Some(
                    launch_readiness_page::retroarch_launch_context(&discovery.environment)
                        .with_multidisc(multidisc.clone()),
                ),
                RetroArchProfilesState::NotScanned
                | RetroArchProfilesState::Scanning { .. }
                | RetroArchProfilesState::Error(_) => None,
            },
            retroarch_scanned,
            standalone_scans_complete,
            dolphin: dolphin_context,
            pcsx2: pcsx2_context,
            duckstation: duckstation_context,
            ppsspp: ppsspp_context,
            rpcs3: rpcs3_context,
            xemu: xemu_context,
            xenia: xenia_context,
            melonds: melonds_context,
            cemu: cemu_context,
            flycast: flycast_context,
            amiga_whdload: amiga_whdload_context,
            #[cfg(target_os = "linux")]
            shadps4: shadps4_context,
        }
    }

    /// Starts the one-time ScummVM readiness probe - see
    /// [`identity_sources_page::ScummVmReadinessState`]'s own doc comment.
    /// The same "NotScanned, run once, never repeat automatically"
    /// convention as `start_dolphin_local_profile_scan`. Read-only: a
    /// filesystem stat plus (only if that succeeds) one bounded `--version`
    /// subprocess call, both already implemented in
    /// `archivefs_core::scummvm_detection`.
    pub(crate) fn start_scummvm_readiness_check(&mut self, context: egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.selected_evidence_ui.scummvm_readiness =
            identity_sources_page::ScummVmReadinessState::Checking { receiver };
        thread::spawn(move || {
            let readiness = identity_sources_page::gather_scummvm_readiness();
            let _ = sender.send(readiness);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_scummvm_readiness(&mut self) {
        if let identity_sources_page::ScummVmReadinessState::Checking { receiver } =
            &self.selected_evidence_ui.scummvm_readiness
            && let Ok(readiness) = receiver.try_recv()
        {
            self.selected_evidence_ui.scummvm_readiness =
                identity_sources_page::ScummVmReadinessState::Ready(readiness);
        }
    }

    /// GUI ScummVM Detection: starts a read-only detector check against
    /// every already-configured ScummVM folder in the loaded library. Never
    /// writes a file, renames anything, computes a hash, reads a DAT, or
    /// makes a network call - it only invokes
    /// `archivefs_core::scummvm_detection::detect_scummvm_directory_with_executable`,
    /// unchanged, once per folder.
    pub(crate) fn start_scummvm_check(&mut self, context: egui::Context, executable: PathBuf) {
        let Some(data) = (match &self.state {
            LoadState::Ready(data) => Some(data.as_ref()),
            _ => None,
        }) else {
            return;
        };
        let candidates = identity_sources_page::scummvm_candidates_from_rows(&data.rows);
        self.selected_evidence_ui.scummvm_check_generation += 1;
        let generation = self.selected_evidence_ui.scummvm_check_generation;
        let (sender, receiver) = mpsc::channel();
        self.selected_evidence_ui.scummvm_check =
            identity_sources_page::ScummVmCheckState::Checking {
                generation,
                receiver,
                checked: 0,
                total: candidates.len(),
                current: None,
            };
        thread::spawn(move || {
            let summary = identity_sources_page::check_scummvm_candidates(
                &executable,
                &candidates,
                |checked, total, current| {
                    let _ = sender.send((
                        generation,
                        identity_sources_page::ScummVmCheckMessage::Progress {
                            checked,
                            total,
                            current: current.to_string(),
                        },
                    ));
                },
            );
            let _ = sender.send((
                generation,
                identity_sources_page::ScummVmCheckMessage::Done(summary),
            ));
            context.request_repaint();
        });
    }

    /// Applies the pure [`identity_sources_page::ScummVmAction`] the panel
    /// returned this frame - the only thing it can ever ask for, and
    /// read-only.
    pub(crate) fn handle_scummvm_action(
        &mut self,
        context: &egui::Context,
        action: Option<identity_sources_page::ScummVmAction>,
    ) {
        if let Some(identity_sources_page::ScummVmAction::Check { executable }) = action {
            self.start_scummvm_check(context.clone(), executable);
        }
    }

    /// Drains progress/completion messages from an in-flight ScummVM check,
    /// discarding anything whose generation is no longer current - the same
    /// stale-result guard `poll_identity_sources` uses.
    pub(crate) fn poll_scummvm_check(&mut self) {
        if let identity_sources_page::ScummVmCheckState::Checking {
            generation,
            receiver,
            checked,
            total,
            current,
        } = &mut self.selected_evidence_ui.scummvm_check
        {
            let generation = *generation;
            while let Ok((message_generation, message)) = receiver.try_recv() {
                if message_generation != generation {
                    continue;
                }
                match message {
                    identity_sources_page::ScummVmCheckMessage::Progress {
                        checked: new_checked,
                        total: new_total,
                        current: new_current,
                    } => {
                        *checked = new_checked;
                        *total = new_total;
                        *current = Some(new_current);
                    }
                    identity_sources_page::ScummVmCheckMessage::Done(summary) => {
                        self.selected_evidence_ui.scummvm_check =
                            identity_sources_page::ScummVmCheckState::Ready {
                                generation,
                                summary,
                            };
                        return;
                    }
                }
            }
        }
    }

    pub(crate) fn start_pcsx2_profile_scan(&mut self, context: egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.history.record(HistoryEntry::new(
            ActivityAction::Pcsx2ProfileScan,
            None,
            ActivityOutcome::Started,
            "PCSX2 profile discovery started.",
        ));
        self.emulator_readiness.pcsx2_profiles = Pcsx2ProfilesState::Scanning { receiver };
        thread::spawn(move || {
            let result = Pcsx2ProfileDiscoveryRoots::from_environment()
                .and_then(|roots| discover_pcsx2_profiles(&roots))
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_pcsx2_profiles(&mut self) {
        if let Pcsx2ProfilesState::Scanning { receiver } = &self.emulator_readiness.pcsx2_profiles {
            match receiver.try_recv() {
                Ok(Ok(discovery)) => {
                    let eligible = eligible_pcsx2_profile_ids(&discovery);
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Pcsx2ProfileScan,
                        None,
                        ActivityOutcome::Completed,
                        format!(
                            "PCSX2 profile discovery found {} profiles ({} eligible).",
                            discovery.profiles.len(),
                            eligible.len()
                        ),
                    ));
                    if let Some(workflow) = self.cheat_workflow.as_mut()
                        && workflow.adapter == CheatEmulatorAdapter::Pcsx2
                        && workflow
                            .selected_pcsx2_profile_id
                            .as_ref()
                            .is_none_or(|selected| !eligible.contains(&selected.as_str()))
                    {
                        workflow.selected_pcsx2_profile_id =
                            (eligible.len() == 1).then(|| eligible[0].to_string());
                        workflow.pcsx2_inventory_profile_id = None;
                        workflow.pcsx2_inventory = CheatStepResource::NotLoaded;
                    }
                    self.emulator_readiness.pcsx2_profiles = Pcsx2ProfilesState::Ready(discovery);
                }
                Ok(Err(message)) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Pcsx2ProfileScan,
                        None,
                        ActivityOutcome::Failed,
                        format!("PCSX2 profile discovery failed: {message}"),
                    ));
                    self.emulator_readiness.pcsx2_profiles = Pcsx2ProfilesState::Error(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.emulator_readiness.pcsx2_profiles = Pcsx2ProfilesState::Error(
                        "PCSX2 profile discovery stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
    }

    pub(crate) fn start_dolphin_profile_scan(&mut self, context: egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.history.record(HistoryEntry::new(
            ActivityAction::DolphinProfileScan,
            None,
            ActivityOutcome::Started,
            "Dolphin profile discovery started.",
        ));
        self.emulator_readiness.dolphin_profiles = DolphinProfilesState::Scanning { receiver };
        let explicit_root = self
            .cheat_workflow
            .as_ref()
            .map(|workflow| workflow.dolphin_explicit_root.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        thread::spawn(move || {
            let result = DolphinProfileDiscoveryRoots::from_environment().map(|mut roots| {
                if let Some(explicit_root) = explicit_root {
                    roots.explicit_configuration_roots.push(explicit_root);
                }
                roots
            });
            let result = result
                .and_then(|roots| discover_dolphin_profiles(&roots))
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_dolphin_profiles(&mut self) {
        if let DolphinProfilesState::Scanning { receiver } =
            &self.emulator_readiness.dolphin_profiles
        {
            match receiver.try_recv() {
                Ok(Ok(discovery)) => {
                    let eligible = eligible_dolphin_profile_ids(&discovery);
                    self.history.record(HistoryEntry::new(
                        ActivityAction::DolphinProfileScan,
                        None,
                        ActivityOutcome::Completed,
                        format!(
                            "Dolphin profile discovery found {} profiles ({} eligible).",
                            discovery.profiles.len(),
                            eligible.len()
                        ),
                    ));
                    if let Some(workflow) = self.cheat_workflow.as_mut()
                        && workflow.adapter == CheatEmulatorAdapter::Dolphin
                    {
                        let session_explicit = workflow.dolphin_profile_choice.clone();
                        let selection =
                            select_dolphin_profile(&discovery, session_explicit.as_deref());
                        // Rule: never silently switch the profile bound to
                        // an install already reviewed/applied this session.
                        let install_in_progress =
                            !matches!(workflow.transaction, CheatTransactionState::Idle);
                        if let EmulatorProfileSelection::Auto { profile_id, .. } = &selection
                            && !install_in_progress
                            && workflow.selected_dolphin_profile_id.as_deref()
                                != Some(profile_id.as_str())
                        {
                            workflow.selected_dolphin_profile_id = Some(profile_id.clone());
                            workflow.dolphin_inventory_profile_id = None;
                            workflow.dolphin_inventory = CheatStepResource::NotLoaded;
                            workflow.dolphin_activation = CheatActivationReadiness::Unknown;
                            workflow.dolphin_activation_receiver = None;
                        } else if !matches!(selection, EmulatorProfileSelection::Auto { .. })
                            && !install_in_progress
                            && workflow
                                .selected_dolphin_profile_id
                                .as_ref()
                                .is_none_or(|selected| !eligible.contains(&selected.as_str()))
                        {
                            workflow.selected_dolphin_profile_id = None;
                            workflow.dolphin_inventory_profile_id = None;
                            workflow.dolphin_inventory = CheatStepResource::NotLoaded;
                            workflow.dolphin_activation = CheatActivationReadiness::Unknown;
                            workflow.dolphin_activation_receiver = None;
                        }
                        workflow.dolphin_profile_selection = Some(selection);
                    }
                    self.emulator_readiness.dolphin_profiles =
                        DolphinProfilesState::Ready(discovery);
                    if let (Some(workflow), DolphinProfilesState::Ready(discovery)) = (
                        self.cheat_workflow.as_mut(),
                        &self.emulator_readiness.dolphin_profiles,
                    ) && workflow.adapter == CheatEmulatorAdapter::Dolphin
                        && matches!(workflow.transaction, CheatTransactionState::Idle)
                    {
                        reconcile_dolphin_provider_selection(workflow, discovery);
                    }
                }
                Ok(Err(message)) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::DolphinProfileScan,
                        None,
                        ActivityOutcome::Failed,
                        format!("Dolphin profile discovery failed: {message}"),
                    ));
                    self.emulator_readiness.dolphin_profiles = DolphinProfilesState::Error(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.emulator_readiness.dolphin_profiles = DolphinProfilesState::Error(
                        "Dolphin profile discovery stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
    }

    /// Starts the modern-model Dolphin profile discovery Launch Readiness
    /// needs to build a native launch binding - see
    /// [`DolphinLocalProfilesState`]'s own doc comment. Read-only: probes
    /// only known XDG/Flatpak paths and any explicit roots already
    /// remembered, never writes anything.
    pub(crate) fn start_dolphin_local_profile_scan(&mut self, context: egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.emulator_readiness.dolphin_local_profiles =
            DolphinLocalProfilesState::Scanning { receiver };
        thread::spawn(move || {
            let result =
                archivefs_core::patch_manager::DolphinLocalDiscoveryRoots::from_environment()
                    .map_err(|error| error.to_string())
                    .map(|roots| {
                        let discovery =
                            archivefs_core::patch_manager::discover_dolphin_local_profiles(&roots);
                        DolphinLocalProfilesReady { discovery, roots }
                    });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_dolphin_local_profiles(&mut self) {
        if let DolphinLocalProfilesState::Scanning { receiver } =
            &self.emulator_readiness.dolphin_local_profiles
        {
            match receiver.try_recv() {
                Ok(Ok(ready)) => {
                    self.emulator_readiness.dolphin_local_profiles =
                        DolphinLocalProfilesState::Ready(ready);
                }
                Ok(Err(message)) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::DolphinProfileScan,
                        None,
                        ActivityOutcome::Failed,
                        format!("Dolphin launch-profile discovery failed: {message}"),
                    ));
                    self.emulator_readiness.dolphin_local_profiles =
                        DolphinLocalProfilesState::Error(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let message = "Dolphin profile discovery stopped unexpectedly.".to_string();
                    self.history.record(HistoryEntry::new(
                        ActivityAction::DolphinProfileScan,
                        None,
                        ActivityOutcome::Failed,
                        format!("Dolphin launch-profile discovery failed: {message}"),
                    ));
                    self.emulator_readiness.dolphin_local_profiles =
                        DolphinLocalProfilesState::Error(message);
                }
            }
        }
    }

    /// Starts the PCSX2 profile discovery Launch Readiness needs to build a
    /// native launch binding - see [`Pcsx2LaunchProfilesState`]'s own doc
    /// comment. Read-only, same shape as
    /// [`Self::start_dolphin_local_profile_scan`].
    pub(crate) fn start_pcsx2_launch_profile_scan(&mut self, context: egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.emulator_readiness.pcsx2_launch_profiles =
            Pcsx2LaunchProfilesState::Scanning { receiver };
        thread::spawn(move || {
            let result = Pcsx2ProfileDiscoveryRoots::from_environment()
                .map_err(|error| error.to_string())
                .and_then(|mut roots| {
                    // Feed an already-validated EmuWiz-managed PCSX2 AppImage
                    // (if one exists) as a caller-confirmed explicit
                    // executable. `discover_managed_appimage_installations`
                    // (bounded, read-only, no shell - `install.json`-backed
                    // entries only) is re-run here so this scan does not
                    // depend on when the Doctor gather ran.
                    roots
                        .explicit_executables
                        .extend(managed_appimage_explicit_executables(
                            &discover_managed_appimage_installations(),
                            "PCSX2",
                        ));
                    discover_pcsx2_profiles(&roots)
                        .map_err(|error| error.to_string())
                        .map(|discovery| Pcsx2LaunchProfilesReady { discovery, roots })
                });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_pcsx2_launch_profiles(&mut self) {
        if let Pcsx2LaunchProfilesState::Scanning { receiver } =
            &self.emulator_readiness.pcsx2_launch_profiles
        {
            match receiver.try_recv() {
                Ok(Ok(ready)) => {
                    self.emulator_readiness.pcsx2_launch_profiles =
                        Pcsx2LaunchProfilesState::Ready(ready);
                }
                Ok(Err(message)) => {
                    self.emulator_readiness.pcsx2_launch_profiles =
                        Pcsx2LaunchProfilesState::Error(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.emulator_readiness.pcsx2_launch_profiles = Pcsx2LaunchProfilesState::Error(
                        "PCSX2 launch-profile discovery stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
    }

    /// Forgets the DuckStation resolver answer and bumps the generation so a
    /// result still in flight for the earlier request is discarded.
    pub(crate) fn invalidate_duckstation_resolution(&mut self) {
        self.emulator_readiness.duckstation_resolution_generation += 1;
        self.emulator_readiness.duckstation_resolution = DuckStationResolutionState::NotScanned;
    }

    /// Starts the read-only DuckStation resolution off the GUI thread.
    pub(crate) fn start_duckstation_resolution(&mut self, context: Option<egui::Context>) {
        let managed = managed_appimage_explicit_executables(
            &discover_managed_appimage_installations(),
            "DuckStation",
        );
        let inputs = DuckStationResolutionInputs::from_environment(
            &self.emulator_readiness.emulator_setup_overrides,
            &managed,
        );
        self.start_duckstation_resolution_with(inputs, context);
    }

    pub(crate) fn start_duckstation_resolution_with(
        &mut self,
        inputs: Result<DuckStationResolutionInputs, String>,
        context: Option<egui::Context>,
    ) {
        self.emulator_readiness.duckstation_resolution_generation += 1;
        let generation = self.emulator_readiness.duckstation_resolution_generation;
        self.emulator_readiness.duckstation_resolution = match inputs {
            Ok(inputs) => DuckStationResolutionState::Scanning {
                receiver: spawn_duckstation_resolution(generation, inputs, context),
            },
            Err(message) => DuckStationResolutionState::Error(message),
        };
    }

    /// Accepts a finished resolution only if it answers the current request.
    pub(crate) fn accept_duckstation_resolution(
        &mut self,
        generation: u64,
        view: DuckStationResolutionView,
    ) -> bool {
        if generation != self.emulator_readiness.duckstation_resolution_generation {
            return false;
        }
        self.emulator_readiness.duckstation_resolution = DuckStationResolutionState::Ready {
            generation,
            view: Box::new(view),
        };
        true
    }

    pub(crate) fn poll_duckstation_resolution(&mut self) {
        let DuckStationResolutionState::Scanning { receiver } =
            &self.emulator_readiness.duckstation_resolution
        else {
            return;
        };
        match receiver.try_recv() {
            Ok((generation, view)) => {
                self.accept_duckstation_resolution(generation, view);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.emulator_readiness.duckstation_resolution = DuckStationResolutionState::Error(
                    "DuckStation profile resolution stopped unexpectedly.".to_string(),
                );
            }
        }
    }

    fn show_duckstation_profile_resolution(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if matches!(
            self.emulator_readiness.duckstation_resolution,
            DuckStationResolutionState::NotScanned
        ) {
            self.start_duckstation_resolution(Some(context.clone()));
        }
        ui.add_space(theme::SECTION_GAP);
        let mut recheck = false;
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("DuckStation: shared resolver selection");
                recheck = ui.button("Re-check").clicked();
            });
            ui.label(
                egui::RichText::new(
                    "This is EmuWiz's shared resolver selection. Launch uses this result for DuckStation; Cheats & Mods is not connected to it yet. Changing saved preferences is not supported here yet.",
                )
                .small()
                .color(theme::muted(ui)),
            );
            match &self.emulator_readiness.duckstation_resolution {
                DuckStationResolutionState::NotScanned
                | DuckStationResolutionState::Scanning { .. } => {
                    ui.label("Checking DuckStation installations…");
                }
                DuckStationResolutionState::Error(message) => {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
                DuckStationResolutionState::Ready { view, .. } => {
                    show_profile_resolution(ui, &present_resolution(view));
                }
            }
        });
        if recheck {
            self.invalidate_duckstation_resolution();
        }
    }

    /// Forgets the PPSSPP resolver answer and bumps the generation so a result
    /// still in flight for the earlier request is discarded.
    pub(crate) fn invalidate_ppsspp_resolution(&mut self) {
        self.emulator_readiness.ppsspp_resolution_generation += 1;
        self.emulator_readiness.ppsspp_resolution = PpssppResolutionState::NotScanned;
    }

    /// Starts the read-only PPSSPP resolution off the GUI thread.
    pub(crate) fn start_ppsspp_resolution(&mut self, context: Option<egui::Context>) {
        let inputs = PpssppResolutionInputs::from_environment(
            &self.emulator_readiness.emulator_setup_overrides,
        );
        self.start_ppsspp_resolution_with(inputs, context);
    }

    pub(crate) fn start_ppsspp_resolution_with(
        &mut self,
        inputs: Result<PpssppResolutionInputs, String>,
        context: Option<egui::Context>,
    ) {
        self.emulator_readiness.ppsspp_resolution_generation += 1;
        let generation = self.emulator_readiness.ppsspp_resolution_generation;
        self.emulator_readiness.ppsspp_resolution = match inputs {
            Ok(inputs) => PpssppResolutionState::Scanning {
                receiver: spawn_ppsspp_resolution(generation, inputs, context),
            },
            Err(message) => PpssppResolutionState::Error(message),
        };
    }

    /// Accepts a finished resolution only if it answers the current request.
    pub(crate) fn accept_ppsspp_resolution(
        &mut self,
        generation: u64,
        view: ResolutionView,
    ) -> bool {
        if generation != self.emulator_readiness.ppsspp_resolution_generation {
            return false;
        }
        self.emulator_readiness.ppsspp_resolution = PpssppResolutionState::Ready {
            generation,
            view: Box::new(view),
        };
        true
    }

    pub(crate) fn poll_ppsspp_resolution(&mut self) {
        let PpssppResolutionState::Scanning { receiver } =
            &self.emulator_readiness.ppsspp_resolution
        else {
            return;
        };
        match receiver.try_recv() {
            Ok((generation, view)) => {
                self.accept_ppsspp_resolution(generation, view);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.emulator_readiness.ppsspp_resolution = PpssppResolutionState::Error(
                    "PPSSPP profile resolution stopped unexpectedly.".to_string(),
                );
            }
        }
    }

    fn show_ppsspp_profile_resolution(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if matches!(
            self.emulator_readiness.ppsspp_resolution,
            PpssppResolutionState::NotScanned
        ) {
            self.start_ppsspp_resolution(Some(context.clone()));
        }
        ui.add_space(theme::SECTION_GAP);
        let mut recheck = false;
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("PPSSPP: shared resolver recommendation (preview)");
                recheck = ui.button("Re-check PPSSPP").clicked();
            });
            ui.label(
                egui::RichText::new(
                    "This is what EmuWiz's shared resolver recommends. Launch and Cheats & Mods are not connected to it yet, so it does not guarantee which installation they use. Changing saved preferences is not supported here yet.",
                )
                .small()
                .color(theme::muted(ui)),
            );
            match &self.emulator_readiness.ppsspp_resolution {
                PpssppResolutionState::NotScanned | PpssppResolutionState::Scanning { .. } => {
                    ui.label("Checking PPSSPP installations…");
                }
                PpssppResolutionState::Error(message) => {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
                PpssppResolutionState::Ready { view, .. } => {
                    show_profile_resolution(
                        ui,
                        &present_resolution_for(view, ResolverSubject::Ppsspp),
                    );
                }
            }
        });
        if recheck {
            self.invalidate_ppsspp_resolution();
        }
    }

    /// Starts read-only Flycast profile discovery for Launch Readiness. No
    /// configuration or emulator state is written by this scan.
    pub(crate) fn start_flycast_profile_scan(&mut self, context: egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.emulator_readiness.flycast_profiles = FlycastProfilesState::Scanning { receiver };
        thread::spawn(move || {
            let result = FlycastProfileDiscoveryRoots::from_environment()
                .map_err(|error| error.to_string())
                .map(|roots| {
                    let discovery =
                        archivefs_core::patch_manager::discover_flycast_profiles(&roots);
                    FlycastProfilesReady { discovery }
                });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_flycast_profiles(&mut self) {
        if let FlycastProfilesState::Scanning { receiver } =
            &self.emulator_readiness.flycast_profiles
        {
            match receiver.try_recv() {
                Ok(Ok(ready)) => {
                    self.emulator_readiness.flycast_profiles = FlycastProfilesState::Ready(ready)
                }
                Ok(Err(message)) => {
                    self.emulator_readiness.flycast_profiles = FlycastProfilesState::Error(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.emulator_readiness.flycast_profiles = FlycastProfilesState::Error(
                        "Flycast profile discovery stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
    }

    /// Starts the shared background load of PS1/PS2 firmware evidence from the
    /// user's registered DAT sources - see
    /// [`load_pcsx2_firmware_evidence_from_registry`]. Read-only: parses
    /// DAT files already on disk, never downloads or writes anything.
    pub(crate) fn start_pcsx2_firmware_evidence_load(&mut self, context: egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.emulator_readiness.pcsx2_firmware_evidence =
            Pcsx2FirmwareEvidenceState::Loading { receiver };
        thread::spawn(move || {
            let result = load_pcsx2_firmware_evidence_from_registry();
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_pcsx2_firmware_evidence(&mut self) {
        if let Pcsx2FirmwareEvidenceState::Loading { receiver } =
            &self.emulator_readiness.pcsx2_firmware_evidence
        {
            match receiver.try_recv() {
                Ok(Ok(evidence)) => {
                    self.emulator_readiness.pcsx2_firmware_evidence =
                        Pcsx2FirmwareEvidenceState::Ready(evidence);
                }
                Ok(Err(message)) => {
                    self.emulator_readiness.pcsx2_firmware_evidence =
                        Pcsx2FirmwareEvidenceState::Error(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.emulator_readiness.pcsx2_firmware_evidence =
                        Pcsx2FirmwareEvidenceState::Error(
                            "PS2 firmware evidence load stopped unexpectedly.".to_string(),
                        );
                }
            }
        }
    }

    /// Starts a background RetroArch profile discovery scan - blocking
    /// filesystem probing, so it never runs on the UI thread, exactly
    /// like every other workflow. The result replaces the previous
    /// discovery wholesale.
    pub(crate) fn start_retroarch_profile_scan(&mut self, context: egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.history.record(HistoryEntry::new(
            ActivityAction::RetroArchProfileScan,
            None,
            ActivityOutcome::Started,
            "RetroArch profile discovery started.",
        ));
        self.emulator_readiness.retroarch_profiles = RetroArchProfilesState::Scanning { receiver };
        let core_directory_override = self
            .emulator_readiness
            .retroarch_core_directory_override
            .clone();
        thread::spawn(move || {
            let filesystem = HostReadOnlyFilesystem;
            let environment = DiscoveryEnvironment::from_process_environment();
            let result = discover_retroarch_cheat_setup_profiles_with_core_directory_override(
                &filesystem,
                &environment,
                None,
                core_directory_override.as_deref(),
            )
            .map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Persist an explicit EmuWiz override for the RetroArch core directory
    /// and keep the in-memory copy in sync. This does not itself trigger a
    /// rescan (matching the `save_gui_mode` convention); callers that need
    /// fresh core state run `start_retroarch_profile_scan` next - the
    /// Emulator Setup repair flow does exactly that.
    pub(crate) fn set_retroarch_core_directory_override(&mut self, path: PathBuf) {
        self.emulator_readiness.retroarch_core_directory_override = Some(path);
        save_retroarch_core_directory_override(
            self.emulator_readiness
                .retroarch_core_directory_override
                .as_deref(),
        );
    }

    /// Drop any persisted RetroArch core-directory override, returning
    /// discovery to fully automatic `retroarch.cfg` resolution. Does not
    /// rescan on its own; the "Reset to automatic" control rescans after.
    pub(crate) fn clear_retroarch_core_directory_override(&mut self) {
        self.emulator_readiness.retroarch_core_directory_override = None;
        save_retroarch_core_directory_override(None);
    }

    /// Emulator Setup "Choose core folder" outcome. A picked directory is
    /// validated (exists + is a directory) *before* it is persisted; on
    /// success it is saved through the shared helper and a fresh RetroArch
    /// profile scan is started so `matching_retroarch_cores` ->
    /// `build_retroarch_candidates` -> `build_launch_plan` -> gamer
    /// readiness all update naturally. An unusable pick is reported and
    /// never stored, so the active core folder is unchanged.
    pub(crate) fn apply_picked_retroarch_core_folder(
        &mut self,
        folder: PathBuf,
        context: egui::Context,
    ) {
        match retroarch_core_setup::classify_picked_core_folder(&folder) {
            retroarch_core_setup::PickedCoreFolder::Directory => {
                self.emulator_readiness.retroarch_core_folder_rejected_pick = None;
                self.set_retroarch_core_directory_override(folder);
                self.start_retroarch_profile_scan(context);
            }
            retroarch_core_setup::PickedCoreFolder::Unusable => {
                self.emulator_readiness.retroarch_core_folder_rejected_pick = Some(folder);
            }
        }
    }

    pub(crate) fn poll_retroarch_profiles(&mut self) {
        if let RetroArchProfilesState::Scanning { receiver } =
            &self.emulator_readiness.retroarch_profiles
        {
            match receiver.try_recv() {
                Ok(Ok(discovery)) => {
                    let eligible = discovery
                        .profiles
                        .iter()
                        .filter(|profile| profile.eligible)
                        .count();
                    self.history.record(HistoryEntry::new(
                        ActivityAction::RetroArchProfileScan,
                        None,
                        ActivityOutcome::Completed,
                        format!(
                            "RetroArch profile discovery found {} profiles ({} eligible).",
                            discovery.profiles.len(),
                            eligible
                        ),
                    ));
                    let automatic_profile = {
                        let eligible = eligible_profile_ids(&discovery);
                        (eligible.len() == 1).then(|| eligible[0].to_string())
                    };
                    self.emulator_readiness.retroarch_profiles =
                        RetroArchProfilesState::Ready(discovery);
                    if let Some(workflow) = self.cheat_workflow.as_mut()
                        && workflow.selected_profile_id.is_none()
                    {
                        workflow.selected_profile_id = automatic_profile;
                        workflow.existing_library_profile_id = None;
                        workflow.existing_library = CheatStepResource::NotLoaded;
                    }
                }
                Ok(Err(message)) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::RetroArchProfileScan,
                        None,
                        ActivityOutcome::Failed,
                        format!("RetroArch profile discovery failed: {message}"),
                    ));
                    self.emulator_readiness.retroarch_profiles =
                        RetroArchProfilesState::Error(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.emulator_readiness.retroarch_profiles = RetroArchProfilesState::Error(
                        "RetroArch profile discovery stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
    }

    /// Looks up the remembered profile id for an adapter key (`"dolphin"`
    /// or `"xenia"`), if any.
    pub(crate) fn remembered_profile_id(&self, adapter: &str) -> Option<String> {
        remembered_profile_for(
            &self.emulator_readiness.remembered_emulator_profiles,
            adapter,
        )
        .filter(|profile| adapter != "dolphin" || !is_dolphin_standard_fallback_root(&profile.root))
        .map(|profile| profile.profile_id.clone())
    }

    /// Looks up the remembered profile's root directory for an adapter
    /// key - used to seed the explicit-root text field so a remembered
    /// portable/explicit profile is rediscovered without the user typing
    /// it again every session.
    pub(crate) fn remembered_profile_root(&self, adapter: &str) -> Option<PathBuf> {
        remembered_profile_for(
            &self.emulator_readiness.remembered_emulator_profiles,
            adapter,
        )
        .filter(|profile| adapter != "dolphin" || !is_dolphin_standard_fallback_root(&profile.root))
        .map(|profile| profile.root.clone())
    }

    /// Persists `profile_id`/`root` as the remembered profile for
    /// `adapter`, updating the in-memory cache immediately so the rest of
    /// the session sees it without a reload. The write is a small local
    /// file (atomic rename) - failures are non-fatal and only recorded in
    /// the Activity Log, never surfaced as a blocking error, since the
    /// session-level selection already succeeded regardless of whether it
    /// could be remembered for next time.
    pub(crate) fn persist_remembered_profile(
        &mut self,
        adapter: &str,
        profile_id: &str,
        root: &Path,
    ) {
        let already_current = remembered_profile_for(
            &self.emulator_readiness.remembered_emulator_profiles,
            adapter,
        )
        .is_some_and(|profile| profile.profile_id == profile_id && profile.root == root);
        if already_current {
            return;
        }
        // The real on-disk write is skipped under `cargo test`: it would
        // otherwise write to the developer's actual
        // `~/.config/archivefs/emulator_profiles.toml` every time a test
        // drives profile discovery to a resolved selection, exactly the
        // kind of real-filesystem side effect the rest of this test suite
        // never has (config-mutating core functions are only ever called
        // from the real app entry point, never from GUI unit tests). The
        // in-memory cache is still updated unconditionally, so selection
        // and chooser behavior remain fully testable.
        #[cfg(not(test))]
        let write_result = archivefs_core::patch_manager::remember_emulator_profile_default(
            adapter, profile_id, root,
        );
        #[cfg(test)]
        let write_result: Result<(), ArchiveFsError> = Ok(());
        match write_result {
            Ok(()) => {
                self.emulator_readiness
                    .remembered_emulator_profiles
                    .retain(|profile| profile.adapter != adapter);
                self.emulator_readiness.remembered_emulator_profiles.push(
                    RememberedEmulatorProfile {
                        adapter: adapter.to_string(),
                        profile_id: profile_id.to_string(),
                        root: root.to_path_buf(),
                    },
                );
            }
            Err(error) => {
                let action = if adapter == "xenia" {
                    ActivityAction::XeniaProfileScan
                } else {
                    ActivityAction::DolphinProfileScan
                };
                self.history.record(HistoryEntry::new(
                    action,
                    None,
                    ActivityOutcome::Failed,
                    format!("Could not remember the chosen {adapter} profile: {error}"),
                ));
            }
        }
    }
}

/// The bridge from the persisted DAT source registry
/// (`archivefs_core::dat::sources::DatSourceRegistry` - the same one
/// [`gather_selected_evidence_with_registry`] reads) into
/// [`archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord`]
/// values PCSX2 Launch Readiness needs to genuinely verify a BIOS - see
/// `launch_readiness_page`'s Launch PCSX2 doc comment.
///
/// Never downloads anything and never invents a record: every registered,
/// enabled source's file(s) are parsed with the exact same
/// [`archivefs_core::dat::parsers::parse_dat_file`] the DAT Sources page
/// itself uses, then handed to
/// [`archivefs_core::dat::firmware_evidence::ps2_bios_evidence_from_dat`],
/// which only ever yields records for a DAT it can itself prove is the
/// Redump PS2 BIOS dataset (ecosystem plus dataset-identifying header text) -
/// an unrelated ROM-set DAT, or one that fails to parse, silently
/// contributes nothing rather than erroring the whole scan. A source's own
/// `platform` label is never trusted as extraction authority here, for the
/// same "never treat an arbitrary DAT as authoritative" reason
/// `ps2_bios_evidence_from_dat` itself documents - every enabled source is
/// tried, and only what genuinely re-parses as the right dataset survives.
///
/// Runs entirely off the UI thread (see
/// [`App::start_pcsx2_firmware_evidence_load`]) - registered DAT files can
/// be large, so this is never called from `build_launch_readiness_input`,
/// which runs every frame the Selected page is shown.
pub(crate) fn pcsx2_firmware_evidence_from_registry(
    registry: &archivefs_core::dat::sources::DatSourceRegistry,
) -> Vec<archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord> {
    use archivefs_core::dat::firmware_evidence::{FirmwareSystem, redump_bios_evidence_from_dat};
    use archivefs_core::dat::limits::DatLimits;
    use archivefs_core::dat::parsers::parse_dat_file;
    use archivefs_core::dat::sources::{DatSourceKind, discover_dat_files};

    let mut evidence = Vec::new();
    for entry in registry.sorted_enabled() {
        let files: Vec<PathBuf> = match entry.kind {
            DatSourceKind::File => vec![entry.path.clone()],
            DatSourceKind::Folder => discover_dat_files(&entry.path)
                .map(|scan| scan.files)
                .unwrap_or_default(),
        };
        for file in files {
            if let Ok(outcome) = parse_dat_file(&file, DatLimits::default()) {
                // Keep one asynchronous registry cache. The shared matcher
                // checks each record's system, so PS1 never verifies PS2.
                for system in [FirmwareSystem::PlayStation, FirmwareSystem::PlayStation2] {
                    if let Ok(records) = redump_bios_evidence_from_dat(&outcome.dat, system) {
                        evidence.extend(records);
                    }
                }
            }
        }
    }
    evidence
}

/// [`pcsx2_firmware_evidence_from_registry`] with the registry loaded fresh
/// from `default_dat_sources_config_path()` - the same on-disk file the DAT
/// Sources page reads and writes, never a second persistent registry. An
/// absent config file (nothing registered yet) or an unresolvable path
/// (e.g. `HOME` unset) both honestly resolve to zero evidence records,
/// mirroring `gather_selected_evidence_with_registry`'s own fallback -
/// never an error banner for the ordinary "nothing configured yet" case.
/// Only a genuine read/parse failure of the registry file itself is
/// reported as `Err`.
pub(crate) fn load_pcsx2_firmware_evidence_from_registry()
-> Result<Vec<archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord>, String> {
    let Ok(config_path) = archivefs_core::dat::sources::default_dat_sources_config_path() else {
        return Ok(Vec::new());
    };
    let config = archivefs_core::dat::sources::load_dat_sources_config_from(&config_path)
        .map_err(|error| error.to_string())?;
    let (registry, _warnings) =
        archivefs_core::dat::sources::DatSourceRegistry::from_config(&config);
    Ok(pcsx2_firmware_evidence_from_registry(&registry))
}

/// The caller-confirmed local executable paths to add to a standalone
/// adapter's `explicit_executables` for `emulator` (a
/// [`LinuxEmulatorInstallationEvidence::emulator`] display name, e.g.
/// `"PPSSPP"` / `"PCSX2"`), taken **only** from an `install.json`-backed
/// EmuWiz-managed AppImage already present in `installations` - see
/// [`managed_appimage_executable_for`] for the exact trust rule (managed
/// form only; never a plain `~/Applications` AppImage, a Flatpak,
/// `$APPIMAGE`, `PATH`, config-only evidence, a lossy path, or an ambiguous
/// multi-match).
///
/// Returns an empty vec whenever no such validated install exists, so
/// feeding it into `ProfileDiscoveryRoots` is a no-op on any machine that
/// does not have one - launch readiness there is byte-for-byte unchanged.
pub(crate) fn managed_appimage_explicit_executables(
    installations: &[LinuxEmulatorInstallationEvidence],
    emulator: &str,
) -> Vec<PathBuf> {
    managed_appimage_executable_for(installations, emulator)
        .into_iter()
        .collect()
}

/// Writes (or, for `None`, removes) the override at an explicit file path.
/// Best-effort: a persistence failure never blocks the in-memory value
/// from taking effect for the session, exactly like `save_gui_mode`.
pub(crate) fn save_retroarch_core_directory_override_at(path: &Path, value: Option<&Path>) {
    match value {
        Some(dir) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, dir.to_string_lossy().as_ref());
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(crate) fn save_retroarch_core_directory_override(value: Option<&Path>) {
    if let Some(path) = retroarch_core_directory_override_path() {
        save_retroarch_core_directory_override_at(&path, value);
    }
}

/// The selected-evidence worker's diagnostic for a CUE disc whose files could
/// not be resolved, without the folder path (the file name is what matters).
fn disc_files_diagnostic(
    report: &archivefs_core::game_identity::GameIdentityReport,
    focused: Option<&Path>,
) -> Option<String> {
    const PREFIX: &str = "CUE data track could not be resolved: ";
    let diagnostic = report
        .evidence
        .iter()
        .find_map(|evidence| evidence.diagnostic.strip_prefix(PREFIX))?;
    let folder = focused
        .and_then(Path::parent)
        .map(|parent| format!("{}/", parent.display()));
    let text = match folder {
        Some(folder) => diagnostic.replace(&folder, ""),
        None => diagnostic.to_string(),
    };
    Some(format!("This disc cannot be launched: {text}"))
}
