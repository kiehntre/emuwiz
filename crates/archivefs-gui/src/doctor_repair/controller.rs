use crate::*;

impl ArchiveFsApp {
    /// Starts a read-only Doctor scan.
    ///
    /// Deliberately **not** `self.refresh(context)`: this never reloads the
    /// application, never rescans the library, never creates the mount root,
    /// and never touches the database. The path-based inputs are gathered on
    /// a worker thread; the preloaded, session-owned inputs are borrowed
    /// when the result arrives.
    ///
    /// Cancellation follows the existing generation pattern: a superseded
    /// run's result is discarded on arrival, and the previous result stays
    /// visible until a newer one completes.
    pub(crate) fn start_doctor_scan(&mut self, context: egui::Context) {
        let generation = self.doctor_scan_generation.next();
        self.doctor_scan_generation = generation;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send((generation, gather_doctor_inputs()));
            context.request_repaint();
        });
        let previous = match std::mem::replace(&mut self.doctor_scan, DoctorScanState::NotRun) {
            DoctorScanState::Ready(outcome) => Some(outcome),
            DoctorScanState::Running { previous, .. } => previous,
            DoctorScanState::NotRun => None,
        };
        self.doctor_scan = DoctorScanState::Running {
            generation,
            receiver,
            previous,
        };
    }

    /// Completes a Doctor scan once its worker delivers the gathered inputs.
    ///
    /// The pure runner itself is executed here, on the main thread, because
    /// it does no I/O and needs to borrow the session-owned inputs
    /// (`LoadedData::doctor`, the cached live health issues, and any already
    /// discovered RetroArch environment). None of those is re-collected: if
    /// a subsystem has not been loaded in this session it is reported as not
    /// checked, never as a pass.
    pub(crate) fn poll_doctor_scan(&mut self) {
        let received = match &self.doctor_scan {
            DoctorScanState::Running {
                generation,
                receiver,
                ..
            } => match receiver.try_recv() {
                Ok((received_generation, gathered)) => {
                    Some((received_generation == *generation, gathered))
                }
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // The worker vanished. Report it once, against the
                    // first subsystem, rather than as four identical copies.
                    Some((
                        true,
                        DoctorGathered {
                            mount_root_safety: Gathered::Failed(
                                "the Doctor worker stopped unexpectedly".to_string(),
                            ),
                            database: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            source_health: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            transactions: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            stale_mount_directories: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            index_freshness: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            storage: Gathered::NotLoaded("not gathered: the Doctor worker stopped"),
                            emulator_profiles: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            linux_emulator_installations: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            arcade_dat_version: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            xemu_readiness: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            xenia_readiness: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            ppsspp_readiness: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            rpcs3_readiness: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                            managed_entries: Gathered::NotLoaded(
                                "not gathered: the Doctor worker stopped",
                            ),
                        },
                    ))
                }
            },
            DoctorScanState::NotRun | DoctorScanState::Ready(_) => None,
        };
        let Some((is_current, gathered)) = received else {
            return;
        };
        if !is_current {
            // A newer run superseded this one; discard it and keep waiting.
            return;
        }

        // `cached_health_issues` needs `&mut self`; copying the small,
        // already-built vector out ends that borrow before the immutable
        // borrows below - the same pattern the Health tab already uses.
        let health_issues = self.cached_health_issues().to_vec();
        let doctor_report = match &self.state {
            LoadState::Ready(data) => Gathered::Ready(&data.doctor),
            LoadState::Loading { .. } | LoadState::Error(_) => Gathered::NotLoaded(
                "The library has not finished loading, so the archive-scan and mount-status checks were not available.",
            ),
        };
        let retroarch = match &self.retroarch_profiles {
            RetroArchProfilesState::Ready(discovery) => Gathered::Ready(&discovery.environment),
            RetroArchProfilesState::Error(message) => Gathered::Failed(message.clone()),
            RetroArchProfilesState::NotScanned | RetroArchProfilesState::Scanning { .. } => {
                Gathered::NotLoaded(
                    "RetroArch profiles have not been discovered in this session. Doctor never starts that scan itself; use Settings to rescan.",
                )
            }
        };

        // The already-computed setup diagnostics, never recomputed here:
        // recomputing would run the mount-root write probe.
        let setup = match &self.diagnostics {
            DiagnosticsState::Ready { report, .. } => Gathered::Ready(report),
            DiagnosticsState::Error { message, .. } => Gathered::Failed(message.clone()),
            DiagnosticsState::Loading { .. } => Gathered::NotLoaded(
                "Configuration diagnostics are still loading. Doctor reuses them rather than re-running them, because that check writes a temporary file to test the mount root.",
            ),
        };
        let inputs = DoctorScanInputs {
            doctor_report,
            setup,
            health_issues: Gathered::Ready(health_issues.as_slice()),
            source_health: borrowed(&gathered.source_health, |value| value.as_slice()),
            database: borrowed(&gathered.database, |value| value),
            mount_root_safety: borrowed(&gathered.mount_root_safety, |value| value),
            retroarch,
            transactions: borrowed(&gathered.transactions, |value| value),
            stale_mount_directories: borrowed(&gathered.stale_mount_directories, |value| {
                value.as_slice()
            }),
            index_freshness: match &gathered.index_freshness {
                Gathered::Ready((freshness, path)) => Gathered::Ready((freshness, path.as_path())),
                Gathered::Failed(reason) => Gathered::Failed(reason.clone()),
                Gathered::NotLoaded(reason) => Gathered::NotLoaded(reason),
            },
            storage: borrowed(&gathered.storage, |value| value),
            emulator_profiles: borrowed(&gathered.emulator_profiles, |value| value),
            linux_emulator_installations: borrowed(
                &gathered.linux_emulator_installations,
                |value| value.as_slice(),
            ),
            arcade_dat_version: borrowed(&gathered.arcade_dat_version, |value| value.as_slice()),
            xemu_readiness: borrowed(&gathered.xemu_readiness, |value| value.as_slice()),
            xenia_readiness: borrowed(&gathered.xenia_readiness, |value| value.as_slice()),
            ppsspp_readiness: borrowed(&gathered.ppsspp_readiness, |value| value.as_slice()),
            rpcs3_readiness: borrowed(&gathered.rpcs3_readiness, |value| value.as_slice()),
            managed_entries: borrowed(&gathered.managed_entries, |value| value),
            // The verified-identity fact cache is not gathered by the GUI
            // Doctor worker yet; a later typed consumer will supply it.
            verified_identity: Gathered::NotLoaded(
                "not gathered: the GUI Doctor worker does not load the identity cache yet",
            ),
            free_space_policy: FreeSpacePolicy::default(),
        };
        let scan = run_doctor_scan(&inputs);
        let finished_at_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        // Keep the selected finding only if it still exists.
        if let Some(selected) = &self.doctor_selected_finding
            && scan
                .finding(doctor_page::doctor_finding_key_id(selected))
                .is_none()
        {
            self.doctor_selected_finding = None;
        }
        self.doctor_scan = DoctorScanState::Ready(Box::new(DoctorScanOutcome {
            scan,
            finished_at_unix_seconds,
        }));
    }

    /// Opens the confirmation screen for a repair. Opening it changes
    /// nothing; only `confirm_doctor_repair` can execute.
    pub(crate) fn review_doctor_repair(&mut self, action: DoctorRepairAction, finding_id: String) {
        let Some(outcome) = self.doctor_scan.displayed() else {
            return;
        };
        // Ambiguous by id alone means the caller should have supplied the
        // resource (which `show_doctor_page` always does), so refuse to guess.
        let Some(finding) = outcome.scan.finding_for(&finding_id, None).found() else {
            return;
        };
        self.doctor_repair_review = Some(DoctorRepairReview {
            action,
            finding_id,
            affected: finding.affected.as_ref().map(|path| path.display.clone()),
            finding_title: finding.title.clone(),
            evidence: finding.evidence.clone(),
        });
        self.doctor_repair_result = None;
    }

    /// Same as [`Self::review_doctor_repair`], for a finding identified by
    /// both its id and its exact resource.
    pub(crate) fn review_doctor_repair_for(
        &mut self,
        action: DoctorRepairAction,
        finding_id: String,
        affected: String,
    ) {
        let Some(outcome) = self.doctor_scan.displayed() else {
            return;
        };
        // The resource comes from a finding this scan reproduced, and is
        // re-matched against it here - it can only select, never introduce.
        let Some(finding) = outcome
            .scan
            .finding_for(&finding_id, Some(affected.as_str()))
            .found()
        else {
            return;
        };
        self.doctor_repair_review = Some(DoctorRepairReview {
            action,
            finding_id,
            affected: Some(affected),
            finding_title: finding.title.clone(),
            evidence: finding.evidence.clone(),
        });
        self.doctor_repair_result = None;
    }

    pub(crate) fn cancel_doctor_repair(&mut self) {
        // Cancelling is purely a state reset. Nothing was executed, so there
        // is nothing to undo.
        self.doctor_repair_review = None;
    }

    pub(crate) fn show_repair_review_page(&mut self, ui: &mut egui::Ui) {
        let page = self
            .repair_review_page
            .get_or_insert_with(repair_review_page::RepairReviewPageState::default);
        // Drained before the view is built, so a running apply or scan keeps
        // repainting (progress/result reach the screen promptly) without the
        // page needing its own render loop.
        if page.poll_apply() || page.is_apply_running() {
            ui.ctx().request_repaint();
        }
        if page.poll_scan() || page.is_scan_running() {
            ui.ctx().request_repaint();
        }
        repair_review_page::show_repair_review_page(ui, page);
    }

    pub(crate) fn show_repair_history_page(&mut self, ui: &mut egui::Ui) {
        let page = self
            .repair_history_page
            .get_or_insert_with(repair_history_page::RepairHistoryPageState::load);
        // Drained before the view is built, so a running undo keeps
        // repainting until it settles.
        if page.poll_undo() || page.is_undo_running() {
            ui.ctx().request_repaint();
        }
        repair_history_page::show_repair_history_page(ui, page, &mut self.clipboard);
    }

    pub(crate) fn show_exact_duplicate_review_page(&mut self, ui: &mut egui::Ui) {
        let page = self.exact_duplicate_review_page.get_or_insert_with(
            exact_duplicate_review_page::ExactDuplicateReviewPageState::default,
        );
        if page.poll_scan() || page.is_scan_running() {
            ui.ctx().request_repaint();
        }
        exact_duplicate_review_page::show_exact_duplicate_review_page(ui, page);
    }

    /// Renders the Doctor page over the shared `doctor_scan` state and
    /// dispatches whatever action it returned. Shared verbatim by the
    /// Problems & Repair -> Diagnostics tab and the dedicated
    /// `MainView::EmulatorSetup` destination, so the two can never diverge in
    /// behaviour or state (there is exactly one `doctor_scan`, one scan
    /// engine, one repair-review flow).
    pub(crate) fn show_doctor_page_body(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let action = doctor_page::show_doctor_page(
            ui,
            &self.doctor_scan,
            &mut self.doctor_selected_finding,
            self.doctor_repair_review.as_ref(),
            self.doctor_repair_result.as_deref(),
            self.doctor_repair_finished_at_unix_seconds,
            &mut self.clipboard,
            self.ui_mode == GuiMode::GamerView,
        );
        match action {
            // Never `self.refresh(context)`: Doctor must not reload the
            // application or rescan the library.
            Some(doctor_page::DoctorPageAction::RunScan) => {
                self.start_doctor_scan(context.clone());
            }
            Some(doctor_page::DoctorPageAction::ReviewRepair {
                action,
                finding_id,
                affected,
            }) => match affected {
                Some(affected) => {
                    self.review_doctor_repair_for(action, finding_id, affected);
                }
                None => self.review_doctor_repair(action, finding_id),
            },
            Some(doctor_page::DoctorPageAction::ConfirmRepair) => {
                self.confirm_doctor_repair();
            }
            Some(doctor_page::DoctorPageAction::CancelRepair) => {
                self.cancel_doctor_repair();
            }
            None => {}
        }
    }

    /// The consolidated "Problems & Repair" destination - see
    /// `problems_repair_page`'s module doc. Renders the shared tab chrome,
    /// then dispatches to whichever tab `self.problems_repair_tab` currently
    /// names. Each arm calls exactly the same rendering this destination
    /// used before consolidation (`doctor_page::show_doctor_page`,
    /// `self.show_repair_review_page`, `self.show_repair_history_page`) -
    /// nothing here re-implements diagnosis or repair.
    pub(crate) fn show_problems_repair_page(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if let Some(tab) =
            problems_repair_page::show_problems_repair_tabs(ui, self.problems_repair_tab)
        {
            self.navigate_to_problems_repair_tab(tab);
        }
        match self.problems_repair_tab {
            ProblemsRepairTab::Overview => {
                if let Some(tab) =
                    problems_repair_page::show_problems_repair_overview(ui, &self.doctor_scan)
                {
                    self.navigate_to_problems_repair_tab(tab);
                }
            }
            ProblemsRepairTab::Diagnostics => {
                let stale_review_clicked =
                    problems_repair_page::show_stale_library_review_entry(ui, &self.doctor_scan);
                self.show_doctor_page_body(ui, context);
                if stale_review_clicked {
                    self.navigate_to_missing_catalogue_review();
                }
            }
            ProblemsRepairTab::Repair => {
                // Review and History are rendered together rather than as a
                // further sub-tab level: both already lazily load their own
                // state regardless of which of `RepairReview`/`RepairHistory`
                // is the current `self.view`, so showing both keeps every
                // existing deep-link (either MainView value) landing on
                // visible, correct content without inventing a third tab
                // layer this task's UX sketch does not ask for.
                //
                // Duplicate Finder and Disc Conversion used to be reached
                // through secondary buttons here; since 0.8.1's "core
                // workflows directly discoverable" pass they are first-class
                // destinations (`MainView::ExactDuplicateReview` /
                // `MainView::DiscConversion`) with their own sidebar and
                // top-menu entries, so this tab is now purely repair-plan
                // review + history. A quiet cross-link is kept for the user
                // who is already here.
                ui.horizontal(|ui| {
                    if widgets::action_button(
                        ui,
                        "Open Duplicate Finder",
                        widgets::ActionStyle::Secondary,
                        true,
                    )
                    .on_hover_text(
                        "Find identical or equivalent copies and move extras into a recoverable \
                         quarantine. Opens the dedicated Duplicate Finder page.",
                    )
                    .clicked()
                    {
                        self.navigate_to_main_view(MainView::ExactDuplicateReview);
                    }
                    if widgets::action_button(
                        ui,
                        "Open Disc Conversion",
                        widgets::ActionStyle::Secondary,
                        true,
                    )
                    .on_hover_text(
                        "Convert a supported CUE/BIN source to a fingerprint-verified CHD. Opens \
                         the dedicated Disc Conversion page.",
                    )
                    .clicked()
                    {
                        self.navigate_to_main_view(MainView::DiscConversion);
                    }
                });
                ui.add_space(theme::SECTION_GAP);

                self.show_repair_review_page(ui);
                ui.add_space(theme::SECTION_GAP);
                ui.separator();
                ui.add_space(theme::SECTION_GAP);
                self.show_repair_history_page(ui);
            }
        }
    }

    pub(crate) fn confirm_doctor_repair(&mut self) {
        let config = match Config::load_default() {
            Ok(config) => config,
            Err(error) => {
                self.doctor_repair_review.take();
                self.history.record(HistoryEntry::new(
                    ActivityAction::DoctorRepair,
                    None,
                    ActivityOutcome::Failed,
                    format!("Doctor repair could not start: {error}"),
                ));
                return;
            }
        };
        let index_path = match default_index_path() {
            Ok(path) => path,
            Err(error) => {
                self.doctor_repair_review.take();
                self.history.record(HistoryEntry::new(
                    ActivityAction::DoctorRepair,
                    None,
                    ActivityOutcome::Failed,
                    format!("Doctor repair could not start: {error}"),
                ));
                return;
            }
        };
        self.confirm_doctor_repair_with(config, index_path);
    }

    /// The repair itself, against an already-resolved configuration.
    ///
    /// `confirm_doctor_repair` reads the per-user configuration and the index
    /// path and delegates here. Tests supply both directly: reading them meant a
    /// test of *refusal* first had to get past a config load, so it passed only
    /// on a machine that happened to have `~/.config/archivefs/config.toml` and
    /// failed on CI, which does not.
    pub(crate) fn confirm_doctor_repair_with(&mut self, config: Config, index_path: PathBuf) {
        let Some(review) = self.doctor_repair_review.take() else {
            return;
        };
        let Some(displayed) = self.doctor_scan.displayed() else {
            return;
        };
        let request = DoctorRepairRequest {
            action: review.action,
            finding_id: review.finding_id.clone(),
            affected: review.affected.clone(),
            confirmed: true,
            dry_run: false,
        };
        let outcome = execute_doctor_repair(
            &request,
            &DoctorRepairContext {
                config: &config,
                scan: &displayed.scan,
                index_path: &index_path,
            },
        );

        // One History entry per attempt, success or not.
        self.history.record(HistoryEntry::new(
            ActivityAction::DoctorRepair,
            outcome
                .record
                .affected
                .as_ref()
                .map(|path| PathBuf::from(&path.display)),
            match outcome.record.status {
                DoctorRepairStatus::Succeeded => match outcome.record.verification {
                    DoctorRepairVerification::Verified => ActivityOutcome::Completed,
                    _ => ActivityOutcome::Skipped,
                },
                DoctorRepairStatus::DryRun => ActivityOutcome::Offered,
                DoctorRepairStatus::Rejected => ActivityOutcome::Rejected,
                DoctorRepairStatus::Failed => ActivityOutcome::Failed,
            },
            doctor_page::doctor_repair_history_detail(&outcome),
        ));
        self.doctor_repair_finished_at_unix_seconds = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0),
        );
        // Update only the affected finding, never the whole scan: the repair
        // verified (or did not verify) exactly one check, and unrelated
        // findings must be preserved.
        if outcome.record.status == DoctorRepairStatus::Succeeded
            && outcome.record.verification == DoctorRepairVerification::Verified
            && let DoctorScanState::Ready(displayed) = &mut self.doctor_scan
        {
            let finding_id = outcome.record.finding_id.clone();
            let affected = outcome
                .record
                .affected
                .as_ref()
                .map(|path| path.display.clone());
            displayed.scan.findings.retain(|finding| {
                finding.id != finding_id
                    || finding.affected.as_ref().map(|path| path.display.clone()) != affected
            });
        }
        if self
            .doctor_selected_finding
            .as_deref()
            .map(doctor_page::doctor_finding_key_id)
            == Some(outcome.record.finding_id.as_str())
        {
            self.doctor_selected_finding = None;
        }
        self.doctor_repair_result = Some(Box::new(outcome));
    }


}

/// Collects the path-based Doctor inputs. Runs on a worker thread.
///
/// Every call here is read-only by the callee's own documented contract:
/// `assess_mount_root_safety` wraps `validate_destination_root` (which
/// "never creates a directory or file"), `diagnose_database` performs no
/// migration/recovery/checkpoint, `list_source_folder_views_default` reads
/// config plus the catalogue read-only, and `discover_shared_apply_history`
/// only reads existing journal files.
///
/// Nothing here scans archives, mounts anything, or writes. In particular
/// `run_setup_diagnostics` is deliberately **not** called: its "Mount root
/// is writable" check probes by creating and removing a file, which changes
/// the mount root's modification time. Doctor borrows the already-computed
/// `SetupDiagnostics` from `self.diagnostics` instead, so opening Doctor
/// never performs that write.
///
/// The one child process started from here is `arcade_version_probe`'s
/// bounded, no-shell `mame -version` query (output- and timeout-capped, no
/// config written). It is a diagnostic probe, not an emulator launch - see
/// that module's docs.
pub(crate) fn gather_doctor_inputs() -> DoctorGathered {
    let config = Config::load_default();

    let transactions = match default_shared_history_root() {
        Ok(root) => Gathered::Ready(discover_shared_apply_history(&root)),
        Err(error) => Gathered::Failed(format!(
            "install history root is unavailable: {}",
            error.detail
        )),
    };
    // Emulator profiles first: where they live decides which filesystems and
    // which managed files matter. Xenia has no documented native path, so it
    // is never guessed at.
    let mount_table = mount_table();
    let discovered = DiscoveredProfiles::from_environment(Vec::new());
    let profile_report = assess_emulator_profiles(&discovered.borrowed(), mount_table.as_deref());
    let managed_targets = managed_scan_targets(&profile_report);
    // Launch readiness (native executable binding, plus - xemu only - the
    // four required system files) is a distinct question from the
    // writability assessment above, so it is gathered separately - see
    // `diagnostics::profiles`'s own "xemu / Xenia launch readiness" module
    // doc section.
    let xemu_readiness = match &discovered.xemu {
        Ok(discovery) => Gathered::Ready(assess_xemu_readiness(Some(discovery))),
        Err(error) => Gathered::Failed(error.clone()),
    };
    let xenia_readiness = Gathered::Ready(assess_xenia_readiness(discovered.xenia.as_ref()));
    let ppsspp_readiness = match &discovered.ppsspp {
        Ok(discovery) => Gathered::Ready(assess_ppsspp_readiness(Some(discovery))),
        Err(error) => Gathered::Failed(error.clone()),
    };
    let rpcs3_readiness = match &discovered.rpcs3 {
        Ok(discovery) => Gathered::Ready(assess_rpcs3_readiness(Some(discovery))),
        Err(error) => Gathered::Failed(error.clone()),
    };
    let installations = discover_linux_emulator_installations();
    // Advisory arcade emulator / DAT version compatibility, now on live inputs:
    //
    // - DAT revision: the arcade `<version>` headers persisted on each DAT
    //   source's health record the last time it was validated. Read from the
    //   already-saved registry; no DAT file is reopened here
    //   (`arcade_dat_catalogues_from_source_health` is pure).
    // - Emulator version: a bounded, no-shell `mame -version` probe
    //   (`arcade_version_probe`) with output and timeout caps - a diagnostic
    //   query, never a normal launch. FinalBurn Neo is not probed, so it stays
    //   "detected, version unknown" unless a version is supplied another way.
    //
    // Both sides fail soft to "unknown"; a version difference is only ever an
    // Info finding and never changes ROM-set completeness.
    let arcade_dat_catalogues = archivefs_core::dat::sources::load_dat_sources_config_default()
        .ok()
        .map(|config| {
            let (registry, _warnings) =
                archivefs_core::dat::sources::DatSourceRegistry::from_config(&config);
            archivefs_core::diagnostics::arcade_dat_version::arcade_dat_catalogues_from_source_health(
                registry
                    .entries()
                    .iter()
                    .flat_map(|entry| entry.health.arcade_catalogue_revisions.iter()),
            )
        })
        .unwrap_or_default();
    let arcade_version_outputs =
        archivefs_core::diagnostics::arcade_version_probe::probe_arcade_emulator_versions(
            &installations,
        );
    let arcade_dat_version = Gathered::Ready(
        archivefs_core::diagnostics::arcade_dat_version::arcade_dat_version_readiness(
            &installations,
            &arcade_dat_catalogues,
            &arcade_version_outputs,
        ),
    );
    let linux_emulator_installations = Gathered::Ready(installations);

    DoctorGathered {
        mount_root_safety: match &config {
            Ok(config) => Gathered::Ready(assess_mount_root_safety(&config.mount_root)),
            Err(error) => Gathered::Failed(format!("configuration could not be read: {error}")),
        },
        // Read-only: walks only EmuWiz's own mount root, never an archive,
        // and shares its removability predicate with the remover.
        stale_mount_directories: match &config {
            Ok(config) => match plan_stale_mount_directories(config) {
                Ok(stale) => Gathered::Ready(stale),
                Err(error) => {
                    Gathered::Failed(format!("the mount root could not be inspected: {error}"))
                }
            },
            Err(error) => Gathered::Failed(format!("configuration could not be read: {error}")),
        },
        index_freshness: match default_index_path() {
            Ok(path) => match read_archive_index(&path) {
                Ok(index) => Gathered::Ready((check_archive_index_freshness(&index), path)),
                // No index yet is not a failure - there is simply nothing to
                // report about its freshness.
                Err(_) => Gathered::NotLoaded(
                    "No archive index has been built yet, so its freshness was not checked.",
                ),
            },
            Err(error) => Gathered::Failed(format!("index path could not be resolved: {error}")),
        },
        database: match default_database_path() {
            Ok(path) => Gathered::Ready(diagnose_database(&path)),
            Err(error) => Gathered::Failed(format!("database path could not be resolved: {error}")),
        },
        source_health: match list_source_folder_views_default() {
            Ok(views) => Gathered::Ready(source_health_issues(views.as_slice())),
            Err(error) => Gathered::Failed(format!("source folders could not be listed: {error}")),
        },
        transactions: transactions.clone(),
        // Free space and mount state for every location EmuWiz depends on,
        // read from `statvfs` and `/proc/self/mountinfo`. No probe file.
        storage: Gathered::Ready(assess_storage(&storage_resources(
            config.as_ref().ok(),
            default_database_path().ok().as_deref(),
            default_index_path().ok().as_deref(),
            default_shared_history_root().ok().as_deref(),
            &profile_destination_directories(&profile_report),
        ))),
        emulator_profiles: Gathered::Ready(profile_report),
        linux_emulator_installations,
        arcade_dat_version,
        xemu_readiness,
        xenia_readiness,
        ppsspp_readiness,
        rpcs3_readiness,
        managed_entries: match &transactions {
            Gathered::Ready(history) => {
                Gathered::Ready(scan_managed_entries(history, &managed_targets))
            }
            Gathered::Failed(reason) => Gathered::Failed(reason.clone()),
            Gathered::NotLoaded(reason) => Gathered::NotLoaded(reason),
        },
    }
}

/// Borrows an owned gathered value as the runner's input, preserving the
/// unavailable/failed reason unchanged.
pub(crate) fn borrowed<'a, T, B: ?Sized>(
    gathered: &'a Gathered<T>,
    borrow: impl FnOnce(&'a T) -> &'a B,
) -> Gathered<&'a B> {
    match gathered {
        Gathered::Ready(value) => Gathered::Ready(borrow(value)),
        Gathered::Failed(reason) => Gathered::Failed(reason.clone()),
        Gathered::NotLoaded(reason) => Gathered::NotLoaded(reason),
    }
}
