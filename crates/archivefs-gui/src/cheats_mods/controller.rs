use crate::*;

impl ArchiveFsApp {
    pub(crate) fn show_cheat_sources_page(&mut self, context: &egui::Context, ui: &mut egui::Ui) {
        if self.cheat_sources_page.is_none() {
            match archivefs_core::patch_manager::default_cheat_sources_config_path() {
                Ok(path) => {
                    self.cheat_sources_page =
                        Some(cheat_sources_page::CheatSourcesPageState::load(
                            path,
                            archivefs_core::patch_manager::default_cheat_source_data_root(),
                        ));
                }
                Err(error) => {
                    widgets::banner(
                        ui,
                        "Preferences location unknown",
                        &format!(
                            "{error}. Cheat source preferences cannot be read or saved without a \
                             home directory."
                        ),
                        widgets::StatusTone::Blocked,
                    );
                    return;
                }
            }
        }

        let Some(page) = self.cheat_sources_page.as_mut() else {
            return;
        };
        let view = page.view();
        let action = cheat_sources_page::show_cheat_sources_page(
            ui,
            &view,
            &mut self.cheat_sources_ui,
            self.ui_mode == GuiMode::GamerView,
        );
        if let Some(action) = action {
            // Reverting throws away in-progress text and any open picker too:
            // leaving a typed priority behind after "Discard changes" would
            // show a value that is no longer anywhere in the state.
            if matches!(action, cheat_sources_page::CheatSourcesPageAction::Revert) {
                self.cheat_sources_ui.clear();
            }
            page.apply(action);
        }

        // The BSFree Archive card: the same Download/Import/Validate/Enable/
        // Remove controls the Sources page already offers, reusing the exact
        // `BsFreeOperation` plumbing (`start_bsfree_operation`,
        // `sources_page::show_bsfree_source_card`). Cheats & Mods tells the
        // user BSFree is managed from the Cheat Sources page, so the real
        // controls must actually live here rather than only on the generic
        // Sources page.
        ui.add_space(theme::SECTION_GAP);
        if let Some(operation) = sources_page::show_bsfree_source_card(
            ui,
            &self.bsfree_manager,
            self.bsfree_operation.is_some(),
            &mut self.bsfree_ui,
            &mut self.clipboard,
        ) {
            self.start_bsfree_operation(context.clone(), operation);
        }
    }

    pub(crate) fn start_bsfree_operation(&mut self, context: egui::Context, operation: BsFreeOperation) {
        if self.bsfree_operation.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.bsfree_operation = Some(RunningBsFreeOperation {
            operation: operation.clone(),
            receiver,
        });
        thread::spawn(move || {
            let result = run_bsfree_operation(&operation).map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_bsfree_operation(&mut self, context: &egui::Context) {
        let result = self.bsfree_operation.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.operation.clone(), result))
        });
        let Some((operation, result)) = result else {
            return;
        };
        self.bsfree_operation = None;
        match result {
            Ok(BsFreeOperationResult::Status(status)) => {
                self.bsfree_manager = BsFreeManagerState::Ready(status);
            }
            Ok(BsFreeOperationResult::Removed) => {
                self.bsfree_manager = BsFreeManagerState::NotLoaded;
                self.bsfree_ui = BsFreeGuiState::default();
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message: "Removed EmuWiz's local BSFree source copy only.".to_string(),
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
            }
            Ok(BsFreeOperationResult::Search(result)) => {
                self.bsfree_ui.search_result = Some(Ok(result));
                self.bsfree_ui.selected_game = None;
                self.bsfree_ui.cheats = None;
            }
            Ok(BsFreeOperationResult::Systems(page)) => {
                self.bsfree_ui.platforms = Some(Ok(page.rows));
            }
            Ok(BsFreeOperationResult::Game(game, cheats)) => {
                self.bsfree_ui.selected_game = Some(game);
                self.bsfree_ui.cheats = Some(Ok(cheats));
            }
            Err(message) => match operation {
                BsFreeOperation::LoadSystems => self.bsfree_ui.platforms = Some(Err(message)),
                BsFreeOperation::Search(_) => self.bsfree_ui.search_result = Some(Err(message)),
                BsFreeOperation::LoadGame { .. } => self.bsfree_ui.cheats = Some(Err(message)),
                BsFreeOperation::LoadStatus => {
                    self.bsfree_manager = BsFreeManagerState::Failed(message)
                }
                _ => {
                    self.feedback = Some(ActionFeedback {
                        succeeded: false,
                        message,
                        cleanup: None,
                        warning: None,
                        more_information: None,
                    });
                    self.start_bsfree_operation(context.clone(), BsFreeOperation::LoadStatus);
                }
            },
        }
    }

    pub(crate) fn start_dolphin_catalogue_status_load(&mut self, context: egui::Context) {
        if matches!(
            self.dolphin_catalogue_manager,
            DolphinCatalogueManagerState::Loading(_)
        ) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.dolphin_catalogue_manager = DolphinCatalogueManagerState::Loading(receiver);
        thread::spawn(move || {
            let result = default_dolphin_catalogue_cache_root().and_then(|root| {
                let catalogue = match load_dolphin_catalogue(&root)? {
                    DolphinCatalogueLoad::NotInstalled => None,
                    DolphinCatalogueLoad::Ready(catalogue) => Some(*catalogue),
                };
                let last_check_unix_seconds =
                    load_dolphin_catalogue_update_state(&root)?.last_check_unix_seconds;
                Ok(DolphinCatalogueStatusSnapshot {
                    catalogue,
                    last_check_unix_seconds,
                })
            });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn start_dolphin_catalogue_retrieval(&mut self, context: egui::Context) {
        if self.dolphin_catalogue_retrieval.is_some() {
            return;
        }
        let Some(kind) = self.dolphin_catalogue_review.take() else {
            return;
        };
        self.dolphin_catalogue_generation = self.dolphin_catalogue_generation.wrapping_add(1);
        let generation = self.dolphin_catalogue_generation;
        let cancellation = CheatSourceCancellation::default();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        let (progress_sender, progress_receiver) = mpsc::channel();
        self.history.record(HistoryEntry::new(
            ActivityAction::DolphinCatalogueRetrieval,
            None,
            ActivityOutcome::Started,
            format!(
                "Dolphin cheat catalogue {} started.",
                dolphin_catalogue_retrieval_kind_verb(kind)
            ),
        ));
        self.dolphin_catalogue_retrieval = Some(RunningDolphinCatalogueRetrieval {
            generation,
            kind,
            cancellation,
            receiver,
            progress_receiver,
            progress: None,
            cancellation_requested: false,
        });
        let progress_context = context.clone();
        let progress = CheatSourceProgressReporter::new(move |event| {
            let _ = progress_sender.send(event);
            progress_context.request_repaint();
        });
        thread::spawn(move || {
            let result = default_dolphin_catalogue_cache_root().and_then(|cache_root| {
                let options = DolphinCatalogueFetchOptions {
                    cache_root,
                    cancellation: Some(worker_cancellation),
                    progress: Some(progress),
                };
                let transport = HttpsCheatSourceTransport::new();
                match kind {
                    DolphinCatalogueRetrievalKind::Download
                    | DolphinCatalogueRetrievalKind::Update => {
                        fetch_dolphin_catalogue_with_transport(&options, &transport)
                    }
                    DolphinCatalogueRetrievalKind::Rebuild => {
                        rebuild_dolphin_catalogue_index_with_transport(&options, &transport)
                    }
                }
            });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn start_dolphin_catalogue_update_check(&mut self, context: egui::Context) {
        if self.dolphin_catalogue_update_check.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.dolphin_catalogue_update_check = Some(receiver);
        thread::spawn(move || {
            let result = default_dolphin_catalogue_cache_root().and_then(|root| {
                check_dolphin_catalogue_update_with_transport(
                    &root,
                    &HttpsCheatSourceTransport::new(),
                )
            });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// The single dispatch point for `DolphinCatalogueManagerAction` -
    /// mirrors `handle_catalogue_manager_action`'s Review-then-Confirm
    /// two-step and "no automatic network access" guarantee.
    pub(crate) fn handle_dolphin_catalogue_manager_action(
        &mut self,
        context: &egui::Context,
        action: DolphinCatalogueManagerAction,
    ) {
        match action {
            DolphinCatalogueManagerAction::Refresh => {
                self.start_dolphin_catalogue_status_load(context.clone());
            }
            DolphinCatalogueManagerAction::Review(kind) => {
                self.dolphin_catalogue_review = Some(kind);
            }
            DolphinCatalogueManagerAction::Confirm => {
                self.start_dolphin_catalogue_retrieval(context.clone());
            }
            DolphinCatalogueManagerAction::CancelReview => {
                self.dolphin_catalogue_review = None;
            }
            DolphinCatalogueManagerAction::CancelRunning => {
                if let Some(running) = self.dolphin_catalogue_retrieval.as_mut() {
                    running.cancellation.cancel();
                    running.cancellation_requested = true;
                }
            }
            DolphinCatalogueManagerAction::CheckForUpdates => {
                self.start_dolphin_catalogue_update_check(context.clone());
            }
            DolphinCatalogueManagerAction::RequestRemove => {
                self.dolphin_catalogue_remove_confirm = true;
            }
            DolphinCatalogueManagerAction::CancelRemove => {
                self.dolphin_catalogue_remove_confirm = false;
            }
            DolphinCatalogueManagerAction::ConfirmRemove => {
                self.dolphin_catalogue_remove_confirm = false;
                let outcome = default_dolphin_catalogue_cache_root()
                    .and_then(|root| remove_dolphin_catalogue(&root));
                self.history.record(HistoryEntry::new(
                    ActivityAction::DolphinCatalogueRetrieval,
                    None,
                    match &outcome {
                        Ok(()) => ActivityOutcome::Completed,
                        Err(_) => ActivityOutcome::Failed,
                    },
                    match &outcome {
                        Ok(()) => {
                            "Dolphin cheat catalogue removed. Installed Dolphin codes and profiles were not touched."
                                .to_string()
                        }
                        Err(error) => format!("Dolphin cheat catalogue removal failed: {error}"),
                    },
                ));
                self.dolphin_catalogue_update_available = None;
                self.start_dolphin_catalogue_status_load(context.clone());
            }
        }
    }

    pub(crate) fn poll_dolphin_catalogue_manager(&mut self, context: &egui::Context) {
        if let DolphinCatalogueManagerState::Loading(receiver) = &self.dolphin_catalogue_manager {
            match receiver.try_recv() {
                Ok(Ok(snapshot)) => {
                    self.dolphin_catalogue_manager =
                        DolphinCatalogueManagerState::Ready(Box::new(snapshot));
                }
                Ok(Err(error)) => {
                    self.dolphin_catalogue_manager = DolphinCatalogueManagerState::Failed(error);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.dolphin_catalogue_manager =
                        DolphinCatalogueManagerState::Failed(DolphinCatalogueError {
                            kind: DolphinCatalogueErrorKind::CacheUnavailable,
                            detail: "catalogue status worker stopped unexpectedly".to_string(),
                        });
                }
            }
        }
        if let Some(receiver) = &self.dolphin_catalogue_update_check {
            match receiver.try_recv() {
                Ok(result) => {
                    self.dolphin_catalogue_update_available =
                        Some(result.as_ref().is_ok_and(|check| check.update_available));
                    self.dolphin_catalogue_update_check = None;
                    self.start_dolphin_catalogue_status_load(context.clone());
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.dolphin_catalogue_update_available = Some(false);
                    self.dolphin_catalogue_update_check = None;
                }
            }
        }
        if let Some(running) = self.dolphin_catalogue_retrieval.as_mut() {
            for progress in running.progress_receiver.try_iter() {
                running.progress = Some(progress);
            }
        }
        let result = self
            .dolphin_catalogue_retrieval
            .as_ref()
            .and_then(|running| {
                running
                    .receiver
                    .try_recv()
                    .ok()
                    .map(|result| (running.generation, result))
            });
        let Some((generation, result)) = result else {
            return;
        };
        self.dolphin_catalogue_retrieval = None;
        if generation != self.dolphin_catalogue_generation {
            return;
        }
        match &result {
            Ok(fetch) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::DolphinCatalogueRetrieval,
                    None,
                    ActivityOutcome::Completed,
                    format!(
                        "Dolphin cheat catalogue activated at commit {}: {} games, {} usable Gecko codes ({} GameSettings files inspected, {} skipped).",
                        fetch.catalogue.metadata.resolved_commit,
                        fetch.catalogue.games.len(),
                        fetch.catalogue.metadata.total_usable_gecko_entries,
                        fetch.catalogue.metadata.game_settings_files_inspected,
                        fetch.catalogue.metadata.malformed_or_skipped_files
                    ),
                ));
                // A freshly downloaded/updated catalogue may now answer a
                // lookup that previously came up empty; let the next
                // selection re-check local sources instead of keeping a
                // stale "nothing found" verdict pinned from before this
                // catalogue existed.
                if let Some(workflow) = self.cheat_workflow.as_mut()
                    && workflow.adapter == CheatEmulatorAdapter::Dolphin
                    && matches!(workflow.dolphin_provider, CheatStepResource::NotLoaded)
                {
                    workflow.dolphin_local_lookup = DolphinLocalLookupState::NotAttempted;
                }
            }
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::DolphinCatalogueRetrieval,
                    None,
                    if error.kind == DolphinCatalogueErrorKind::Cancelled {
                        ActivityOutcome::Skipped
                    } else {
                        ActivityOutcome::Failed
                    },
                    format!("Dolphin cheat catalogue retrieval failed: {error}. Existing catalogue, if any, retained."),
                ));
            }
        }
        self.dolphin_catalogue_last_result = Some(result);
        self.dolphin_catalogue_update_available = None;
        self.dolphin_catalogue_manager = DolphinCatalogueManagerState::NotLoaded;
        self.start_dolphin_catalogue_status_load(context.clone());
    }

    /// Selects the full-page Cheats & Mods workspace context without
    /// starting I/O. Returning to the same exact archive preserves the
    /// existing workflow; changing archive replaces it wholesale. The
    /// return value says whether a new source-list load is needed.
    pub(crate) fn prepare_cheats_mods_workspace(&mut self, archive_path: PathBuf) -> bool {
        // Phase 5 fix: this used to only set `self.view`, which Gamer
        // View's own render branch never reads (chosen purely from
        // `ui_mode` - see the Phase 4 fix to `start_cheat_install_
        // rollback` for the identical bug shape). Clicking "Cheats &
        // Mods" from Gamer View therefore changed internal state with
        // nothing visible happening on screen at all - the design doc's
        // own stated centerpiece workflow (§2.5) was silently
        // unreachable from the screen it names as its entry point. This
        // page is deliberately the *existing* full workflow, unsimplified
        // (§2.1: "opens the existing 5-area workflow page... no
        // independent archive picker"), so switching modes here is
        // correct, not a compromise - the doc's own design already
        // expects this transition.
        self.ui_mode = GuiMode::AdvancedView;
        save_gui_mode(self.ui_mode);
        self.view = MainView::CheatsMods;
        self.tools_overlay = ToolsOverlay::None;
        if self
            .cheat_workflow
            .as_ref()
            .is_some_and(|workflow| workflow.archive_path == archive_path)
        {
            return false;
        }
        if let Some(cancellation) = self
            .cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.gamecube_gamehacking_cancellation.as_ref())
        {
            cancellation.store(true, Ordering::Relaxed);
        }
        let (
            source_mode,
            selected_source_id,
            fetch_force_refresh,
            previous_profile_id,
            previous_pcsx2_profile_id,
            previous_dolphin_profile_id,
            previous_xenia_profile_id,
        ) = self
            .cheat_workflow
            .as_ref()
            .map(|workflow| {
                (
                    workflow.source_mode,
                    workflow.selected_source_id.clone(),
                    workflow.fetch_force_refresh,
                    workflow.selected_profile_id.clone(),
                    workflow.selected_pcsx2_profile_id.clone(),
                    workflow.selected_dolphin_profile_id.clone(),
                    workflow.selected_xenia_profile_id.clone(),
                )
            })
            .unwrap_or((
                CheatSourceMode::ArchiveFsTrustedCatalogue,
                None,
                false,
                None,
                None,
                None,
                None,
            ));
        let record_details = match &self.state {
            LoadState::Ready(data) => Some(data.as_ref()),
            LoadState::Loading { previous, .. } => previous.as_deref(),
            LoadState::Error(_) => None,
        }
        .and_then(|data| {
            data.records
                .iter()
                .find(|record| record.mount_plan.archive.path == archive_path)
                .map(|record| {
                    (
                        record.identity.display_name.clone(),
                        record.identity.normalized_name.clone(),
                        record.identity.platform.clone(),
                        record.identity.region.clone(),
                        record.identity.source_root.clone(),
                        record.identity.size_bytes,
                    )
                })
        });
        let Some((display_name, normalized_name, platform, region, source_root, size_bytes)) =
            record_details
        else {
            self.cheat_workflow = None;
            return false;
        };
        let adapter = cheat_adapter_route(platform.as_deref());
        let persisted_identity = self
            .database_state
            .snapshot()
            .and_then(|snapshot| {
                snapshot
                    .archives
                    .iter()
                    .find(|archive| archive.absolute_path == archive_path)
            })
            .and_then(|archive| archive.identity_report.clone());
        let persisted_identity_request = persisted_identity.as_ref().map(|_| GameIdentityRequest {
            archive_path: archive_path.clone(),
            platform: platform.clone(),
            adapter,
        });
        let selected_profile_id = match &self.retroarch_profiles {
            RetroArchProfilesState::Ready(discovery) => {
                let eligible = eligible_profile_ids(discovery);
                if let Some(previous) =
                    previous_profile_id.filter(|previous| eligible.contains(&previous.as_str()))
                {
                    Some(previous)
                } else if eligible.len() == 1 {
                    Some(eligible[0].to_string())
                } else {
                    None
                }
            }
            _ => None,
        };
        let selected_pcsx2_profile_id = match &self.pcsx2_profiles {
            Pcsx2ProfilesState::Ready(discovery) => {
                let eligible = eligible_pcsx2_profile_ids(discovery);
                if let Some(previous) = previous_pcsx2_profile_id
                    .filter(|previous| eligible.contains(&previous.as_str()))
                {
                    Some(previous)
                } else if eligible.len() == 1 {
                    Some(eligible[0].to_string())
                } else {
                    None
                }
            }
            _ => None,
        };
        let (selected_dolphin_profile_id, dolphin_profile_selection) = match &self.dolphin_profiles
        {
            DolphinProfilesState::Ready(discovery) => {
                let selection =
                    select_dolphin_profile(discovery, previous_dolphin_profile_id.as_deref());
                let selected = match &selection {
                    EmulatorProfileSelection::Auto { profile_id, .. } => Some(profile_id.clone()),
                    EmulatorProfileSelection::NeedsChoice { .. }
                    | EmulatorProfileSelection::SetupNeeded => None,
                };
                (selected, Some(selection))
            }
            _ => (None, None),
        };
        let (selected_xenia_profile_id, xenia_profile_selection) = match &self.xenia_profiles {
            XeniaProfilesState::Ready(discovery) => {
                let candidates = xenia_profile_candidates(discovery);
                let selection = select_emulator_profile(
                    &candidates,
                    self.remembered_profile_id("xenia").as_deref(),
                    previous_xenia_profile_id.as_deref(),
                );
                let selected = match &selection {
                    EmulatorProfileSelection::Auto { profile_id, .. } => Some(profile_id.clone()),
                    EmulatorProfileSelection::NeedsChoice { .. }
                    | EmulatorProfileSelection::SetupNeeded => None,
                };
                (selected, Some(selection))
            }
            XeniaProfilesState::NotScanned => (None, None),
        };
        self.cheat_workflow = Some(CheatWorkflowState {
            archive_path,
            display_name,
            normalized_name,
            platform,
            region,
            source_root,
            size_bytes,
            adapter,
            identity_request: persisted_identity_request.clone(),
            identity: match (persisted_identity_request, persisted_identity) {
                (Some(request), Some(report)) => CheatStepResource::Ready((request, report)),
                _ => CheatStepResource::NotLoaded,
            },
            preview_request: None,
            preview: CheatStepResource::NotLoaded,
            transaction: CheatTransactionState::Idle,
            transaction_notice: None,
            selected_profile_id,
            selected_pcsx2_profile_id,
            pcsx2_inventory_profile_id: None,
            pcsx2_inventory: CheatStepResource::NotLoaded,
            pcsx2_activation: CheatActivationReadiness::Unknown,
            pcsx2_activation_receiver: None,
            pcsx2_gamehacking: CheatStepResource::NotLoaded,
            gamecube_gamehacking: CheatStepResource::NotLoaded,
            gamecube_gamehacking_request: None,
            gamecube_gamehacking_cancellation: None,
            gamecube_gamehacking_generation: 0,
            gamecube_gamehacking_blocked: false,
            browser_import: None,
            browser_import_open_error: None,
            bsfree_gamecube: CheatStepResource::NotLoaded,
            bsfree_gamecube_cancellation: None,
            bsfree_gamecube_generation: 0,
            bsfree_wii: CheatStepResource::NotLoaded,
            bsfree_wii_cancellation: None,
            bsfree_wii_generation: 0,
            selected_dolphin_profile_id,
            dolphin_explicit_root: String::new(),
            dolphin_inventory_profile_id: None,
            dolphin_inventory: CheatStepResource::NotLoaded,
            dolphin_activation: CheatActivationReadiness::Unknown,
            dolphin_activation_receiver: None,
            dolphin_provider_request: None,
            dolphin_provider: CheatStepResource::NotLoaded,
            dolphin_provider_selection: None,
            dolphin_destination_error: None,
            dolphin_local_lookup: DolphinLocalLookupState::NotAttempted,
            dolphin_profile_selection,
            // Remembered standard/install-only profiles are rediscovery hints,
            // not a hidden choice when more than one credible profile exists.
            dolphin_profile_choice: None,
            dolphin_details_open: false,
            dolphin_show_exact_changes: false,
            selected_xenia_profile_id,
            xenia_explicit_root: String::new(),
            xenia_provider_request: None,
            xenia_provider: CheatStepResource::NotLoaded,
            xenia_selected_candidate_index: None,
            xenia_selection: None,
            xenia_destination_error: None,
            xenia_profile_selection,
            xenia_profile_choice: None,
            xenia_details_open: false,
            xenia_show_exact_changes: false,
            source_mode,
            existing_library_profile_id: None,
            existing_library: CheatStepResource::NotLoaded,
            source_list: CheatStepResource::NotLoaded,
            source_fetch: CheatStepResource::NotLoaded,
            selected_source_id,
            fetch_force_refresh,
            candidates: CheatStepResource::NotLoaded,
            candidates_request: None,
            candidate_query: String::new(),
            candidate_selection: None,
            candidate_load_error: None,
        });
        true
    }

    /// Opens the full-page workspace and starts its read-only trusted
    /// source inventory only when the exact archive context is new.
    pub(crate) fn open_cheats_mods_workspace(&mut self, context: &egui::Context, archive_path: PathBuf) {
        if !self.prepare_cheats_mods_workspace(archive_path) {
            return;
        }
        // Read-only listing of the local trusted-source cache; safe to
        // start immediately (no network, background thread).
        self.start_cheat_source_list(context.clone());
        if self
            .cheat_workflow
            .as_ref()
            .is_some_and(|workflow| matches!(workflow.identity, CheatStepResource::NotLoaded))
        {
            self.start_game_identity_inspection(context.clone());
        }
        if self
            .cheat_workflow
            .as_ref()
            .is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Pcsx2)
            && matches!(
                self.pcsx2_profiles,
                Pcsx2ProfilesState::NotScanned | Pcsx2ProfilesState::Error(_)
            )
        {
            self.start_pcsx2_profile_scan(context.clone());
        }
        if self
            .cheat_workflow
            .as_ref()
            .is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Dolphin)
        {
            self.seed_explicit_root_from_remembered_profile("dolphin");
            if matches!(
                self.dolphin_profiles,
                DolphinProfilesState::NotScanned | DolphinProfilesState::Error(_)
            ) {
                self.start_dolphin_profile_scan(context.clone());
            }
        }
        if self
            .cheat_workflow
            .as_ref()
            .is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Xenia)
        {
            self.seed_explicit_root_from_remembered_profile("xenia");
            if matches!(self.xenia_profiles, XeniaProfilesState::NotScanned) {
                self.start_xenia_profile_scan();
            }
        }
    }

    /// Seeds the workflow's explicit-root text field from the remembered
    /// profile for `adapter`, but only when the field is still empty -
    /// never overwrites anything the user has already typed this
    /// session. This is what lets a remembered portable Dolphin install
    /// or a remembered Xenia Canary directory be rediscovered
    /// automatically without asking again, since neither adapter has a
    /// single standard path EmuWiz can otherwise find on its own.
    pub(crate) fn seed_explicit_root_from_remembered_profile(&mut self, adapter: &str) {
        let Some(root) = self.remembered_profile_root(adapter) else {
            return;
        };
        let Some(root) = root.to_str().map(str::to_string) else {
            return;
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        match adapter {
            "dolphin" if workflow.dolphin_explicit_root.trim().is_empty() => {
                workflow.dolphin_explicit_root = root;
            }
            "xenia" if workflow.xenia_explicit_root.trim().is_empty() => {
                workflow.xenia_explicit_root = root;
            }
            _ => {}
        }
    }

    pub(crate) fn start_game_identity_inspection(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if workflow.adapter == CheatEmulatorAdapter::Unsupported {
            return;
        }
        let request = GameIdentityRequest {
            archive_path: workflow.archive_path.clone(),
            platform: workflow.platform.clone(),
            adapter: workflow.adapter,
        };
        let path = request.archive_path.clone();
        let platform = request.platform.clone();
        let worker_request = request.clone();
        let (sender, receiver) = mpsc::channel();
        workflow.identity_request = Some(request);
        workflow.identity = CheatStepResource::Loading { receiver };
        workflow.preview_request = None;
        workflow.preview = CheatStepResource::NotLoaded;
        thread::spawn(move || {
            let report = inspect_catalogued_game_identity(&path, platform.as_deref());
            let _ = sender.send(Ok((worker_request, report)));
            context.request_repaint();
        });
    }

    pub(crate) fn start_cheat_preview(&mut self, context: egui::Context) {
        let Some((key, work)) = self.cheat_workflow.as_ref().and_then(|workflow| {
            build_cheat_preview_request(
                workflow,
                &self.retroarch_profiles,
                &self.pcsx2_profiles,
                &self.dolphin_profiles,
            )
        }) else {
            return;
        };
        if self.cheat_workflow.as_ref().is_some_and(|workflow| {
            workflow.preview_request.as_ref() == Some(&key)
                && !matches!(workflow.preview, CheatStepResource::NotLoaded)
        }) {
            return;
        }
        let archive_path = key.archive_path.clone();
        let worker_key = key.clone();
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Loading { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            Some(archive_path),
            ActivityOutcome::Started,
            "Read-only Cheats & Mods preview started.",
        ));
        thread::spawn(move || {
            let (outcome, materialized) = match work {
                CheatPreviewWork::Shared(request) => (
                    match build_shared_preview(&request) {
                        Ok(report) => CheatPreviewOutcome::Ready(report),
                        Err(error) => {
                            CheatPreviewOutcome::Failed(CheatPreviewFailure::Shared(error))
                        }
                    },
                    None,
                ),
                CheatPreviewWork::RetroArch(request) => {
                    match materialize_retroarch_shared_preview(&request) {
                        Ok(materialized) => (
                            CheatPreviewOutcome::Ready(materialized.preview.clone()),
                            Some(materialized),
                        ),
                        Err(error) => (
                            CheatPreviewOutcome::Failed(CheatPreviewFailure::Materialization(
                                error,
                            )),
                            None,
                        ),
                    }
                }
            };
            let _ = sender.send(Ok(CheatPreviewResponse {
                key: worker_key,
                outcome,
                materialized,
                generated: None,
                dolphin_generated: None,
                xenia_generated: None,
                pcsx2_generated: None,
                gamecube_gamehacking_generated: None,
                bsfree_gamecube_generated: None,

                bsfree_wii_generated: None,
            }));
            context.request_repaint();
        });
    }

    /// Stage 4: builds the ranked candidate list for the selected archive
    /// against the verified catalogue snapshot, off the UI thread.
    ///
    /// Bound to the same request key the preview uses, so a result that
    /// arrives after the archive, profile, or snapshot changed is discarded
    /// rather than shown against the wrong context.
    /// Stage 4's dispatch target - reached from the "Find matching cheat
    /// files" button. Every call produces exactly one immediately visible
    /// outcome: it starts a background match (`Loading`, then `Ready` or a
    /// worker `Failed`), or it sets an explained `Failed` state on the spot
    /// when a prerequisite is unmet (`CheatCandidatePrerequisite`) - never
    /// a silent no-op. A call while a match is already running is itself a
    /// no-op (guards against a double click restarting the work).
    pub(crate) fn start_cheat_candidate_match(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        if workflow.adapter != CheatEmulatorAdapter::RetroArch
            || workflow.source_mode != CheatSourceMode::ArchiveFsTrustedCatalogue
            || matches!(workflow.candidates, CheatStepResource::Loading { .. })
        {
            return;
        }
        let archive_path = workflow.archive_path.clone();
        let outcome = build_cheat_candidate_request(workflow, &self.retroarch_profiles);
        match outcome {
            Ok((key, catalogue_root, archive)) => {
                let worker_key = key.clone();
                let worker_root = catalogue_root.clone();
                let (sender, receiver) = mpsc::channel();
                let Some(workflow) = self.cheat_workflow.as_mut() else {
                    return;
                };
                workflow.candidates_request = Some(key);
                workflow.candidates = CheatStepResource::Loading { receiver };
                workflow.candidate_selection = None;
                workflow.candidate_load_error = None;
                workflow.preview = CheatStepResource::NotLoaded;
                workflow.preview_request = None;
                workflow.transaction = CheatTransactionState::Idle;
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Started,
                    "Matching the selected archive against the trusted cheat catalogue.",
                ));
                thread::spawn(move || {
                    let snapshot = load_cheat_catalogue_snapshot(
                        &HostReadOnlyFilesystem,
                        "trusted-catalogue",
                        &worker_root,
                    );
                    let list = build_cheat_candidates(
                        &snapshot,
                        &archive,
                        &CheatCandidateOptions::default(),
                    );
                    let _ = sender.send(Ok(CheatCandidateStage {
                        key: worker_key,
                        catalogue_root: worker_root,
                        list,
                    }));
                    context.request_repaint();
                });
            }
            Err(reason) => {
                let Some(workflow) = self.cheat_workflow.as_mut() else {
                    return;
                };
                workflow.candidates_request = None;
                workflow.candidates = CheatStepResource::Failed(format!(
                    "{CHEAT_MATCH_BLOCKED_PREFIX}{}",
                    reason.message()
                ));
                workflow.candidate_selection = None;
                workflow.candidate_load_error = None;
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Rejected,
                    format!("Matching blocked: {}", reason.message()),
                ));
            }
        }
    }

    /// Stage 5/6: opens one chosen candidate and builds its cheat picker.
    ///
    /// Deliberately synchronous: this is one bounded read of a single small
    /// file in direct response to a click, and doing it inline keeps the
    /// chosen candidate and its parsed cheats impossible to get out of step.
    pub(crate) fn apply_cheat_candidate_choice(&mut self, relative_path: &str) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let CheatStepResource::Ready(stage) = &workflow.candidates else {
            return;
        };
        let Some(candidate) = stage
            .list
            .candidates
            .iter()
            .find(|candidate| candidate.catalogue_relative_path == relative_path)
            .cloned()
        else {
            return;
        };
        if !candidate.manually_selectable {
            // The UI never offers this, but a candidate that can never be
            // installed must not become the selection through any path.
            return;
        }
        let catalogue_root = stage.catalogue_root.clone();
        let archive_path = workflow.archive_path.clone();
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
        match load_candidate_document(
            &catalogue_root,
            &candidate.catalogue_relative_path,
            candidate.source_file_hash.as_deref(),
        ) {
            Ok(loaded) => {
                let selection = CheatSelection::from_document(&loaded.document);
                let cheat_count = selection.entries.len();
                let blocked = selection.blocked_count();
                workflow.candidate_load_error = None;
                workflow.candidate_selection = Some(CheatCandidateSelection {
                    candidate,
                    loaded,
                    selection,
                });
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Completed,
                    format!(
                        "Candidate '{relative_path}' opened: {cheat_count} cheat(s), {blocked} unavailable."
                    ),
                ));
            }
            Err(error) => {
                workflow.candidate_selection = None;
                workflow.candidate_load_error = Some(error.detail.clone());
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    format!(
                        "Candidate '{relative_path}' could not be opened: {}",
                        error.detail
                    ),
                ));
            }
        }
    }

    /// Applies one Dolphin code picker edit and invalidates anything
    /// downstream, the same way `update_cheat_selection` does for RetroArch.
    pub(crate) fn update_dolphin_code_selection(
        &mut self,
        edit: impl FnOnce(&mut DolphinProviderCodeSelection),
    ) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let Some(state) = workflow.dolphin_provider_selection.as_mut() else {
            return;
        };
        edit(&mut state.selection);
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
    }

    pub(crate) fn start_dolphin_provider_fetch(&mut self, context: egui::Context, force_refresh: bool) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        if workflow.adapter != CheatEmulatorAdapter::Dolphin
            || !platform_is_gamecube(workflow.platform.as_deref())
            || matches!(workflow.dolphin_provider, CheatStepResource::Loading { .. })
        {
            return;
        }
        let Some(identity) = ready_game_identity(workflow) else {
            return;
        };
        let Some(game_id) = identity.verified_dolphin_game_id().map(str::to_string) else {
            return;
        };
        let Some(revision) = identity.verified_dolphin_revision() else {
            return;
        };
        let Some(region) = region_for_game_id(&game_id) else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let mut options = match GeckoProviderFetchOptions::with_default_cache(now) {
            Ok(options) => options,
            Err(error) => {
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.dolphin_provider = CheatStepResource::Failed(error.to_string());
                }
                return;
            }
        };
        options.force_refresh = force_refresh;
        let query = GeckoProviderQuery {
            game_id: game_id.clone(),
            region,
            revision,
        };
        let key = DolphinProviderRequestKey {
            archive_path: workflow.archive_path.clone(),
            game_id,
            revision,
        };
        let archive_path = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.dolphin_provider_request = Some(key);
        workflow.dolphin_provider = CheatStepResource::Loading { receiver };
        workflow.dolphin_provider_selection = None;
        workflow.dolphin_destination_error = None;
        workflow.preview_request = None;
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.transaction = CheatTransactionState::Idle;
        self.history.record(HistoryEntry::new(
            ActivityAction::DolphinGeckoCandidateMatch,
            Some(archive_path),
            ActivityOutcome::Started,
            if force_refresh {
                "Refreshing exact-ID Gecko definitions from Dolphin upstream."
            } else {
                "Loading exact-ID Gecko definitions from Dolphin upstream or its local cache."
            },
        ));
        thread::spawn(move || {
            let result =
                fetch_dolphin_upstream_gecko(&query, &options).map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// The synchronous, network-free counterpart to
    /// `start_dolphin_provider_fetch`: tries the local full catalogue
    /// first, then a validated cached single-game result. Never spawns a
    /// thread and never issues a network request - per the Dolphin cheat
    /// catalogue design, an explicit fetch (Details > Refresh) is the only
    /// way to reach the network once the local sources have nothing.
    /// Returns `true` if it populated `dolphin_provider`.
    pub(crate) fn try_resolve_dolphin_provider_from_local_sources(
        &mut self,
        dolphin_profile_paths: &HashMap<String, PathBuf>,
    ) -> bool {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return false;
        };
        if workflow.adapter != CheatEmulatorAdapter::Dolphin
            || !platform_is_gamecube(workflow.platform.as_deref())
            || !matches!(workflow.dolphin_provider, CheatStepResource::NotLoaded)
        {
            return false;
        }
        let Some(identity) = ready_game_identity(workflow) else {
            return false;
        };
        let Some(game_id) = identity.verified_dolphin_game_id().map(str::to_string) else {
            return false;
        };
        let Some(revision) = identity.verified_dolphin_revision() else {
            return false;
        };
        let Some(region) = region_for_game_id(&game_id) else {
            return false;
        };
        let (Ok(catalogue_root), Ok(provider_root)) = (
            default_dolphin_catalogue_cache_root(),
            default_gecko_provider_cache_root(),
        ) else {
            return false;
        };
        let outcome = resolve_dolphin_gecko_lookup(
            &catalogue_root,
            &provider_root,
            &game_id,
            &region,
            revision,
        );
        let (fetch, local_state) = match outcome {
            Ok(DolphinGeckoLookupResult::Found(result)) => (
                Some(GeckoProviderFetchResult {
                    result,
                    status: GeckoProviderFetchStatus::Catalogue,
                    refresh_error: None,
                }),
                DolphinLocalLookupState::NotAttempted,
            ),
            Ok(
                DolphinGeckoLookupResult::NoCatalogueInstalled {
                    cached: Some(result),
                }
                | DolphinGeckoLookupResult::NotInCatalogue {
                    cached: Some(result),
                }
                | DolphinGeckoLookupResult::RegionMismatch {
                    cached: Some(result),
                }
                | DolphinGeckoLookupResult::CatalogueEntryHasNoUsableCodes {
                    cached: Some(result),
                    ..
                },
            ) => (
                Some(GeckoProviderFetchResult {
                    result,
                    status: GeckoProviderFetchStatus::FreshCache,
                    refresh_error: None,
                }),
                DolphinLocalLookupState::NotAttempted,
            ),
            Ok(DolphinGeckoLookupResult::NoCatalogueInstalled { cached: None }) => {
                (None, DolphinLocalLookupState::NoCatalogueInstalled)
            }
            Ok(DolphinGeckoLookupResult::NotInCatalogue { cached: None }) => {
                (None, DolphinLocalLookupState::NotInCatalogue)
            }
            Ok(DolphinGeckoLookupResult::RegionMismatch { cached: None }) => {
                (None, DolphinLocalLookupState::RegionMismatch)
            }
            Ok(DolphinGeckoLookupResult::CatalogueEntryHasNoUsableCodes {
                warnings,
                cached: None,
            }) => (None, DolphinLocalLookupState::NoUsableCodes { warnings }),
            Err(_) => (None, DolphinLocalLookupState::NotAttempted),
        };

        let Some(fetch) = fetch else {
            if let Some(workflow) = self.cheat_workflow.as_mut() {
                workflow.dolphin_local_lookup = local_state;
            }
            return false;
        };

        let key = DolphinProviderRequestKey {
            archive_path: workflow.archive_path.clone(),
            game_id,
            revision,
        };
        let (selection, destination_error) = build_dolphin_provider_selection(
            dolphin_profile_paths,
            workflow.selected_dolphin_profile_id.as_deref(),
            &fetch,
        );
        let archive_path = workflow.archive_path.clone();
        let message = format!(
            "Local Dolphin cheat source returned {} exact-ID code(s) for {} ({}).",
            fetch.result.entries.len(),
            fetch.result.game_id,
            dolphin_provider_fetch_status_label(fetch.status)
        );

        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return false;
        };
        workflow.dolphin_provider_request = Some(key);
        workflow.dolphin_destination_error = destination_error;
        workflow.dolphin_provider_selection = selection;
        workflow.dolphin_provider = CheatStepResource::Ready(fetch);
        workflow.dolphin_local_lookup = DolphinLocalLookupState::NotAttempted;
        self.history.record(HistoryEntry::new(
            ActivityAction::DolphinGeckoCandidateMatch,
            Some(archive_path),
            ActivityOutcome::Completed,
            message,
        ));
        true
    }

    /// Dolphin Stage 5: stages the surgically edited GameSettings file and
    /// builds its shared install preview. Synchronous for the same reason
    /// as `start_dolphin_candidate_match` - a single small local file.
    pub(crate) fn start_dolphin_install_preview(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let Some(profile_id) = workflow.selected_dolphin_profile_id.clone() else {
            return;
        };
        let CheatStepResource::Ready(provider) = &workflow.dolphin_provider else {
            return;
        };
        let Some(state) = workflow.dolphin_provider_selection.as_ref() else {
            return;
        };
        let key = cheat_preview_key(workflow);
        let archive_path = workflow.archive_path.clone();
        let configuration_path = match &self.dolphin_profiles {
            DolphinProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == profile_id)
                .map(|profile| profile.configuration_path.clone()),
            _ => None,
        };
        let Some(configuration_path) = configuration_path else {
            self.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(archive_path),
                ActivityOutcome::Rejected,
                "Install preview blocked: the selected Dolphin profile is no longer eligible.",
            ));
            return;
        };
        let names = match state.selection.resolve_names(&provider.result) {
            Ok(names) => names,
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Rejected,
                    format!("Install preview blocked: {}", error.detail),
                ));
                return;
            }
        };
        let staging_root = match default_generated_dolphin_staging_root() {
            Ok(root) => root,
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    message,
                ));
                return;
            }
        };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            Some(archive_path.clone()),
            ActivityOutcome::Started,
            format!(
                "Generating an install preview for {} selected Gecko code(s).",
                names.len()
            ),
        ));
        let response = (|| {
            let staged = stage_dolphin_provider_ini(
                &staging_root,
                &state.destination,
                &provider.result,
                &state.selection,
            )?;
            let preview = build_dolphin_install_preview(&DolphinInstallPreviewRequest {
                selected_archive: archive_path.clone(),
                configuration_path,
                game_id: provider.result.game_id.clone(),
                revision: Some(provider.result.revision),
                staged: staged.clone(),
            })?;
            Ok::<_, archivefs_core::patch_manager::DolphinInstallPlanError>((
                preview,
                staged,
                staging_root,
            ))
        })();
        let message = match response {
            Ok((preview, staged, staging_root)) => CheatPreviewResponse {
                key: key.clone(),
                outcome: CheatPreviewOutcome::Ready(preview.report),
                materialized: None,
                generated: None,
                dolphin_generated: Some(GeneratedDolphinInstall {
                    staging_root,
                    provider: provider.clone(),
                    destination: state.destination.path.clone(),
                    staged,
                }),
                xenia_generated: None,
                pcsx2_generated: None,
                gamecube_gamehacking_generated: None,
                bsfree_gamecube_generated: None,

                bsfree_wii_generated: None,
            },
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    format!("Install preview failed: {}", error.detail),
                ));
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::DolphinInstallPlan(
                        error,
                    )),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                }
            }
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Ready(message);
        workflow.transaction = CheatTransactionState::Idle;
    }

    /// Explicit-directory-only and synchronous - see
    /// `discover_xenia_profiles`'s own documentation for why this never
    /// needs a background thread or a failure state.
    pub(crate) fn start_xenia_profile_scan(&mut self) {
        let explicit_root = self
            .cheat_workflow
            .as_ref()
            .map(|workflow| workflow.xenia_explicit_root.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let roots = XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: explicit_root.into_iter().collect(),
        };
        let discovery = discover_xenia_profiles(&roots);
        self.history.record(HistoryEntry::new(
            ActivityAction::XeniaProfileScan,
            None,
            ActivityOutcome::Completed,
            format!(
                "Xenia Canary profile discovery found {} profile(s) ({} eligible).",
                discovery.profiles.len(),
                discovery
                    .profiles
                    .iter()
                    .filter(|profile| profile.eligible)
                    .count()
            ),
        ));
        let eligible = eligible_xenia_profile_ids(&discovery);
        let candidates = xenia_profile_candidates(&discovery);
        let remembered = self.remembered_profile_id("xenia");
        let mut to_persist: Option<(String, PathBuf)> = None;
        if let Some(workflow) = self.cheat_workflow.as_mut()
            && workflow.adapter == CheatEmulatorAdapter::Xenia
        {
            let session_explicit = workflow.xenia_profile_choice.clone();
            let selection = select_emulator_profile(
                &candidates,
                remembered.as_deref(),
                session_explicit.as_deref(),
            );
            let install_in_progress = !matches!(workflow.transaction, CheatTransactionState::Idle);
            if let EmulatorProfileSelection::Auto { profile_id, .. } = &selection
                && !install_in_progress
                && workflow.selected_xenia_profile_id.as_deref() != Some(profile_id.as_str())
            {
                workflow.selected_xenia_profile_id = Some(profile_id.clone());
                if let Some(root) = candidates
                    .iter()
                    .find(|candidate| candidate.profile_id == *profile_id)
                    .map(|candidate| candidate.root.clone())
                {
                    to_persist = Some((profile_id.clone(), root));
                }
            } else if !matches!(selection, EmulatorProfileSelection::Auto { .. })
                && !install_in_progress
                && workflow
                    .selected_xenia_profile_id
                    .as_ref()
                    .is_none_or(|selected| !eligible.contains(&selected.as_str()))
            {
                workflow.selected_xenia_profile_id = None;
            }
            workflow.xenia_profile_selection = Some(selection);
        }
        self.xenia_profiles = XeniaProfilesState::Ready(discovery);
        if let Some((profile_id, root)) = to_persist {
            self.persist_remembered_profile("xenia", &profile_id, &root);
        }
    }

    pub(crate) fn start_xenia_provider_fetch(&mut self, context: egui::Context, force_refresh: bool) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        if workflow.adapter != CheatEmulatorAdapter::Xenia
            || matches!(workflow.xenia_provider, CheatStepResource::Loading { .. })
        {
            return;
        }
        let Some(title_id) = ready_game_identity(workflow)
            .and_then(GameIdentityReport::verified_xex_title_id)
            .map(str::to_string)
        else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let mut options = match XeniaProviderFetchOptions::with_default_cache(now) {
            Ok(options) => options,
            Err(error) => {
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.xenia_provider = CheatStepResource::Failed(error.to_string());
                }
                return;
            }
        };
        options.force_refresh = force_refresh;
        let key = XeniaProviderRequestKey {
            archive_path: workflow.archive_path.clone(),
            title_id: title_id.clone(),
        };
        let archive_path = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.xenia_provider_request = Some(key);
        workflow.xenia_provider = CheatStepResource::Loading { receiver };
        workflow.xenia_selected_candidate_index = None;
        workflow.xenia_selection = None;
        workflow.xenia_destination_error = None;
        workflow.preview_request = None;
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.transaction = CheatTransactionState::Idle;
        self.history.record(HistoryEntry::new(
            ActivityAction::XeniaPatchCandidateMatch,
            Some(archive_path),
            ActivityOutcome::Started,
            if force_refresh {
                "Refreshing exact Title ID patches from the Xenia Canary game-patches provider."
            } else {
                "Loading exact Title ID patches from the Xenia Canary game-patches provider or its local cache."
            },
        ));
        thread::spawn(move || {
            let result = fetch_xenia_provider_patches(&title_id, &options)
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Applies one Xenia patch picker edit and invalidates anything
    /// downstream, the same way `update_dolphin_code_selection` does.
    pub(crate) fn update_xenia_patch_selection(&mut self, edit: impl FnOnce(&mut XeniaPatchSelection)) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let Some(state) = workflow.xenia_selection.as_mut() else {
            return;
        };
        edit(&mut state.selection);
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
    }

    /// Stages the merged `.patch.toml` and builds its shared install
    /// preview. Synchronous for the same reason as Dolphin's local path -
    /// a single small local file.
    pub(crate) fn start_xenia_install_preview(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let Some(profile_id) = workflow.selected_xenia_profile_id.clone() else {
            return;
        };
        let Some(state) = workflow.xenia_selection.as_ref() else {
            return;
        };
        let key = cheat_preview_key(workflow);
        let archive_path = workflow.archive_path.clone();
        let configuration_path = match &self.xenia_profiles {
            XeniaProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == profile_id)
                .map(|profile| profile.configuration_path.clone()),
            XeniaProfilesState::NotScanned => None,
        };
        let Some(configuration_path) = configuration_path else {
            self.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(archive_path),
                ActivityOutcome::Rejected,
                "Install preview blocked: the selected Xenia profile is no longer eligible.",
            ));
            return;
        };
        let names = match state.selection.resolve_names() {
            Ok(names) => names,
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Rejected,
                    format!("Install preview blocked: {}", error.detail),
                ));
                return;
            }
        };
        let staging_root = match default_generated_xenia_staging_root() {
            Ok(root) => root,
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    message,
                ));
                return;
            }
        };
        let file_name = state
            .destination
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            Some(archive_path.clone()),
            ActivityOutcome::Started,
            format!(
                "Generating an install preview for {} selected patch(es).",
                names.len()
            ),
        ));
        let response = (|| {
            let staged = stage_xenia_patch_file(
                &staging_root,
                &file_name,
                &state.candidate,
                state.destination.document.as_ref(),
                &names,
            )?;
            let preview = build_xenia_install_preview(&XeniaInstallPreviewRequest {
                selected_archive: archive_path.clone(),
                configuration_path,
                title_id: state.candidate.title_id.clone(),
                compatibility: state.candidate.compatibility,
                staged: staged.clone(),
            })?;
            Ok::<_, XeniaInstallPlanError>((preview, staged, staging_root))
        })();
        let message = match response {
            Ok((preview, staged, staging_root)) => CheatPreviewResponse {
                key: key.clone(),
                outcome: CheatPreviewOutcome::Ready(preview.report),
                materialized: None,
                generated: None,
                dolphin_generated: None,
                xenia_generated: Some(GeneratedXeniaInstall {
                    staging_root,
                    candidate: state.candidate.clone(),
                    destination: state.destination.path.clone(),
                    staged,
                }),
                pcsx2_generated: None,
                gamecube_gamehacking_generated: None,
                bsfree_gamecube_generated: None,

                bsfree_wii_generated: None,
            },
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    format!("Install preview failed: {}", error.detail),
                ));
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::XeniaInstallPlan(
                        error,
                    )),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                }
            }
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Ready(message);
        workflow.transaction = CheatTransactionState::Idle;
    }

    pub(crate) fn start_generated_cheat_preview(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let Some(selection) = workflow.candidate_selection.as_ref() else {
            return;
        };
        let Some(destination_root) =
            selected_retroarch_cheat_root(workflow, &self.retroarch_profiles)
        else {
            return;
        };
        let Some(key) =
            workflow
                .candidates_request
                .clone()
                .or_else(|| match &workflow.candidates {
                    CheatStepResource::Ready(stage) => Some(stage.key.clone()),
                    _ => None,
                })
        else {
            return;
        };

        let entries = match selection.selection.resolve(&selection.loaded.document) {
            Ok(entries) => entries,
            Err(error) => {
                let archive = workflow.archive_path.clone();
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive),
                    ActivityOutcome::Rejected,
                    format!("Install preview blocked: {}", error.detail),
                ));
                return;
            }
        };

        let destination_request = CheatDestinationRequest {
            profile_cheat_root: destination_root.clone(),
            platform: workflow.platform.clone(),
            content_basename: cheat_content_basename(workflow),
            playlist_name: None,
            catalogue_name: selection.candidate.display_name.clone(),
        };
        let match_strength = match match_strength_for_candidate(&selection.candidate) {
            Ok(strength) => strength,
            Err(error) => {
                let archive = workflow.archive_path.clone();
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive),
                    ActivityOutcome::Rejected,
                    format!("Install preview blocked: {}", error.detail),
                ));
                return;
            }
        };
        let staging_root = match default_generated_cheat_staging_root() {
            Ok(root) => root,
            Err(message) => {
                let archive = workflow.archive_path.clone();
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive),
                    ActivityOutcome::Failed,
                    message,
                ));
                return;
            }
        };
        let archive_path = workflow.archive_path.clone();
        let platform = workflow.platform.clone();
        let candidate_display_name = selection.candidate.display_name.clone();
        let identity = format!(
            "retroarch-catalogue:{}:{}",
            selection.candidate.catalogue_relative_path, selection.loaded.digest
        );
        let comments = vec![format!(
            "Source catalogue file: {}",
            selection.candidate.catalogue_relative_path
        )];

        let worker_key = key.clone();
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Loading { receiver };
        workflow.transaction = CheatTransactionState::Idle;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            Some(archive_path.clone()),
            ActivityOutcome::Started,
            format!(
                "Generating an install preview for {} selected cheat(s).",
                entries.len()
            ),
        ));
        thread::spawn(move || {
            let response = (|| {
                let destination = resolve_cheat_destination(&destination_request)?;
                let staged = stage_generated_cheat_file(
                    &staging_root,
                    destination
                        .file_name
                        .strip_suffix(".cht")
                        .unwrap_or(&destination.file_name),
                    &entries,
                    &comments,
                )?;
                let preview = build_cheat_install_preview(&CheatInstallPreviewRequest {
                    selected_archive: archive_path.clone(),
                    platform,
                    verified_identity: identity,
                    destination: destination.clone(),
                    profile_cheat_root: destination_request.profile_cheat_root.clone(),
                    staged: staged.clone(),
                    match_strength,
                })?;
                Ok::<_, CheatInstallPlanError>((preview, destination, staged, staging_root))
            })();
            let message = match response {
                Ok((preview, destination, staged, staging_root)) => CheatPreviewResponse {
                    key: worker_key,
                    outcome: CheatPreviewOutcome::Ready(preview.report),
                    materialized: None,
                    generated: Some(GeneratedCheatInstall {
                        staging_root,
                        destination,
                        staged,
                        candidate_display_name,
                    }),
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                },
                Err(error) => CheatPreviewResponse {
                    key: worker_key,
                    outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::InstallPlan(error)),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                },
            };
            let _ = sender.send(Ok(message));
            context.request_repaint();
        });
    }

    /// Applies one picker edit and invalidates anything downstream of it.
    /// A preview built from a different selection must never survive a
    /// change to that selection.
    pub(crate) fn update_cheat_selection(&mut self, edit: impl FnOnce(&mut CheatSelection)) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let Some(selection) = workflow.candidate_selection.as_mut() else {
            return;
        };
        edit(&mut selection.selection);
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
    }

    /// Stage 9: rolls the completed install back through the same
    /// journal-backed machinery History & Logs uses, so there is exactly
    /// one rollback implementation.
    pub(crate) fn start_cheat_install_rollback(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let CheatTransactionState::Result { result, .. } = &workflow.transaction else {
            return;
        };
        let Some(journal_path) = result.journal_path.clone() else {
            self.history.record(HistoryEntry::new(
                ActivityAction::CheatInstall,
                Some(workflow.archive_path.clone()),
                ActivityOutcome::Rejected,
                "Undo unavailable: this install did not create an undo record.",
            ));
            return;
        };
        let destination_root = PathBuf::from(&result.journal.destination_root.display);
        let archive = workflow.archive_path.clone();
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatInstall,
            Some(archive),
            ActivityOutcome::Started,
            format!(
                "Rollback requested for install '{}'.",
                result.journal.operation_id
            ),
        ));
        // Phase 4 fix: undoing a change requires an explicit Review-then-
        // Confirm step (SharedRollbackState::Review) - a deliberate safety
        // pattern this codebase already uses for every apply-style action,
        // not something to skip. Gamer View has no confirm-dialog UI of
        // its own for it (building one would be new-feature scope, not a
        // polish pass), so - exactly like the existing "Open Advanced
        // View's Mount page to resolve this" precedent for a blocked
        // mount - this switches into Advanced View so the already-built
        // review screen is actually visible. Without also setting
        // `ui_mode` here, this used to silently update `self.view` while
        // still rendering Gamer View, which never reads it: a click with
        // no visible effect at all.
        self.ui_mode = GuiMode::AdvancedView;
        save_gui_mode(self.ui_mode);
        self.view = MainView::HistoryLogs;
        self.feedback = Some(ActionFeedback {
            succeeded: true,
            message: "Opening your undo history so you can review and confirm this change."
                .to_string(),
            cleanup: None,
            warning: None,
            more_information: None,
        });
        self.start_shared_rollback_preview(context, journal_path, destination_root);
    }

    pub(crate) fn review_cheat_apply(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if !matches!(
            workflow.adapter,
            CheatEmulatorAdapter::RetroArch
                | CheatEmulatorAdapter::Pcsx2
                | CheatEmulatorAdapter::Dolphin
                | CheatEmulatorAdapter::Xenia
        ) {
            return;
        }
        let CheatStepResource::Ready(response) = &workflow.preview else {
            return;
        };
        let CheatPreviewOutcome::Ready(report) = &response.outcome else {
            return;
        };
        // The approved source root differs by path: a whole-file catalogue
        // install is approved against the immutable snapshot it came from,
        // while a generated selected-cheat/selected-code install (RetroArch,
        // Dolphin, or Xenia) is approved against the private staging root
        // its bytes were written into.
        let Some(approved_source_root) = response
            .generated
            .as_ref()
            .map(|generated| generated.staging_root.clone())
            .or_else(|| {
                response
                    .dolphin_generated
                    .as_ref()
                    .map(|generated| generated.staging_root.clone())
            })
            .or_else(|| {
                response
                    .xenia_generated
                    .as_ref()
                    .map(|generated| generated.staging_root.clone())
            })
            .or_else(|| {
                response
                    .pcsx2_generated
                    .as_ref()
                    .map(|generated| generated.staging_root.clone())
            })
            .or_else(|| {
                response
                    .gamecube_gamehacking_generated
                    .as_ref()
                    .map(|generated| generated.staging_root.clone())
            })
            .or_else(|| {
                response
                    .bsfree_gamecube_generated
                    .as_ref()
                    .map(|generated| generated.staging_root.clone())
            })
            .or_else(|| {
                response
                    .bsfree_wii_generated
                    .as_ref()
                    .map(|generated| generated.staging_root.clone())
            })
            .or_else(|| {
                response
                    .materialized
                    .as_ref()
                    .map(|materialized| materialized.snapshot_root.clone())
            })
        else {
            return;
        };
        let profile_id = match workflow.adapter {
            CheatEmulatorAdapter::RetroArch => workflow.selected_profile_id.as_deref(),
            CheatEmulatorAdapter::Dolphin => workflow.selected_dolphin_profile_id.as_deref(),
            CheatEmulatorAdapter::Xenia => workflow.selected_xenia_profile_id.as_deref(),
            CheatEmulatorAdapter::Pcsx2 => workflow.selected_pcsx2_profile_id.as_deref(),
            CheatEmulatorAdapter::Unsupported => None,
        };
        let Some(profile_id) = profile_id else {
            return;
        };
        match build_shared_transaction_plan(
            report,
            profile_id,
            workflow.source_mode.label(),
            &approved_source_root,
        ) {
            Ok(mut plan) => {
                if let Some(generated) = &response.gamecube_gamehacking_generated {
                    let expected_managed_names = archivefs_core::patch_manager::managed_names(
                        &parse_dolphin_ini(&generated.staged.contents),
                    )
                    .into_iter()
                    .collect();
                    if let Err(error) = require_dolphin_managed_gamehacking_verification(
                        &mut plan,
                        expected_managed_names,
                    ) {
                        self.history.record(HistoryEntry::new(
                            ActivityAction::CheatPreview,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Rejected,
                            format!(
                                "GameCube live-target verification plan blocked: {}",
                                error.detail
                            ),
                        ));
                        return;
                    }
                }
                if let Some(generated) = &response.bsfree_gamecube_generated {
                    let expected_managed_names = archivefs_core::patch_manager::managed_names(
                        &parse_dolphin_ini(&generated.staged.contents),
                    )
                    .into_iter()
                    .collect();
                    if let Err(error) = require_dolphin_managed_gamehacking_verification(
                        &mut plan,
                        expected_managed_names,
                    ) {
                        self.history.record(HistoryEntry::new(
                            ActivityAction::CheatPreview,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Rejected,
                            format!(
                                "BSFree live-target verification plan blocked: {}",
                                error.detail
                            ),
                        ));
                        return;
                    }
                }
                workflow.transaction = CheatTransactionState::Review {
                    key: response.key.clone(),
                    plan,
                    replacement_approved: false,
                };
            }
            Err(error) => self.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(workflow.archive_path.clone()),
                ActivityOutcome::Rejected,
                format!("Shared apply review blocked: {}", error.detail),
            )),
        }
    }

    pub(crate) fn refresh_shared_history(&mut self, context: egui::Context) {
        let history_root = match default_shared_history_root() {
            Ok(path) => path,
            Err(error) => {
                self.shared_history = SharedHistoryState::Failed(error.detail);
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.shared_history = SharedHistoryState::Loading { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            None,
            ActivityOutcome::Started,
            "Refreshing bounded shared transaction history.",
        ));
        thread::spawn(move || {
            let report = discover_shared_apply_history(&history_root);
            let _ = sender.send(Ok(report));
            context.request_repaint();
        });
    }

    pub(crate) fn poll_shared_history(&mut self) {
        let SharedHistoryState::Loading { receiver } = &self.shared_history else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(report)) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    None,
                    ActivityOutcome::Completed,
                    format!(
                        "Shared transaction history refreshed: {} journal(s), {} warning(s).",
                        report.journals.len(),
                        report.warnings.len()
                    ),
                ));
                self.shared_history = SharedHistoryState::Ready(report);
            }
            Ok(Err(message)) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    None,
                    ActivityOutcome::Failed,
                    format!("Shared transaction history refresh failed: {message}"),
                ));
                self.shared_history = SharedHistoryState::Failed(message);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.shared_history = SharedHistoryState::Failed(
                    "Transaction history worker stopped unexpectedly.".to_string(),
                );
            }
        }
    }

    pub(crate) fn start_shared_rollback_preview(
        &mut self,
        context: egui::Context,
        journal_path: PathBuf,
        destination_root: PathBuf,
    ) {
        let Ok(history_root) = default_shared_history_root() else {
            self.shared_rollback =
                SharedRollbackState::Failed("Managed history root is unavailable.".to_string());
            return;
        };
        let Ok(backup_root) = default_shared_backup_root() else {
            self.shared_rollback =
                SharedRollbackState::Failed("Managed backup root is unavailable.".to_string());
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.shared_rollback = SharedRollbackState::Previewing { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            None,
            ActivityOutcome::Started,
            "Rollback preview started; no files are being changed.",
        ));
        thread::spawn(move || {
            let preview = preview_shared_rollback(&journal_path, &destination_root, &backup_root);
            let _ = sender.send(Ok((preview, history_root, backup_root)));
            context.request_repaint();
        });
    }

    pub(crate) fn start_shared_rollback(&mut self, context: egui::Context) {
        let state = std::mem::replace(&mut self.shared_rollback, SharedRollbackState::Idle);
        let SharedRollbackState::Review {
            preview,
            history_root,
            backup_root,
        } = state
        else {
            self.shared_rollback = state;
            return;
        };
        if !preview.available {
            self.shared_rollback = SharedRollbackState::Review {
                preview,
                history_root,
                backup_root,
            };
            return;
        }
        let options = SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
            history_root,
            backup_root,
        };
        let (sender, receiver) = mpsc::channel();
        self.shared_rollback = SharedRollbackState::Applying { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            None,
            ActivityOutcome::Started,
            format!(
                "Rollback '{}' started after exact preview confirmation.",
                options.rollback_operation_id
            ),
        ));
        thread::spawn(move || {
            let result = execute_shared_rollback(&preview, &options);
            let _ = sender.send(Ok(result));
            context.request_repaint();
        });
    }

    pub(crate) fn poll_shared_rollback(&mut self) {
        match &self.shared_rollback {
            SharedRollbackState::Previewing { receiver } => match receiver.try_recv() {
                Ok(Ok((preview, history_root, backup_root))) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::CheatPreview,
                        None,
                        if preview.available {
                            ActivityOutcome::Completed
                        } else {
                            ActivityOutcome::Rejected
                        },
                        if preview.available {
                            "Rollback preview completed and is available."
                        } else {
                            "Rollback preview completed and is blocked."
                        },
                    ));
                    self.shared_rollback = SharedRollbackState::Review {
                        preview,
                        history_root,
                        backup_root,
                    };
                }
                Ok(Err(message)) => self.shared_rollback = SharedRollbackState::Failed(message),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.shared_rollback = SharedRollbackState::Failed(
                        "Rollback preview worker stopped unexpectedly.".to_string(),
                    );
                }
            },
            SharedRollbackState::Applying { receiver } => match receiver.try_recv() {
                Ok(Ok(result)) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::CheatPreview,
                        None,
                        if result.status == SharedApplyStatus::Success {
                            ActivityOutcome::Completed
                        } else {
                            ActivityOutcome::Failed
                        },
                        format!("Rollback finished with {:?}.", result.status),
                    ));
                    if result.status == SharedApplyStatus::Success
                        && let Some(workflow) = self.cheat_workflow.as_mut()
                        && matches!(
                            &workflow.transaction,
                            CheatTransactionState::Result { result: apply, .. }
                                if apply.journal.operation_id == result.preview.original_operation_id
                        )
                    {
                        // The selected provider state was built from the
                        // exact pre-apply destination that rollback just
                        // restored. Return to it instead of leaving a stale
                        // "Installed successfully / Undo" card behind.
                        workflow.transaction = CheatTransactionState::Idle;
                    }
                    self.shared_rollback = SharedRollbackState::Result(result);
                    self.shared_history = SharedHistoryState::NotLoaded;
                }
                Ok(Err(message)) => self.shared_rollback = SharedRollbackState::Failed(message),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.shared_rollback = SharedRollbackState::Failed(
                        "Rollback worker stopped unexpectedly.".to_string(),
                    );
                }
            },
            _ => {}
        }
    }

    pub(crate) fn start_cheat_apply(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let state = std::mem::replace(&mut workflow.transaction, CheatTransactionState::Idle);
        let CheatTransactionState::Review {
            key,
            plan,
            replacement_approved,
        } = state
        else {
            workflow.transaction = state;
            return;
        };
        if key != cheat_preview_key(workflow)
            || plan.context.selected_archive.to_path_buf().ok().as_ref()
                != Some(&workflow.archive_path)
        {
            return;
        }
        let loose_identity = ready_game_identity(workflow).and_then(|report| {
            report.verified_loose_rom_sha256().map(|digest| {
                (
                    workflow.archive_path.clone(),
                    workflow.platform.clone(),
                    digest.to_string(),
                )
            })
        });
        let history_root = match default_shared_history_root() {
            Ok(path) => path,
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatInstall,
                    Some(workflow.archive_path.clone()),
                    ActivityOutcome::Failed,
                    format!("Apply root unavailable: {}", error.detail),
                ));
                return;
            }
        };
        let backup_root = match default_shared_backup_root() {
            Ok(path) => path,
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatInstall,
                    Some(workflow.archive_path.clone()),
                    ActivityOutcome::Failed,
                    format!("Backup root unavailable: {}", error.detail),
                ));
                return;
            }
        };
        let operation_id = generate_shared_operation_id();
        let timestamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let options = SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: true,
                replacement_approved,
            }),
            operation_id: operation_id.clone(),
            timestamp_unix_seconds: timestamp,
            current_context: plan.context.clone(),
            history_root,
            backup_root,
        };
        let archive = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        workflow.transaction = CheatTransactionState::Applying {
            key: key.clone(),
            receiver,
        };
        workflow.transaction_notice = None;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatInstall,
            Some(archive),
            ActivityOutcome::Started,
            format!("Shared apply '{operation_id}' started."),
        ));
        thread::spawn(move || {
            if let Some((path, platform, expected_digest)) = loose_identity {
                let current = inspect_catalogued_game_identity(&path, platform.as_deref());
                if current.verified_loose_rom_sha256() != Some(expected_digest.as_str()) {
                    let _ = sender.send(Err(
                        "Loose ROM changed after preview; the approved plan was rejected before writing."
                            .to_string(),
                    ));
                    context.request_repaint();
                    return;
                }
            }
            let result = execute_shared_apply(&plan, &options);
            let _ = sender.send(Ok(result));
            context.request_repaint();
        });
    }

    pub(crate) fn open_cheat_archive_picker(&mut self) {
        let adapter_platform =
            self.cheat_workflow
                .as_ref()
                .and_then(|workflow| match workflow.adapter {
                    CheatEmulatorAdapter::Dolphin => workflow
                        .platform
                        .as_ref()
                        .filter(|platform| matches!(platform.as_str(), "GameCube" | "Wii"))
                        .cloned()
                        .or_else(|| Some("GameCube".to_string())),
                    CheatEmulatorAdapter::Xenia => Some("Xbox360".to_string()),
                    CheatEmulatorAdapter::Pcsx2 => Some("PS2".to_string()),
                    _ => self.library_filters.platform.clone(),
                });
        self.library_filters.platform = adapter_platform.clone();
        self.cheat_archive_picker = Some(CheatArchivePickerState::for_current(
            self.cheat_workflow
                .as_ref()
                .map(|workflow| workflow.archive_path.as_path()),
            adapter_platform,
        ));
    }

    /// Applies one explicit picker choice. Also writes the choice back to
    /// `selected_archive`/`selected_archives` (the field Library, Selected,
    /// and Mount all read) so the archive picked here is immediately the
    /// one every other page considers selected too - archive selection is
    /// authoritative and shared, not a Cheats & Mods-only copy. Queue
    /// membership and mount state are untouched.
    pub(crate) fn apply_cheat_archive_choice(&mut self, context: &egui::Context, archive_path: PathBuf) {
        self.confirm_cheat_archive_change = None;
        self.cheat_archive_picker = None;
        let needs_profile_scan = matches!(
            self.retroarch_profiles,
            RetroArchProfilesState::NotScanned | RetroArchProfilesState::Error(_)
        );
        self.archive_context.select_only(archive_path.clone());
        self.open_cheats_mods_workspace(context, archive_path);
        if self.cheat_workflow.is_some() && needs_profile_scan {
            self.start_retroarch_profile_scan(context.clone());
        }
    }

    /// Lists the trusted cheat sources and their cached snapshots in
    /// the background (read-only cache inspection, no network).
    pub(crate) fn start_cheat_source_list(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        workflow.source_list = CheatStepResource::Loading { receiver };
        thread::spawn(move || {
            let result = default_cheat_source_cache_root()
                .map_err(|error| error.to_string())
                .and_then(|cache_root| {
                    list_retroarch_cheat_sources(&cache_root).map_err(|error| error.to_string())
                });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn start_existing_retroarch_library_inspection(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let Some(profile_id) = workflow.selected_profile_id.clone() else {
            return;
        };
        let destination = match &self.retroarch_profiles {
            RetroArchProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == profile_id)
                .and_then(|profile| profile.cheat_destination_root.as_ref())
                .filter(|path| !path.lossy)
                .map(|path| PathBuf::from(&path.display)),
            _ => None,
        };
        let Some(destination) = destination else {
            workflow.existing_library_profile_id = Some(profile_id);
            workflow.existing_library = CheatStepResource::Failed(
                "The selected profile has no safely resolved cheat destination.".to_string(),
            );
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let platform = workflow.platform.clone().unwrap_or_default();
        let display_name = workflow.display_name.clone();
        workflow.existing_library_profile_id = Some(profile_id);
        workflow.existing_library = CheatStepResource::Loading { receiver };
        thread::spawn(move || {
            let result =
                inspect_retroarch_cheat_library_for_game(&destination, &platform, &display_name);
            let _ = sender.send(Ok(result));
            context.request_repaint();
        });
    }

    pub(crate) fn start_pcsx2_gamehacking_fetch(&mut self, context: egui::Context, force_refresh: bool) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        if workflow.adapter != CheatEmulatorAdapter::Pcsx2
            || matches!(
                workflow.pcsx2_gamehacking,
                CheatStepResource::Loading { .. }
            )
        {
            return;
        }
        let Some(identity) = pcsx2_identity_for_workflow(workflow) else {
            if let Some(workflow) = self.cheat_workflow.as_mut() {
                workflow.pcsx2_gamehacking = CheatStepResource::Failed(
                    "EmuWiz needs a verified local PCSX2 executable CRC before checking the cached GameHacking.org PS2 catalogue."
                        .to_string(),
                );
            }
            return;
        };
        let mut options = match GameHackingFetchOptions::defaults() {
            Ok(options) => options,
            Err(failure) => {
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.pcsx2_gamehacking = CheatStepResource::Failed(failure.to_string());
                }
                return;
            }
        };
        options.force_refresh = force_refresh;
        let archive_path = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.pcsx2_gamehacking = CheatStepResource::Loading { receiver };
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            "Checking one local PS2 game against GameHacking.org.",
        ));
        thread::spawn(move || {
            let provider = GameHackingProvider::default();
            let result = (|| {
                let matched = provider.match_game(&identity, &options)?;
                let Some(game) = matched.game.clone() else {
                    return Ok(Pcsx2GameHackingState {
                        status: matched.status,
                        detail: matched.detail,
                        game: None,
                        match_candidates: matched.candidates,
                        candidates: Vec::new(),
                        selection: Pcsx2CheatSelection::default(),
                        cached_fallback: false,
                    });
                };
                let fetch = provider.fetch_cheats_with_status(&identity, &game, &options)?;
                let cheats = fetch.data;
                let catalogue = provider.catalogue(&identity, &game, &cheats)?;
                let candidates = build_pcsx2_cheat_candidates(&catalogue, &identity);
                Ok(Pcsx2GameHackingState {
                    status: matched.status,
                    detail: matched.detail,
                    game: Some(game),
                    match_candidates: Vec::new(),
                    candidates,
                    selection: Pcsx2CheatSelection::default(),
                    cached_fallback: fetch.cached_fallback,
                })
            })()
            .map_err(|failure: archivefs_core::patch_manager::GameHackingError| {
                failure.to_string()
            });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn confirm_pcsx2_gamehacking_match(&mut self, context: egui::Context, game_id: u64) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let Some(identity) = pcsx2_identity_for_workflow(workflow) else {
            return;
        };
        let CheatStepResource::Ready(state) = &workflow.pcsx2_gamehacking else {
            return;
        };
        let Some(game) = state
            .match_candidates
            .iter()
            .find(|candidate| candidate.game.game_id == game_id)
            .map(|candidate| candidate.game.clone())
        else {
            return;
        };
        let options = match GameHackingFetchOptions::defaults() {
            Ok(options) => options,
            Err(failure) => {
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.pcsx2_gamehacking = CheatStepResource::Failed(failure.to_string());
                }
                return;
            }
        };
        let archive_path = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.pcsx2_gamehacking = CheatStepResource::Loading { receiver };
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            format!("Downloading confirmed GameHacking.org game {game_id} PCSX2 export."),
        ));
        thread::spawn(move || {
            let provider = GameHackingProvider::default();
            let result = (|| {
                let fetch = provider
                    .fetch_cheats_for_confirmed_candidate_with_status(&identity, &game, &options)?;
                let cheats = fetch.data;
                let catalogue =
                    provider.catalogue_for_confirmed_candidate(&identity, &game, &cheats)?;
                let candidates = build_pcsx2_cheat_candidates(&catalogue, &identity);
                Ok(Pcsx2GameHackingState {
                    status: GameHackingMatchStatus::Matched,
                    detail: format!(
                        "Using user-confirmed GameHacking.org match: {} (game {}).",
                        game.title, game.game_id
                    ),
                    game: Some(game),
                    match_candidates: Vec::new(),
                    candidates,
                    selection: Pcsx2CheatSelection::default(),
                    cached_fallback: fetch.cached_fallback,
                })
            })()
            .map_err(|failure: archivefs_core::patch_manager::GameHackingError| {
                failure.to_string()
            });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Matches GameCube or Wii through its platform adapter, while sharing
    /// the existing Dolphin selection/install workflow.
    pub(crate) fn start_gamecube_gamehacking_fetch(&mut self, context: egui::Context, force_refresh: bool) {
        self.start_gamecube_gamehacking_fetch_mode(
            context,
            force_refresh,
            WiiGameHackingFetchMode::ExplicitNetworkAllowed,
        );
    }

    pub(crate) fn start_gamecube_gamehacking_fetch_mode(
        &mut self,
        context: egui::Context,
        force_refresh: bool,
        wii_mode: WiiGameHackingFetchMode,
    ) {
        if self.cheat_workflow.as_ref().is_some_and(|workflow| {
            matches!(
                workflow.gamecube_gamehacking,
                CheatStepResource::Loading { .. }
            )
        }) {
            return;
        }
        let mut options = match GameHackingGameCubeFetchOptions::defaults() {
            Ok(options) => options,
            Err(failure) => {
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.gamecube_gamehacking = CheatStepResource::Failed(failure.to_string());
                    workflow.gamecube_gamehacking_blocked = false;
                }
                return;
            }
        };
        options.force_refresh = force_refresh;
        self.start_gamecube_gamehacking_fetch_with_options(context, wii_mode, options);
    }

    pub(crate) fn start_gamecube_gamehacking_fetch_with_options(
        &mut self,
        context: egui::Context,
        wii_mode: WiiGameHackingFetchMode,
        mut options: GameHackingGameCubeFetchOptions,
    ) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let is_wii = workflow.platform.as_deref() == Some("Wii");
        if workflow.adapter != CheatEmulatorAdapter::Dolphin
            || (!is_wii && workflow.platform.as_deref() != Some("GameCube"))
            || matches!(
                workflow.gamecube_gamehacking,
                CheatStepResource::Loading { .. }
            )
        {
            return;
        }
        let gamecube_identity = (!is_wii)
            .then(|| gamecube_identity_for_workflow(workflow))
            .flatten();
        let wii_identity = is_wii
            .then(|| wii_identity_for_workflow(workflow))
            .flatten();
        if gamecube_identity.is_none() && wii_identity.is_none() {
            if let Some(workflow) = self.cheat_workflow.as_mut() {
                workflow.gamecube_gamehacking = CheatStepResource::Failed(
                    if is_wii {
                        "EmuWiz needs a verified local Dolphin Game ID before checking the cached GameHacking.org Wii catalogue."
                    } else {
                        "EmuWiz needs a verified local Dolphin Game ID before checking the cached GameHacking.org GameCube catalogue."
                    }
                    .to_string(),
                );
                workflow.gamecube_gamehacking_blocked = false;
            }
            return;
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        options.cancellation = Some(cancellation.clone());
        let archive_path = workflow.archive_path.clone();
        let generation = workflow.gamecube_gamehacking_generation.saturating_add(1);
        let Some(request_key) = dolphin_gamehacking_request_key(workflow, generation) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if let Some(previous) = workflow.gamecube_gamehacking_cancellation.take() {
            previous.store(true, Ordering::Relaxed);
        }
        workflow.gamecube_gamehacking_generation = generation;
        workflow.gamecube_gamehacking_request = Some(request_key.clone());
        workflow.gamecube_gamehacking_cancellation = Some(cancellation);
        workflow.gamecube_gamehacking = CheatStepResource::Loading { receiver };
        workflow.gamecube_gamehacking_blocked = false;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            format!(
                "Checking one local {} game against GameHacking.org.",
                if is_wii { "Wii" } else { "GameCube" }
            ),
        ));
        thread::spawn(move || {
            let started = Instant::now();
            log::debug!(
                "Wii/GameCube GameHacking task started: platform={} game_id={} generation={} cache_only={}",
                request_key.platform,
                request_key.game_id,
                request_key.generation,
                is_wii && wii_mode == WiiGameHackingFetchMode::CacheOnly
            );
            let result = (|| {
                if let Some(identity) = wii_identity {
                    let provider = GameHackingWiiProvider::default();
                    let outcome = provider.match_game_with_metrics(&identity, &options)?;
                    let matched = outcome.result;
                    let candidate_count = matched.candidates.len() + usize::from(matched.game.is_some());
                    log::debug!(
                        "Wii GameHacking cached match: game_id={} catalogue_rows={} candidates={}",
                        request_key.game_id,
                        outcome.catalogue_rows_examined,
                        candidate_count
                    );
                    let Some(game) = matched.game.clone() else {
                        return Ok(wii_match_state(matched, Vec::new(), false));
                    };
                    if wii_mode == WiiGameHackingFetchMode::CacheOnly {
                        let cheats = provider
                            .load_cached_game_page_cheats(&identity, &game, &options)?
                            .unwrap_or_default();
                        Ok(wii_match_state(matched, cheats, false))
                    } else {
                        let fetch = provider
                            .fetch_game_page_cheats_with_status(&identity, &game, false, &options)?;
                        Ok(wii_match_state(matched, fetch.data, fetch.cached_fallback))
                    }
                } else {
                    let identity = gamecube_identity.expect("identity variant checked above");
                    let provider = GameHackingGameCubeProvider::default();
                    let matched = provider.match_game(&identity, &options)?;
                    let Some(game) = matched.game.clone() else {
                        return Ok(GameCubeGameHackingState {
                            status: matched.status,
                            detail: matched.detail,
                            game: None,
                            match_candidates: matched.candidates,
                            selection: gamecube_gamehacking_selection_for(&[]),
                            cheats: Vec::new(),
                            cached_fallback: false,
                        });
                    };
                    let fetch = provider.fetch_cheats_with_status(&identity, &game, &options)?;
                    let cheats = fetch.data;
                    Ok(GameCubeGameHackingState {
                        status: matched.status,
                        detail: matched.detail,
                        game: Some(game),
                        match_candidates: Vec::new(),
                        selection: gamecube_gamehacking_selection_for(&cheats),
                        cheats,
                        cached_fallback: fetch.cached_fallback,
                    })
                }
            })()
            .map_err(|failure: archivefs_core::patch_manager::GameHackingError| {
                if failure.kind == GameHackingErrorKind::Cancelled {
                    log::debug!(
                        "Wii/GameCube GameHacking task cancelled: platform={} game_id={} generation={} elapsed_ms={}",
                        request_key.platform,
                        request_key.game_id,
                        request_key.generation,
                        started.elapsed().as_millis()
                    );
                } else {
                    log::debug!(
                        "Wii/GameCube GameHacking task error: platform={} game_id={} generation={} elapsed_ms={} error={}",
                        request_key.platform,
                        request_key.game_id,
                        request_key.generation,
                        started.elapsed().as_millis(),
                        failure
                    );
                }
                failure.to_string()
            });
            if let Ok(state) = &result {
                log::debug!(
                    "Wii/GameCube GameHacking task terminal: platform={} game_id={} generation={} status={:?} candidates={} cheats={} elapsed_ms={}",
                    request_key.platform,
                    request_key.game_id,
                    request_key.generation,
                    state.status,
                    state.match_candidates.len() + usize::from(state.game.is_some()),
                    state.cheats.len(),
                    started.elapsed().as_millis()
                );
            }
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn confirm_gamecube_gamehacking_match(&mut self, context: egui::Context, game_id: u64) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        if workflow.platform.as_deref() == Some("Wii") {
            self.confirm_wii_gamehacking_match(context, game_id);
            return;
        }
        let Some(identity) = gamecube_identity_for_workflow(workflow) else {
            return;
        };
        let CheatStepResource::Ready(state) = &workflow.gamecube_gamehacking else {
            return;
        };
        let Some(game) = state
            .match_candidates
            .iter()
            .find(|candidate| candidate.game.game_id == game_id)
            .map(|candidate| candidate.game.clone())
        else {
            return;
        };
        let options = match GameHackingGameCubeFetchOptions::defaults() {
            Ok(options) => options,
            Err(failure) => {
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.gamecube_gamehacking = CheatStepResource::Failed(failure.to_string());
                    workflow.gamecube_gamehacking_blocked = false;
                }
                return;
            }
        };
        let archive_path = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.gamecube_gamehacking = CheatStepResource::Loading { receiver };
        workflow.gamecube_gamehacking_blocked = false;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            format!("Downloading confirmed GameHacking.org game {game_id} GameCube export."),
        ));
        thread::spawn(move || {
            let provider = GameHackingGameCubeProvider::default();
            let result = (|| {
                let fetch = provider
                    .fetch_cheats_for_confirmed_candidate_with_status(&identity, &game, &options)?;
                let cheats = fetch.data;
                Ok(GameCubeGameHackingState {
                    status: GameHackingGameCubeMatchStatus::Matched,
                    detail: format!(
                        "Using user-confirmed GameHacking.org match: {} (game {}).",
                        game.title, game.game_id
                    ),
                    game: Some(game),
                    match_candidates: Vec::new(),
                    selection: gamecube_gamehacking_selection_for(&cheats),
                    cheats,
                    cached_fallback: fetch.cached_fallback,
                })
            })()
            .map_err(|failure: archivefs_core::patch_manager::GameHackingError| {
                failure.to_string()
            });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn confirm_wii_gamehacking_match(&mut self, context: egui::Context, game_id: u64) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let Some(identity) = wii_identity_for_workflow(workflow) else {
            return;
        };
        let CheatStepResource::Ready(state) = &workflow.gamecube_gamehacking else {
            return;
        };
        let Some(game) = state
            .match_candidates
            .iter()
            .find(|candidate| candidate.game.game_id == game_id)
            .map(|candidate| wii_game_from_dolphin_game(&candidate.game))
        else {
            return;
        };
        let options = match GameHackingGameCubeFetchOptions::defaults() {
            Ok(options) => options,
            Err(failure) => {
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.gamecube_gamehacking = CheatStepResource::Failed(failure.to_string());
                    workflow.gamecube_gamehacking_blocked = false;
                }
                return;
            }
        };
        let archive_path = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.gamecube_gamehacking = CheatStepResource::Loading { receiver };
        workflow.gamecube_gamehacking_blocked = false;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            format!("Loading confirmed GameHacking.org Wii game {game_id}."),
        ));
        thread::spawn(move || {
            let provider = GameHackingWiiProvider::default();
            let result = provider
                .fetch_game_page_cheats_with_status(&identity, &game, true, &options)
                .map(|fetch| {
                    wii_match_state(
                        GameHackingWiiMatch {
                            status: GameHackingWiiMatchStatus::Matched,
                            detail: format!(
                                "Using user-confirmed GameHacking.org match: {} (game {}).",
                                game.title, game.game_id
                            ),
                            game: Some(game),
                            candidates: Vec::new(),
                        },
                        fetch.data,
                        fetch.cached_fallback,
                    )
                })
                .map_err(|failure| failure.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn update_gamecube_gamehacking_cheat_selection(&mut self, index: usize, selected: bool) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let CheatStepResource::Ready(state) = &mut workflow.gamecube_gamehacking else {
            return;
        };
        state.selection.set_selected(index, selected);
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
    }

    // --- Browser-assisted GameHacking.org import ------------------------
    //
    // None of these methods make a network request. The only outward
    // action any of them takes is asking the desktop to open a validated
    // `https://gamehacking.org` URL, which is deliberately never treated
    // as a successful import.

    /// Opens the import panel for the currently selected GameHacking.org
    /// candidate.
    ///
    /// The candidate's numeric game ID is resolved by re-running the
    /// provider's *local* catalogue match (`match_game` reads only the
    /// already-cached catalogue file). That matters when live access is
    /// blocked: the failed fetch left no match result behind, but the
    /// match itself never needed the network.
    pub(crate) fn open_browser_import(&mut self, platform: BrowserImportPlatform) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let cache_root = match archivefs_core::patch_manager::gamehacking_cache_root() {
            Ok(cache_root) => cache_root,
            Err(failure) => {
                self.set_browser_import_failure(
                    "Cache unavailable".to_string(),
                    failure.to_string(),
                );
                return;
            }
        };
        let resolved = match platform {
            BrowserImportPlatform::GameCube => {
                self.resolve_gamecube_browser_import_target(workflow, &cache_root)
            }
            BrowserImportPlatform::PlayStation2 => {
                self.resolve_ps2_browser_import_target(workflow, &cache_root)
            }
        };
        let (identity, game_id, source_url, candidate_title) = match resolved {
            Ok(resolved) => resolved,
            Err((headline, detail)) => {
                self.set_browser_import_failure(headline, detail);
                return;
            }
        };
        match plan_gamehacking_browser_import(
            platform,
            game_id,
            source_url.as_deref(),
            &identity,
            &cache_root,
        ) {
            Ok(plan) => {
                let state = BrowserImportState::new(plan, identity, candidate_title);
                if let Some(workflow) = self.cheat_workflow.as_mut() {
                    workflow.browser_import_open_error = None;
                    workflow.browser_import = Some(state);
                }
            }
            Err(failure) => {
                self.set_browser_import_failure(
                    failure.kind.headline().to_string(),
                    failure.detail,
                );
            }
        }
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn resolve_gamecube_browser_import_target(
        &self,
        workflow: &CheatWorkflowState,
        cache_root: &Path,
    ) -> Result<(BrowserImportLocalIdentity, u64, Option<String>, String), (String, String)> {
        let identity = gamecube_identity_for_workflow(workflow).ok_or_else(|| {
            (
                "Local game identity incomplete".to_string(),
                "EmuWiz needs this GameCube game's verified Dolphin Game ID before it can check an imported page against it.".to_string(),
            )
        })?;
        let local = BrowserImportLocalIdentity::from_gamecube(&identity)
            .map_err(|failure| (failure.kind.headline().to_string(), failure.detail))?;
        // An already-matched candidate wins; otherwise the local
        // catalogue match is re-run, which never touches the network.
        if let CheatStepResource::Ready(state) = &workflow.gamecube_gamehacking
            && let Some(game) = &state.game
        {
            return Ok((
                local,
                game.game_id,
                Some(game.source_url.clone()),
                game.title.clone(),
            ));
        }
        let options = GameHackingGameCubeFetchOptions {
            cache_root: cache_root.to_path_buf(),
            force_refresh: false,
            delay: Duration::from_secs(0),
            cancellation: None,
        };
        let matched = GameHackingGameCubeProvider::default()
            .match_game(&identity, &options)
            .map_err(|failure| ("Cached catalogue unavailable".to_string(), failure.detail))?;
        let game = matched.game.ok_or_else(|| {
            (
                "No GameHacking candidate selected".to_string(),
                format!(
                    "{} Choose the correct GameHacking.org game first - an import is always checked against one exact candidate.",
                    matched.detail
                ),
            )
        })?;
        Ok((
            local,
            game.game_id,
            Some(game.source_url.clone()),
            game.title.clone(),
        ))
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn resolve_ps2_browser_import_target(
        &self,
        workflow: &CheatWorkflowState,
        cache_root: &Path,
    ) -> Result<(BrowserImportLocalIdentity, u64, Option<String>, String), (String, String)> {
        let identity = pcsx2_identity_for_workflow(workflow).ok_or_else(|| {
            (
                "Local game identity incomplete".to_string(),
                "EmuWiz needs this PS2 game's verified PCSX2 executable CRC before it can check an imported export against it.".to_string(),
            )
        })?;
        let local = BrowserImportLocalIdentity::from_ps2(&identity)
            .map_err(|failure| (failure.kind.headline().to_string(), failure.detail))?;
        if let CheatStepResource::Ready(state) = &workflow.pcsx2_gamehacking
            && let Some(game) = &state.game
        {
            return Ok((
                local,
                game.game_id,
                Some(game.source_url.clone()),
                game.title.clone(),
            ));
        }
        let options = GameHackingFetchOptions {
            cache_root: cache_root.to_path_buf(),
            force_refresh: false,
            delay: Duration::from_secs(0),
            cancellation: None,
        };
        let matched = GameHackingProvider::default()
            .match_game(&identity, &options)
            .map_err(|failure| ("Cached catalogue unavailable".to_string(), failure.detail))?;
        let game = matched.game.ok_or_else(|| {
            (
                "No GameHacking candidate selected".to_string(),
                format!(
                    "{} Choose the correct GameHacking.org game first - an import is always checked against one exact candidate.",
                    matched.detail
                ),
            )
        })?;
        Ok((
            local,
            game.game_id,
            Some(game.source_url.clone()),
            game.title.clone(),
        ))
    }

    /// Surfaces a browser-import failure without opening the panel, for
    /// the cases that stop it opening at all.
    pub(crate) fn set_browser_import_failure(&mut self, headline: String, detail: String) {
        if let Some(workflow) = self.cheat_workflow.as_mut() {
            workflow.browser_import_open_error = Some((headline, detail));
        }
    }

    pub(crate) fn close_browser_import(&mut self) {
        if let Some(workflow) = self.cheat_workflow.as_mut() {
            workflow.browser_import = None;
            workflow.browser_import_open_error = None;
        }
    }

    /// Hands the exact validated page URL to the desktop's default
    /// browser. On success the panel says so *and* says nothing has been
    /// imported yet, so a launch can never be mistaken for an import.
    pub(crate) fn open_gamehacking_page_in_browser(&mut self) {
        let Some(url) = self
            .cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.browser_import.as_ref())
            .map(|state| state.plan.expected_source_url.clone())
        else {
            return;
        };
        let outcome = open_gamehacking_url_in_browser(&url, &DesktopBrowserLauncher);
        let Some(state) = self
            .cheat_workflow
            .as_mut()
            .and_then(|workflow| workflow.browser_import.as_mut())
        else {
            return;
        };
        state.clear_result();
        match outcome {
            Ok(notice) => state.notice = Some(notice),
            Err(failure) => {
                state.failure = Some((failure.kind.headline().to_string(), failure.detail))
            }
        }
    }

    pub(crate) fn copy_gamehacking_page_url(&mut self) {
        let Some(url) = self
            .cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.browser_import.as_ref())
            .map(|state| state.plan.expected_source_url.clone())
        else {
            return;
        };
        let result = self.clipboard.set_text(url.clone());
        let Some(state) = self
            .cheat_workflow
            .as_mut()
            .and_then(|workflow| workflow.browser_import.as_mut())
        else {
            return;
        };
        state.clear_result();
        match result {
            Ok(()) => state.notice = Some(format!("Copied {url} to the clipboard.")),
            Err(reason) => {
                state.failure = Some((
                    BrowserImportErrorKind::ClipboardUnavailable
                        .headline()
                        .to_string(),
                    format!("EmuWiz could not write to the clipboard on this system: {reason}"),
                ))
            }
        }
    }

    /// Reads the clipboard exactly once, only because "Paste from
    /// clipboard" was clicked. Nothing is retained beyond this import.
    pub(crate) fn import_browser_clipboard(&mut self, context: egui::Context) {
        let status = self.clipboard.get_text_status();
        match status {
            ClipboardTextStatus::Ready(text) => {
                self.import_browser_content(
                    context,
                    BrowserImportSource::Text {
                        text,
                        origin: BrowserImportTextOrigin::Clipboard,
                    },
                );
            }
            ClipboardTextStatus::Empty => self.record_browser_import_failure(
                BrowserImportErrorKind::ClipboardEmpty.headline().to_string(),
                "The clipboard held no text. Copy the game page or Text export in your browser first."
                    .to_string(),
            ),
            ClipboardTextStatus::Unavailable(reason) => self.record_browser_import_failure(
                BrowserImportErrorKind::ClipboardUnavailable
                    .headline()
                    .to_string(),
                format!("EmuWiz could not read the clipboard on this system: {reason}"),
            ),
        }
    }

    pub(crate) fn import_browser_pasted_text(&mut self, context: egui::Context) {
        let Some(text) = self
            .cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.browser_import.as_ref())
            .map(|state| state.pasted.clone())
        else {
            return;
        };
        self.import_browser_content(
            context,
            BrowserImportSource::Text {
                text,
                origin: BrowserImportTextOrigin::PastedText,
            },
        );
    }

    /// Opens the native file picker for a saved page or export. `rfd`'s
    /// `pick_file` is synchronous and returns `None` on cancel, so a
    /// cancelled picker simply does nothing.
    pub(crate) fn import_browser_saved_file(&mut self, context: egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import a saved GameHacking.org page or export")
            .add_filter(
                "Saved page or cheat export",
                &["html", "htm", "txt", "pnach"],
            )
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        self.import_browser_content(context, BrowserImportSource::File(path));
    }

    /// Runs one validated import and, on success, refreshes the provider
    /// state so the normal preview/selection/install flow picks the
    /// imported cache up immediately.
    pub(crate) fn import_browser_content(&mut self, context: egui::Context, source: BrowserImportSource) {
        let Some(state) = self
            .cheat_workflow
            .as_ref()
            .and_then(|workflow| workflow.browser_import.as_ref())
        else {
            return;
        };
        let platform = state.plan.platform;
        let request = BrowserImportRequest {
            platform,
            game_id: state.plan.gamehacking_game_id,
            source_url: Some(state.plan.expected_source_url.clone()),
            candidate_title: if state.candidate_title.trim().is_empty() {
                state.plan.local_game_title.clone()
            } else {
                state.candidate_title.clone()
            },
            identity: state.identity.clone(),
            cache_root: match archivefs_core::patch_manager::gamehacking_cache_root() {
                Ok(cache_root) => cache_root,
                Err(failure) => {
                    self.record_browser_import_failure(
                        "Cache unavailable".to_string(),
                        failure.to_string(),
                    );
                    return;
                }
            },
            kind: state.kind,
            source,
        };
        match import_gamehacking_browser_content(&request) {
            Ok(outcome) => {
                let summary = format!(
                    "Browser import successful: {} cheat(s) from GameHacking game {} written to {}.",
                    outcome.cheat_count,
                    outcome.gamehacking_game_id,
                    outcome.cache_path.display()
                );
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatInstall,
                    self.cheat_workflow
                        .as_ref()
                        .map(|workflow| workflow.archive_path.clone()),
                    ActivityOutcome::Completed,
                    &summary,
                ));
                // The plan's "would this replace an existing cached
                // response?" facts are now stale - the import just wrote
                // there - so they are recomputed against the real cache.
                let refreshed_plan = plan_gamehacking_browser_import(
                    platform,
                    request.game_id,
                    request.source_url.as_deref(),
                    &request.identity,
                    &request.cache_root,
                )
                .ok();
                if let Some(state) = self
                    .cheat_workflow
                    .as_mut()
                    .and_then(|workflow| workflow.browser_import.as_mut())
                {
                    state.clear_result();
                    state.pasted.clear();
                    state.paste_open = false;
                    state.outcome = Some(outcome);
                    if let Some(plan) = refreshed_plan {
                        state.plan = plan;
                    }
                }
                // The whole point: the ordinary provider flow continues
                // from the imported cache, with no special case.
                match platform {
                    BrowserImportPlatform::GameCube => {
                        self.start_gamecube_gamehacking_fetch(context, false)
                    }
                    BrowserImportPlatform::PlayStation2 => {
                        self.start_pcsx2_gamehacking_fetch(context, false)
                    }
                }
            }
            Err(failure) => {
                self.record_browser_import_failure(
                    failure.kind.headline().to_string(),
                    failure.detail,
                );
            }
        }
    }

    pub(crate) fn record_browser_import_failure(&mut self, headline: String, detail: String) {
        if let Some(state) = self
            .cheat_workflow
            .as_mut()
            .and_then(|workflow| workflow.browser_import.as_mut())
        {
            state.notice = None;
            state.outcome = None;
            state.failure = Some((headline, detail));
        }
    }

    /// Resolves the Dolphin profile's own configuration root the same way
    /// `start_dolphin_install_preview` does - GameHacking.org GameCube
    /// installs write into exactly the same profile.
    pub(crate) fn gamecube_gamehacking_profile(&self) -> Option<DolphinProfile> {
        let workflow = self.cheat_workflow.as_ref()?;
        let profile_id = workflow.selected_dolphin_profile_id.as_ref()?;
        let DolphinProfilesState::Ready(discovery) = &self.dolphin_profiles else {
            return None;
        };
        discovery
            .profiles
            .iter()
            .find(|profile| profile.eligible && &profile.profile_id == profile_id)
            .cloned()
    }

    /// GameHacking.org GameCube Stage: stages the selected `ActionReplay`/
    /// `Gecko` cheats into the real Dolphin GameSettings file (preserving
    /// every other section byte-for-byte) and builds its shared install
    /// preview. Synchronous for the same reason
    /// `start_dolphin_install_preview` is - a single small local file.
    pub(crate) fn start_gamecube_gamehacking_install_preview(&mut self) {
        let is_wii = self
            .cheat_workflow
            .as_ref()
            .is_some_and(|workflow| workflow.platform.as_deref() == Some("Wii"));
        let staging_root = match default_generated_dolphin_gamehacking_staging_root(is_wii) {
            Ok(root) => root,
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    self.cheat_workflow
                        .as_ref()
                        .map(|workflow| workflow.archive_path.clone()),
                    ActivityOutcome::Failed,
                    message,
                ));
                return;
            }
        };
        self.start_gamecube_gamehacking_install_preview_with_staging_root(staging_root);
    }

    pub(crate) fn start_gamecube_gamehacking_install_preview_with_staging_root(
        &mut self,
        staging_root: PathBuf,
    ) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let CheatStepResource::Ready(state) = &workflow.gamecube_gamehacking else {
            return;
        };
        let Some(game) = state.game.clone() else {
            return;
        };
        let Some(game_id) = game.dolphin_game_id.clone() else {
            self.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(workflow.archive_path.clone()),
                ActivityOutcome::Rejected,
                "Install preview blocked: this GameHacking.org game has no verified Dolphin Game ID.",
            ));
            return;
        };
        let Some(profile) = self.gamecube_gamehacking_profile() else {
            self.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(workflow.archive_path.clone()),
                ActivityOutcome::Rejected,
                "Install preview blocked: the selected Dolphin profile is no longer eligible.",
            ));
            return;
        };
        let configuration_path = profile.configuration_path.clone();
        let cheats = state.cheats.clone();
        let selection = state.selection.clone();
        let archive_path = workflow.archive_path.clone();
        let is_wii = workflow.platform.as_deref() == Some("Wii");
        let key = cheat_preview_key(workflow);
        let response = (|| {
            let destination =
                load_dolphin_destination(&configuration_path, &game_id).map_err(|failure| {
                    GameCubeInstallPlanError {
                        kind: GameCubeInstallPlanErrorKind::SelectionInvalid,
                        cheat_name: None,
                        detail: failure.to_string(),
                    }
                })?;
            let staged = stage_gamecube_gamehacking_install(
                &staging_root,
                &format!("{game_id}.ini"),
                &destination.document,
                destination.existed,
                &cheats,
                &selection,
            )?;
            let request = GameCubeGameHackingInstallPreviewRequest {
                selected_archive: archive_path.clone(),
                configuration_path: configuration_path.clone(),
                game_id: game_id.clone(),
                revision: None,
                staged: staged.clone(),
            };
            let preview = if is_wii {
                build_wii_gamehacking_install_preview(&request)?
            } else {
                build_gamecube_gamehacking_install_preview(&request)?
            };
            Ok::<_, GameCubeInstallPlanError>((preview, staged))
        })();
        let message = match response {
            Ok((preview, staged)) => CheatPreviewResponse {
                key: key.clone(),
                outcome: CheatPreviewOutcome::Ready(preview.report),
                materialized: None,
                generated: None,
                dolphin_generated: None,
                xenia_generated: None,
                pcsx2_generated: None,
                gamecube_gamehacking_generated: Some(GeneratedGameCubeGameHackingInstall {
                    staging_root,
                    staged,
                    profile,
                }),
                bsfree_gamecube_generated: None,

                bsfree_wii_generated: None,
            },
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    format!("Install preview failed: {}", error.detail),
                ));
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Failed(
                        CheatPreviewFailure::GameCubeGameHackingInstallPlan(error),
                    ),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                }
            }
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Ready(message);
        workflow.transaction = CheatTransactionState::Idle;
        workflow.transaction_notice = None;
        self.review_cheat_apply();
    }

    /// GameHacking.org GameCube removal: stages removal of exactly the
    /// selected, already-EmuWiz-managed cheats from the real Dolphin
    /// GameSettings file, reusing the same shared preview/apply/rollback
    /// pipeline as install.
    pub(crate) fn start_gamecube_gamehacking_removal_preview(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let CheatStepResource::Ready(state) = &workflow.gamecube_gamehacking else {
            return;
        };
        let Some(game) = state.game.clone() else {
            return;
        };
        let Some(game_id) = game.dolphin_game_id.clone() else {
            return;
        };
        let Some(profile) = self.gamecube_gamehacking_profile() else {
            self.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(workflow.archive_path.clone()),
                ActivityOutcome::Rejected,
                "Removal preview blocked: the selected Dolphin profile is no longer eligible.",
            ));
            return;
        };
        let configuration_path = profile.configuration_path.clone();
        let remove_names: Vec<String> = state
            .selection
            .entries
            .iter()
            .filter(|entry| entry.selected && entry.already_managed)
            .map(|entry| entry.dolphin_name.clone())
            .collect();
        let archive_path = workflow.archive_path.clone();
        let is_wii = workflow.platform.as_deref() == Some("Wii");
        let key = cheat_preview_key(workflow);
        let staging_root = match default_generated_dolphin_gamehacking_staging_root(is_wii) {
            Ok(root) => root,
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    message,
                ));
                return;
            }
        };
        let response = (|| {
            let destination =
                load_dolphin_destination(&configuration_path, &game_id).map_err(|failure| {
                    GameCubeInstallPlanError {
                        kind: GameCubeInstallPlanErrorKind::SelectionInvalid,
                        cheat_name: None,
                        detail: failure.to_string(),
                    }
                })?;
            let staged = stage_gamecube_gamehacking_removal(
                &staging_root,
                &format!("{game_id}.ini"),
                &destination.document,
                destination.existed,
                &remove_names,
            )?;
            let request = GameCubeGameHackingInstallPreviewRequest {
                selected_archive: archive_path.clone(),
                configuration_path: configuration_path.clone(),
                game_id: game_id.clone(),
                revision: None,
                staged: staged.clone(),
            };
            let preview = if is_wii {
                build_wii_gamehacking_install_preview(&request)?
            } else {
                build_gamecube_gamehacking_install_preview(&request)?
            };
            Ok::<_, GameCubeInstallPlanError>((preview, staged))
        })();
        let message = match response {
            Ok((preview, staged)) => CheatPreviewResponse {
                key: key.clone(),
                outcome: CheatPreviewOutcome::Ready(preview.report),
                materialized: None,
                generated: None,
                dolphin_generated: None,
                xenia_generated: None,
                pcsx2_generated: None,
                gamecube_gamehacking_generated: Some(GeneratedGameCubeGameHackingInstall {
                    staging_root,
                    staged,
                    profile,
                }),
                bsfree_gamecube_generated: None,

                bsfree_wii_generated: None,
            },
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    format!("Removal preview failed: {}", error.detail),
                ));
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Failed(
                        CheatPreviewFailure::GameCubeGameHackingInstallPlan(error),
                    ),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                }
            }
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Ready(message);
        workflow.transaction = CheatTransactionState::Idle;
        workflow.transaction_notice = None;
        self.review_cheat_apply();
    }

    /// Searches the optional BSFree Archive SQLite database for the selected
    /// GameCube game, on a background thread (opening the immutable source
    /// re-validates its pinned SHA-256, which is too heavy for the UI thread).
    /// Exactly one match auto-loads its classified cheats; several are shown
    /// as candidates for explicit confirmation; none yields a search box.
    pub(crate) fn start_bsfree_gamecube_search(&mut self, context: egui::Context, search_title: String) {
        let Some((archive_path, game_id, region, platform_ok)) =
            self.cheat_workflow.as_ref().and_then(|workflow| {
                let identity = gamecube_identity_for_workflow(workflow)?;
                Some((
                    workflow.archive_path.clone(),
                    identity.dolphin_game_id.clone()?,
                    identity.region.clone(),
                    workflow.platform.as_deref() == Some("GameCube"),
                ))
            })
        else {
            return;
        };
        if !platform_ok {
            return;
        }
        let generation = self
            .cheat_workflow
            .as_ref()
            .map(|workflow| workflow.bsfree_gamecube_generation.saturating_add(1))
            .unwrap_or(1);
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if let Some(previous) = workflow.bsfree_gamecube_cancellation.take() {
            previous.store(true, Ordering::Relaxed);
        }
        workflow.bsfree_gamecube_generation = generation;
        workflow.bsfree_gamecube_cancellation = Some(Arc::new(AtomicBool::new(false)));
        workflow.bsfree_gamecube = CheatStepResource::Loading { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            "Searching the local BSFree Archive for this GameCube game.".to_string(),
        ));
        thread::spawn(move || {
            let result = (|| -> Result<BsFreeGameCubeGuiState, String> {
                let catalogue = open_installed_bsfree_catalogue()?;
                let outcome =
                    bsfree_gamecube_search(&catalogue, &search_title, &game_id, region.as_deref())
                        .map_err(|error| error.to_string())?;
                Ok(bsfree_gui_state_from_outcome(outcome, search_title))
            })();
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Loads the classified cheats for a BSFree GameCube game the user
    /// explicitly confirmed from the search candidates.
    pub(crate) fn start_bsfree_gamecube_confirm(&mut self, context: egui::Context, upstream_uid: i64) {
        let Some((archive_path, game_id, region, archive_title, platform_ok)) =
            self.cheat_workflow.as_ref().and_then(|workflow| {
                let identity = gamecube_identity_for_workflow(workflow)?;
                Some((
                    workflow.archive_path.clone(),
                    identity.dolphin_game_id.clone()?,
                    identity.region.clone(),
                    workflow.display_name.clone(),
                    workflow.platform.as_deref() == Some("GameCube"),
                ))
            })
        else {
            return;
        };
        if !platform_ok {
            return;
        }
        let generation = self
            .cheat_workflow
            .as_ref()
            .map(|workflow| workflow.bsfree_gamecube_generation.saturating_add(1))
            .unwrap_or(1);
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if let Some(previous) = workflow.bsfree_gamecube_cancellation.take() {
            previous.store(true, Ordering::Relaxed);
        }
        workflow.bsfree_gamecube_generation = generation;
        workflow.bsfree_gamecube_cancellation = Some(Arc::new(AtomicBool::new(false)));
        workflow.bsfree_gamecube = CheatStepResource::Loading { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            "Loading the confirmed BSFree GameCube game's cheats.".to_string(),
        ));
        thread::spawn(move || {
            let result = (|| -> Result<BsFreeGameCubeGuiState, String> {
                let catalogue = open_installed_bsfree_catalogue()?;
                let outcome = bsfree_gamecube_load_confirmed(
                    &catalogue,
                    upstream_uid,
                    &archive_title,
                    &game_id,
                    region.as_deref(),
                )
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    "The confirmed BSFree game is no longer in the catalogue.".to_string()
                })?;
                Ok(bsfree_gui_state_from_outcome(outcome, archive_title))
            })();
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn update_bsfree_gamecube_cheat_selection(&mut self, index: usize, selected: bool) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let CheatStepResource::Ready(state) = &mut workflow.bsfree_gamecube else {
            return;
        };
        if !state.selection.set_selected(index, selected) {
            return;
        }
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
        workflow.transaction_notice = None;
    }

    pub(crate) fn update_bsfree_gamecube_cheat_selection_all(&mut self, selected: bool) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let CheatStepResource::Ready(state) = &mut workflow.bsfree_gamecube else {
            return;
        };
        if selected {
            state.selection.select_all();
        } else {
            state.selection.clear_all();
        }
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
        workflow.transaction_notice = None;
    }

    /// BSFree GameCube Stage: stages the selected supported cheats into the
    /// real Dolphin GameSettings file (preserving every other section
    /// byte-for-byte) and builds its shared install preview, exactly like the
    /// GameHacking.org GameCube path. Synchronous - a single small local file.
    pub(crate) fn start_bsfree_gamecube_install_preview(&mut self) {
        let Some(staging_root) = default_generated_dolphin_gamehacking_staging_root(false).ok()
        else {
            return;
        };
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let CheatStepResource::Ready(state) = &workflow.bsfree_gamecube else {
            return;
        };
        let Some(game) = state.game.clone() else {
            return;
        };
        let Some(profile) = self.gamecube_gamehacking_profile() else {
            self.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(workflow.archive_path.clone()),
                ActivityOutcome::Rejected,
                "BSFree install preview blocked: the selected Dolphin profile is no longer eligible.",
            ));
            return;
        };
        let game_id = game.archive_game_id.clone();
        let configuration_path = profile.configuration_path.clone();
        let cheats = state.cheats.clone();
        let selection = state.selection.clone();
        let archive_path = workflow.archive_path.clone();
        let key = cheat_preview_key(workflow);
        let response = (|| {
            let destination =
                load_dolphin_destination(&configuration_path, &game_id).map_err(|failure| {
                    BsFreeGameCubeError {
                        kind: BsFreeGameCubeErrorKind::SelectionInvalid,
                        cheat_name: None,
                        detail: failure.to_string(),
                    }
                })?;
            let staged = stage_bsfree_gamecube_install(
                &staging_root,
                &format!("{game_id}.ini"),
                &destination.document,
                destination.existed,
                &cheats,
                &selection,
            )?;
            let request = BsFreeGameCubeInstallPreviewRequest {
                selected_archive: archive_path.clone(),
                configuration_path: configuration_path.clone(),
                game_id: game_id.clone(),
                revision: None,
                staged: staged.staged.clone(),
            };
            let preview = build_bsfree_gamecube_install_preview(&request)?;
            Ok::<_, BsFreeGameCubeError>((preview, staged))
        })();
        let message = match response {
            Ok((preview, staged)) => CheatPreviewResponse {
                key: key.clone(),
                outcome: CheatPreviewOutcome::Ready(preview.report),
                materialized: None,
                generated: None,
                dolphin_generated: None,
                xenia_generated: None,
                pcsx2_generated: None,
                gamecube_gamehacking_generated: None,
                bsfree_gamecube_generated: Some(GeneratedBsFreeGameCubeInstall {
                    staging_root,
                    staged: staged.staged,
                    profile,
                    findings: staged.findings,
                    skipped_duplicates: staged.skipped_duplicates,
                    skipped_unselectable: staged.skipped_unselectable,
                }),
                bsfree_wii_generated: None,
            },
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    format!("BSFree install preview failed: {}", error.detail),
                ));
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Failed(
                        CheatPreviewFailure::BsFreeGameCubeInstallPlan(error),
                    ),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                }
            }
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Ready(message);
        workflow.transaction = CheatTransactionState::Idle;
        workflow.transaction_notice = None;
        self.review_cheat_apply();
    }

    /// Searches the local BSFree catalogue for a Wii game matching the given
    /// title, gated on the archive's verified Dolphin Wii Game ID. The shipped
    /// BSFree snapshot contains no Wii rows, so this normally resolves to
    /// `NoMatch`; the path is implemented so it activates if Wii data is
    /// loaded.
    pub(crate) fn start_bsfree_wii_search(&mut self, context: egui::Context, search_title: String) {
        let Some((archive_path, game_id, region, platform_ok)) =
            self.cheat_workflow.as_ref().and_then(|workflow| {
                let identity = wii_identity_for_workflow(workflow)?;
                Some((
                    workflow.archive_path.clone(),
                    identity.dolphin_game_id.clone()?,
                    identity.region.clone(),
                    workflow.platform.as_deref() == Some("Wii"),
                ))
            })
        else {
            return;
        };
        if !platform_ok {
            return;
        }
        let generation = self
            .cheat_workflow
            .as_ref()
            .map(|workflow| workflow.bsfree_wii_generation.saturating_add(1))
            .unwrap_or(1);
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if let Some(previous) = workflow.bsfree_wii_cancellation.take() {
            previous.store(true, Ordering::Relaxed);
        }
        workflow.bsfree_wii_generation = generation;
        workflow.bsfree_wii_cancellation = Some(Arc::new(AtomicBool::new(false)));
        workflow.bsfree_wii = CheatStepResource::Loading { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            "Searching the local BSFree Archive for this Wii game.".to_string(),
        ));
        thread::spawn(move || {
            let result = (|| -> Result<BsFreeWiiGuiState, String> {
                let catalogue = open_installed_bsfree_catalogue()?;
                let outcome =
                    bsfree_wii_search(&catalogue, &search_title, &game_id, region.as_deref())
                        .map_err(|error| error.to_string())?;
                Ok(bsfree_wii_gui_state_from_outcome(outcome, search_title))
            })();
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Loads the classified cheats for a BSFree Wii game the user explicitly
    /// confirmed from the search candidates.
    pub(crate) fn start_bsfree_wii_confirm(&mut self, context: egui::Context, upstream_uid: i64) {
        let Some((archive_path, _game_id, _region, archive_title, platform_ok)) =
            self.cheat_workflow.as_ref().and_then(|workflow| {
                let identity = wii_identity_for_workflow(workflow)?;
                Some((
                    workflow.archive_path.clone(),
                    identity.dolphin_game_id.clone()?,
                    identity.region.clone(),
                    workflow.display_name.clone(),
                    workflow.platform.as_deref() == Some("Wii"),
                ))
            })
        else {
            return;
        };
        if !platform_ok {
            return;
        }
        let generation = self
            .cheat_workflow
            .as_ref()
            .map(|workflow| workflow.bsfree_wii_generation.saturating_add(1))
            .unwrap_or(1);
        let (sender, receiver) = mpsc::channel();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if let Some(previous) = workflow.bsfree_wii_cancellation.take() {
            previous.store(true, Ordering::Relaxed);
        }
        workflow.bsfree_wii_generation = generation;
        workflow.bsfree_wii_cancellation = Some(Arc::new(AtomicBool::new(false)));
        workflow.bsfree_wii = CheatStepResource::Loading { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            "Loading the confirmed BSFree Wii game's cheats.".to_string(),
        ));
        thread::spawn(move || {
            let result = (|| -> Result<BsFreeWiiGuiState, String> {
                let catalogue = open_installed_bsfree_catalogue()?;
                let cheats = bsfree_wii_load_confirmed(&catalogue, upstream_uid)
                    .map_err(|error| error.to_string())?;
                Ok(bsfree_wii_gui_state_from_matched(
                    upstream_uid,
                    &archive_title,
                    cheats,
                ))
            })();
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn update_bsfree_wii_cheat_selection(&mut self, index: usize, selected: bool) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let CheatStepResource::Ready(state) = &mut workflow.bsfree_wii else {
            return;
        };
        if !state.selection.set_selected(index, selected) {
            return;
        }
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
        workflow.transaction_notice = None;
    }

    pub(crate) fn update_bsfree_wii_cheat_selection_all(&mut self, selected: bool) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let CheatStepResource::Ready(state) = &mut workflow.bsfree_wii else {
            return;
        };
        if selected {
            state.selection.select_all();
        } else {
            state.selection.clear_all();
        }
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
        workflow.transaction_notice = None;
    }

    /// BSFree Wii Stage: stages the selected supported cheats into the real
    /// Dolphin GameSettings file through the shared Wii adapter and builds its
    /// shared install preview, exactly like the GameCube path. Synchronous.
    pub(crate) fn start_bsfree_wii_install_preview(&mut self) {
        let Some(staging_root) = default_generated_dolphin_gamehacking_staging_root(false).ok()
        else {
            return;
        };
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let CheatStepResource::Ready(state) = &workflow.bsfree_wii else {
            return;
        };
        let Some(game) = state.game.clone() else {
            return;
        };
        let Some(profile) = self.gamecube_gamehacking_profile() else {
            self.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(workflow.archive_path.clone()),
                ActivityOutcome::Rejected,
                "BSFree Wii install preview blocked: the selected Dolphin profile is no longer eligible.",
            ));
            return;
        };
        let game_id = game.archive_game_id.clone();
        let configuration_path = profile.configuration_path.clone();
        let cheats = state.cheats.clone();
        let selection = state.selection.clone();
        let archive_path = workflow.archive_path.clone();
        let key = cheat_preview_key(workflow);
        let response = (|| {
            let destination =
                load_dolphin_destination(&configuration_path, &game_id).map_err(|failure| {
                    BsFreeWiiError {
                        kind: BsFreeWiiErrorKind::SelectionInvalid,
                        cheat_name: None,
                        detail: failure.to_string(),
                    }
                })?;
            let staged = stage_bsfree_wii_install(
                &staging_root,
                &format!("{game_id}.ini"),
                &destination.document,
                destination.existed,
                &cheats,
                &selection,
            )?;
            let request = BsFreeWiiInstallPreviewRequest {
                selected_archive: archive_path.clone(),
                configuration_path: configuration_path.clone(),
                game_id: game_id.clone(),
                revision: None,
                staged: staged.staged.clone(),
            };
            let preview = build_bsfree_wii_install_preview(&request)?;
            Ok::<_, BsFreeWiiError>((preview, staged))
        })();
        let message = match response {
            Ok((preview, staged)) => CheatPreviewResponse {
                key: key.clone(),
                outcome: CheatPreviewOutcome::Ready(preview.report),
                materialized: None,
                generated: None,
                dolphin_generated: None,
                xenia_generated: None,
                pcsx2_generated: None,
                gamecube_gamehacking_generated: None,
                bsfree_gamecube_generated: None,
                bsfree_wii_generated: Some(GeneratedBsFreeWiiInstall {
                    staging_root,
                    staged: staged.staged,
                    profile,
                    findings: staged.findings,
                    skipped_duplicates: staged.skipped_duplicates,
                    skipped_unselectable: staged.skipped_unselectable,
                }),
            },
            Err(error) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    format!("BSFree Wii install preview failed: {}", error.detail),
                ));
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Failed(
                        CheatPreviewFailure::BsFreeGameCubeInstallPlan(BsFreeGameCubeError {
                            kind: BsFreeGameCubeErrorKind::PreviewFailed,
                            cheat_name: error.cheat_name,
                            detail: error.detail,
                        }),
                    ),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,
                    bsfree_wii_generated: None,
                }
            }
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Ready(message);
        workflow.transaction = CheatTransactionState::Idle;
        workflow.transaction_notice = None;
        self.review_cheat_apply();
    }

    pub(crate) fn update_pcsx2_cheat_selection(&mut self, id: &str, selected: bool) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let CheatStepResource::Ready(provider) = &mut workflow.pcsx2_gamehacking else {
            return;
        };
        if selected {
            provider.selection.selected_ids.insert(id.to_string());
        } else {
            provider.selection.selected_ids.remove(id);
        }
        workflow.preview = CheatStepResource::NotLoaded;
        workflow.preview_request = None;
        workflow.transaction = CheatTransactionState::Idle;
    }

    pub(crate) fn start_pcsx2_install_preview(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_ref() else {
            return;
        };
        let Some(identity) = pcsx2_identity_for_workflow(workflow) else {
            return;
        };
        let CheatStepResource::Ready(provider) = &workflow.pcsx2_gamehacking else {
            return;
        };
        let selected =
            match selected_pcsx2_managed_cheats(&provider.candidates, &provider.selection) {
                Ok(selected) => selected,
                Err(message) => {
                    self.history.record(HistoryEntry::new(
                        ActivityAction::CheatPreview,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Rejected,
                        message,
                    ));
                    return;
                }
            };
        let Some(profile_id) = workflow.selected_pcsx2_profile_id.clone() else {
            return;
        };
        let profile = match &self.pcsx2_profiles {
            Pcsx2ProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == profile_id)
                .cloned(),
            _ => None,
        };
        let Some(profile) = profile else {
            return;
        };
        let Ok(cache_root) = archivefs_core::patch_manager::gamehacking_cache_root() else {
            return;
        };
        let staging_root = cache_root.join("staging").join(format!(
            "{}-{}",
            std::process::id(),
            generate_shared_operation_id()
        ));
        log::info!(
            "pcsx2 install: {} cheat(s) selected, profile {:?} ({:?}), staging root {}",
            selected.len(),
            profile.profile_id,
            profile.installation_type,
            staging_root.display(),
        );
        let key = cheat_preview_key(workflow);
        let response = (|| {
            let crc =
                identity.verified_crc().ok_or_else(|| {
                    Pcsx2InstallPlanError {
                kind: archivefs_core::patch_manager::Pcsx2InstallPlanErrorKind::IdentityUnavailable,
                path: Some(identity.archive_path.clone()),
                detail: "verified PCSX2 CRC is required".to_string(),
            }
                })?;
            let staged = stage_pcsx2_pnach(
                &staging_root,
                &profile,
                identity.serial.as_deref(),
                crc,
                &selected,
            )?;
            let legacy_migration_report = build_pcsx2_legacy_migration_preview(
                &staged,
                &profile,
                &workflow.archive_path,
                crc,
            )?
            .map(|legacy_preview| legacy_preview.report);
            let preview = build_pcsx2_install_preview(&Pcsx2InstallPreviewRequest {
                selected_archive: workflow.archive_path.clone(),
                profile,
                identity,
                staged: staged.clone(),
            })?;
            Ok::<_, Pcsx2InstallPlanError>((preview, staged, legacy_migration_report))
        })();
        let message = match response {
            Ok((preview, staged, legacy_migration_report)) => {
                log::info!(
                    "pcsx2 install: staged pnach for {} at {} ({} byte(s)); target {}",
                    key.archive_path.display(),
                    staged.path.display(),
                    staged.contents.len(),
                    staged.destination_path.display(),
                );
                if let Some(migration) = &staged.legacy_migration {
                    log::info!(
                        "pcsx2 install: legacy file {} will be migrated ({} cheat id(s): {})",
                        migration.legacy_destination_path.display(),
                        migration.migrated_block_ids.len(),
                        migration.migrated_block_ids.join(", "),
                    );
                }
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Ready(preview.report),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: Some(GeneratedPcsx2Install {
                        staging_root,
                        legacy_migration_report,
                    }),
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                }
            }
            Err(failure) => {
                log::warn!(
                    "pcsx2 install: build-preview failed for {}: {:?} ({})",
                    key.archive_path.display(),
                    failure.kind,
                    failure.detail,
                );
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(key.archive_path.clone()),
                    ActivityOutcome::Failed,
                    format!(
                        "PCSX2 cheat install could not be prepared: {}",
                        failure.detail
                    ),
                ));
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::Pcsx2InstallPlan(
                        failure,
                    )),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,

                    bsfree_wii_generated: None,
                }
            }
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Ready(message);
        workflow.transaction = CheatTransactionState::Idle;
        self.review_cheat_apply();
    }

    pub(crate) fn start_pcsx2_inventory(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if workflow.adapter != CheatEmulatorAdapter::Pcsx2 {
            return;
        }
        let Some(profile_id) = workflow.selected_pcsx2_profile_id.clone() else {
            return;
        };
        let profile = match &self.pcsx2_profiles {
            Pcsx2ProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == profile_id)
                .cloned(),
            _ => None,
        };
        let Some(profile) = profile else {
            workflow.pcsx2_inventory_profile_id = Some(profile_id);
            workflow.pcsx2_inventory = CheatStepResource::Failed(
                "The selected PCSX2 profile is no longer eligible.".to_string(),
            );
            return;
        };
        let archive_path = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        workflow.pcsx2_inventory_profile_id = Some(profile_id.clone());
        workflow.pcsx2_inventory = CheatStepResource::Loading { receiver };
        workflow.pcsx2_activation = CheatActivationReadiness::Unknown;
        let (activation_sender, activation_receiver) = mpsc::channel();
        workflow.pcsx2_activation_receiver = Some(activation_receiver);
        workflow.preview_request = None;
        workflow.preview = CheatStepResource::NotLoaded;
        self.history.record(HistoryEntry::new(
            ActivityAction::Pcsx2PnachInspection,
            Some(archive_path),
            ActivityOutcome::Started,
            format!("PCSX2 PNACH inspection started for profile '{profile_id}'."),
        ));
        thread::spawn(move || {
            let result =
                inspect_pcsx2_profile_with_activation(&profile).map_err(|error| error.to_string());
            match result {
                Ok(inspection) => {
                    let _ = activation_sender.send(Ok(inspection.cheats_enabled));
                    let _ = sender.send(Ok(inspection.inventory));
                }
                Err(error) => {
                    let message = error.clone();
                    let _ = activation_sender.send(Err(message));
                    let _ = sender.send(Err(error));
                }
            }
            context.request_repaint();
        });
    }

    pub(crate) fn start_dolphin_inventory(&mut self, context: egui::Context) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if workflow.adapter != CheatEmulatorAdapter::Dolphin {
            return;
        }
        let Some(profile_id) = workflow.selected_dolphin_profile_id.clone() else {
            return;
        };
        let profile = match &self.dolphin_profiles {
            DolphinProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == profile_id)
                .cloned(),
            _ => None,
        };
        let Some(profile) = profile else {
            workflow.dolphin_inventory_profile_id = Some(profile_id);
            workflow.dolphin_inventory = CheatStepResource::Failed(
                "The selected Dolphin profile is no longer eligible.".to_string(),
            );
            return;
        };
        let archive_path = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        workflow.dolphin_inventory_profile_id = Some(profile_id.clone());
        workflow.dolphin_inventory = CheatStepResource::Loading { receiver };
        workflow.dolphin_activation = CheatActivationReadiness::Unknown;
        let (activation_sender, activation_receiver) = mpsc::channel();
        workflow.dolphin_activation_receiver = Some(activation_receiver);
        workflow.preview_request = None;
        workflow.preview = CheatStepResource::NotLoaded;
        self.history.record(HistoryEntry::new(
            ActivityAction::DolphinGameIniInspection,
            Some(archive_path),
            ActivityOutcome::Started,
            format!("Dolphin GameSettings inspection started for profile '{profile_id}'."),
        ));
        thread::spawn(move || {
            let result = inspect_dolphin_profile_with_activation(&profile)
                .map_err(|error| error.to_string());
            match result {
                Ok(inspection) => {
                    let _ = activation_sender.send(Ok(inspection.cheats_enabled));
                    let _ = sender.send(Ok(inspection.inventory));
                }
                Err(error) => {
                    let message = error.clone();
                    let _ = activation_sender.send(Err(message));
                    let _ = sender.send(Err(error));
                }
            }
            context.request_repaint();
        });
    }

    /// Retrieves the selected trusted source's catalogue in the
    /// background - a real network fetch, or offline reuse of the
    /// cached snapshot when `offline` is set. All size/digest/redirect
    /// protections live in `fetch_retroarch_cheat_source`; the GUI adds
    /// nothing and bypasses nothing.
    pub(crate) fn start_cheat_source_fetch(&mut self, context: egui::Context, offline: bool) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let Some(source_id) = workflow.selected_source_id.clone() else {
            return;
        };
        let archive_path = workflow.archive_path.clone();
        let force_refresh = workflow.fetch_force_refresh && !offline;
        let (sender, receiver) = mpsc::channel();
        workflow.source_fetch = CheatStepResource::Loading { receiver };
        clear_cheat_candidate_state(workflow);
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatSourceRetrieval,
            Some(archive_path),
            ActivityOutcome::Started,
            if offline {
                format!("Cheat source '{source_id}': reusing cached snapshot (offline).")
            } else {
                format!("Cheat source '{source_id}': catalogue retrieval started.")
            },
        ));
        thread::spawn(move || {
            let result = default_cheat_source_cache_root()
                .map_err(|error| error.to_string())
                .and_then(|cache_root| {
                    let options = CheatSourceFetchOptions {
                        cache_root,
                        force_refresh,
                        offline,
                        expected_sha256: None,
                        max_download_bytes: None,
                        cancellation: None,
                        progress: None,
                    };
                    let transport = HttpsCheatSourceTransport::new();
                    fetch_retroarch_cheat_source(&source_id, &options, &transport)
                        .map_err(|error| error.to_string())
                });
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Polls the cheat workflow's background resources. A retrieval
    /// result whose source no longer matches the current selection is
    /// discarded (request-identity check); a superseded receiver was
    /// already dropped when its state was replaced, so its result can
    /// never arrive here at all.
    pub(crate) fn poll_cheat_workflow(&mut self, context: &egui::Context) {
        let identity_page_is_current = self.view == MainView::CheatsMods;
        let mut automatic_candidate: Option<String> = None;
        let dolphin_profile_paths: HashMap<String, PathBuf> = match &self.dolphin_profiles {
            DolphinProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .filter(|profile| profile.eligible)
                .map(|profile| {
                    (
                        profile.profile_id.clone(),
                        profile.configuration_path.clone(),
                    )
                })
                .collect(),
            _ => HashMap::new(),
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if !identity_page_is_current {
            workflow.identity_request = None;
            workflow.identity = CheatStepResource::NotLoaded;
            workflow.preview_request = None;
            workflow.preview = CheatStepResource::NotLoaded;
        } else if let CheatStepResource::Loading { receiver } = &workflow.identity {
            match receiver.try_recv() {
                Ok(Ok((request, report))) => {
                    let current = GameIdentityRequest {
                        archive_path: workflow.archive_path.clone(),
                        platform: workflow.platform.clone(),
                        adapter: workflow.adapter,
                    };
                    if request == current
                        && workflow.identity_request.as_ref() == Some(&request)
                        && report.archive_path == workflow.archive_path
                    {
                        workflow.identity = CheatStepResource::Ready((request, report));
                    } else {
                        workflow.identity_request = None;
                        workflow.identity = CheatStepResource::NotLoaded;
                    }
                }
                Ok(Err(message)) => workflow.identity = CheatStepResource::Failed(message),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.identity = CheatStepResource::Failed(
                        "Game identity inspection stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
        // Automatic provider loading: once identity is ready, quietly
        // start the exact-match provider fetch in the background the
        // first time it's needed - never repeatedly, since the gate is
        // `NotLoaded` and every subsequent poll sees `Loading`/`Ready`/
        // `Failed` instead. A failed fetch is never auto-retried here;
        // the user retries explicitly (Details > Refresh).
        let need_dolphin_provider_fetch = dolphin_provider_auto_fetch_needed(workflow);
        let need_xenia_provider_fetch = xenia_provider_auto_fetch_needed(workflow);
        let need_wii_gamehacking_match = wii_gamehacking_auto_match_needed(workflow);
        let gamehacking_request_is_current = workflow
            .gamecube_gamehacking_request
            .as_ref()
            .and_then(|request| {
                dolphin_gamehacking_request_key(workflow, request.generation)
                    .map(|current| current == *request)
            })
            .unwrap_or(false);
        let mut preview_history_entry = None;
        let mut legacy_migration_history_entry = None;
        if let CheatStepResource::Loading { receiver } = &workflow.pcsx2_gamehacking {
            match receiver.try_recv() {
                Ok(Ok(provider)) => {
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatSourceRetrieval,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Completed,
                        format!(
                            "GameHacking.org check completed with {} compatible cheat(s).",
                            provider
                                .candidates
                                .iter()
                                .filter(|candidate| candidate.selectable())
                                .count()
                        ),
                    ));
                    workflow.pcsx2_gamehacking = CheatStepResource::Ready(provider);
                }
                Ok(Err(message)) => {
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatSourceRetrieval,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("GameHacking.org check failed: {message}"),
                    ));
                    workflow.pcsx2_gamehacking = CheatStepResource::Failed(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.pcsx2_gamehacking = CheatStepResource::Failed(
                        "GameHacking.org worker stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
        if let CheatStepResource::Loading { receiver } = &workflow.gamecube_gamehacking {
            match receiver.try_recv() {
                Ok(Ok(state)) if gamehacking_request_is_current => {
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatSourceRetrieval,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Completed,
                        format!(
                            "GameHacking.org check completed with {} cheat(s).",
                            state.cheats.len()
                        ),
                    ));
                    workflow.gamecube_gamehacking = CheatStepResource::Ready(state);
                    workflow.gamecube_gamehacking_blocked = false;
                    workflow.gamecube_gamehacking_cancellation = None;
                }
                Ok(Err(message)) if gamehacking_request_is_current => {
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatSourceRetrieval,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("GameHacking.org check failed: {message}"),
                    ));
                    workflow.gamecube_gamehacking_blocked = message
                        == GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE
                        || message.starts_with("GameHacking.org blocked");
                    workflow.gamecube_gamehacking = CheatStepResource::Failed(message);
                    workflow.gamecube_gamehacking_cancellation = None;
                }
                Ok(_) => {
                    if let Some(cancellation) = workflow.gamecube_gamehacking_cancellation.take() {
                        cancellation.store(true, Ordering::Relaxed);
                    }
                    workflow.gamecube_gamehacking_request = None;
                    workflow.gamecube_gamehacking = CheatStepResource::NotLoaded;
                    workflow.gamecube_gamehacking_blocked = false;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.gamecube_gamehacking = CheatStepResource::Failed(
                        "GameHacking.org worker stopped unexpectedly.".to_string(),
                    );
                    workflow.gamecube_gamehacking_blocked = false;
                    workflow.gamecube_gamehacking_cancellation = None;
                }
            }
        }
        if let CheatStepResource::Loading { receiver } = &workflow.bsfree_gamecube {
            match receiver.try_recv() {
                Ok(Ok(mut state)) => {
                    // Compute the destination-based duplicate/conflict analysis
                    // once against the real Dolphin GameSettings file so the
                    // list shows "Already installed"/"Conflict" honestly.
                    if let Some(configuration_path) = workflow
                        .selected_dolphin_profile_id
                        .as_ref()
                        .and_then(|id| dolphin_profile_paths.get(id))
                        && let Some(game) = state.game.clone()
                        && let Ok(destination) =
                            load_dolphin_destination(configuration_path, &game.archive_game_id)
                    {
                        state.analysis =
                            archivefs_core::patch_manager::analyze_bsfree_gamecube_duplicates(
                                &state.cheats,
                                &destination.document,
                            );
                        state.selection = BsFreeGameCubeCheatSelection::from_cheats(
                            &state.cheats,
                            &destination.document,
                        );
                    }
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatSourceRetrieval,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Completed,
                        format!(
                            "BSFree GameCube search returned {} cheat(s).",
                            state.cheats.len()
                        ),
                    ));
                    workflow.bsfree_gamecube = CheatStepResource::Ready(state);
                    workflow.bsfree_gamecube_cancellation = None;
                }
                Ok(Err(message)) => {
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatSourceRetrieval,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("BSFree GameCube search failed: {message}"),
                    ));
                    workflow.bsfree_gamecube = CheatStepResource::Failed(message);
                    workflow.bsfree_gamecube_cancellation = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.bsfree_gamecube = CheatStepResource::Failed(
                        "BSFree Archive worker stopped unexpectedly.".to_string(),
                    );
                    workflow.bsfree_gamecube_cancellation = None;
                }
            }
        }
        if let CheatStepResource::Loading { receiver } = &workflow.dolphin_provider {
            match receiver.try_recv() {
                Ok(Ok(fetch)) => {
                    let current_identity = ready_game_identity(workflow);
                    let current_key = current_identity.and_then(|identity| {
                        Some(DolphinProviderRequestKey {
                            archive_path: workflow.archive_path.clone(),
                            game_id: identity.verified_dolphin_game_id()?.to_string(),
                            revision: identity.verified_dolphin_revision()?,
                        })
                    });
                    if workflow.adapter == CheatEmulatorAdapter::Dolphin
                        && current_key.as_ref() == workflow.dolphin_provider_request.as_ref()
                        && current_key.as_ref().is_some_and(|key| {
                            key.game_id == fetch.result.game_id
                                && key.revision == fetch.result.revision
                        })
                    {
                        let (selection, destination_error) = build_dolphin_provider_selection(
                            &dolphin_profile_paths,
                            workflow.selected_dolphin_profile_id.as_deref(),
                            &fetch,
                        );
                        workflow.dolphin_destination_error = destination_error;
                        workflow.dolphin_provider_selection = selection;
                        preview_history_entry = Some(HistoryEntry::new(
                            ActivityAction::DolphinGeckoCandidateMatch,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Completed,
                            format!(
                                "External Gecko provider returned {} exact-ID code(s) for {} ({}).",
                                fetch.result.entries.len(),
                                fetch.result.game_id,
                                dolphin_provider_fetch_status_label(fetch.status)
                            ),
                        ));
                        workflow.dolphin_provider = CheatStepResource::Ready(fetch);
                    } else {
                        workflow.dolphin_provider_request = None;
                        workflow.dolphin_provider = CheatStepResource::NotLoaded;
                        workflow.dolphin_provider_selection = None;
                    }
                }
                Ok(Err(message)) => {
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::DolphinGeckoCandidateMatch,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("External Gecko provider failed: {message}"),
                    ));
                    workflow.dolphin_provider = CheatStepResource::Failed(message);
                    workflow.dolphin_provider_selection = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.dolphin_provider = CheatStepResource::Failed(
                        "External Gecko provider stopped unexpectedly.".to_string(),
                    );
                    workflow.dolphin_provider_selection = None;
                }
            }
        }
        if let CheatStepResource::Loading { receiver } = &workflow.xenia_provider {
            match receiver.try_recv() {
                Ok(Ok(fetch)) => {
                    let current_identity = ready_game_identity(workflow);
                    let current_key = current_identity.and_then(|identity| {
                        Some(XeniaProviderRequestKey {
                            archive_path: workflow.archive_path.clone(),
                            title_id: identity.verified_xex_title_id()?.to_string(),
                        })
                    });
                    if workflow.adapter == CheatEmulatorAdapter::Xenia
                        && current_key.as_ref() == workflow.xenia_provider_request.as_ref()
                        && current_key
                            .as_ref()
                            .is_some_and(|key| key.title_id == fetch.result.title_id)
                    {
                        preview_history_entry = Some(HistoryEntry::new(
                            ActivityAction::XeniaPatchCandidateMatch,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Completed,
                            format!(
                                "Xenia Canary provider returned {} file(s) for Title ID {}.",
                                fetch.result.documents.len(),
                                fetch.result.title_id
                            ),
                        ));
                        workflow.xenia_selected_candidate_index = None;
                        workflow.xenia_selection = None;
                        workflow.xenia_destination_error = None;
                        workflow.xenia_provider = CheatStepResource::Ready(fetch);
                    } else {
                        workflow.xenia_provider_request = None;
                        workflow.xenia_provider = CheatStepResource::NotLoaded;
                        workflow.xenia_selection = None;
                    }
                }
                Ok(Err(message)) => {
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::XeniaPatchCandidateMatch,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("Xenia Canary provider failed: {message}"),
                    ));
                    workflow.xenia_provider = CheatStepResource::Failed(message);
                    workflow.xenia_selection = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.xenia_provider = CheatStepResource::Failed(
                        "Xenia Canary provider stopped unexpectedly.".to_string(),
                    );
                    workflow.xenia_selection = None;
                }
            }
        }
        if identity_page_is_current {
            let current_key = cheat_preview_key(workflow);
            if workflow
                .preview_request
                .as_ref()
                .is_some_and(|key| key != &current_key)
            {
                workflow.preview_request = None;
                workflow.preview = CheatStepResource::NotLoaded;
            } else if let CheatStepResource::Loading { receiver } = &workflow.preview {
                match receiver.try_recv() {
                    Ok(Ok(response)) if response.key == current_key => {
                        let (outcome, message) = match &response.outcome {
                            CheatPreviewOutcome::Ready(report) if !report.conflicts.is_empty() => (
                                ActivityOutcome::Rejected,
                                format!(
                                    "Read-only preview found {} conflicts across {} entries.",
                                    report.conflicts.len(),
                                    report.entries.len()
                                ),
                            ),
                            CheatPreviewOutcome::Ready(report) if report.summary.blocked > 0 => (
                                ActivityOutcome::Rejected,
                                format!(
                                    "Read-only preview was blocked for {} of {} entries.",
                                    report.summary.blocked,
                                    report.entries.len()
                                ),
                            ),
                            CheatPreviewOutcome::Ready(report)
                                if response.materialized.is_some() =>
                            {
                                let materialized = response.materialized.as_ref().unwrap();
                                (
                                    ActivityOutcome::Completed,
                                    format!(
                                        "Matching ready: {} indexed catalogue entries, {} excluded; preview completed for {} entries.",
                                        materialized.indexed_file_count,
                                        materialized.excluded_file_count,
                                        report.entries.len()
                                    ),
                                )
                            }
                            CheatPreviewOutcome::Ready(report) if response.generated.is_some() => {
                                let generated = response.generated.as_ref().unwrap();
                                (
                                    ActivityOutcome::Completed,
                                    format!(
                                        "Install preview created: {} cheat(s) from '{}' -> {}. No files changed yet.",
                                        generated.staged.selected_cheat_count,
                                        generated.candidate_display_name,
                                        generated.destination.path.display()
                                    ),
                                )
                            }
                            CheatPreviewOutcome::Ready(report) => (
                                ActivityOutcome::Completed,
                                format!(
                                    "Read-only preview completed for {} entries; no files changed.",
                                    report.entries.len()
                                ),
                            ),
                            CheatPreviewOutcome::Failed(CheatPreviewFailure::Materialization(
                                error,
                            )) if error.kind
                                == RetroArchMaterializationErrorKind::MatchingEntryExcluded =>
                            {
                                (
                                    ActivityOutcome::Rejected,
                                    format!("Matching entry excluded: {}", error.detail),
                                )
                            }
                            CheatPreviewOutcome::Failed(error) => (
                                ActivityOutcome::Failed,
                                format!("Read-only preview failed: {error}"),
                            ),
                        };
                        preview_history_entry = Some(HistoryEntry::new(
                            ActivityAction::CheatPreview,
                            Some(workflow.archive_path.clone()),
                            outcome,
                            message,
                        ));
                        workflow.preview = CheatStepResource::Ready(response);
                    }
                    Ok(Ok(_)) => {
                        workflow.preview_request = None;
                        workflow.preview = CheatStepResource::NotLoaded;
                    }
                    Ok(Err(message)) => {
                        preview_history_entry = Some(HistoryEntry::new(
                            ActivityAction::CheatPreview,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Failed,
                            format!("Read-only preview worker failed: {message}"),
                        ));
                        workflow.preview = CheatStepResource::Failed(message);
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        let message = "Read-only preview stopped unexpectedly.".to_string();
                        preview_history_entry = Some(HistoryEntry::new(
                            ActivityAction::CheatPreview,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Failed,
                            &message,
                        ));
                        workflow.preview = CheatStepResource::Failed(message);
                    }
                }
            }
        }
        let current_transaction_key = cheat_preview_key(workflow);
        let transaction_is_stale = match &workflow.transaction {
            CheatTransactionState::Review { key, .. }
            | CheatTransactionState::Applying { key, .. }
            | CheatTransactionState::Result { key, .. } => key != &current_transaction_key,
            CheatTransactionState::Idle => false,
        };
        // docs/GUI_NAVIGATION_RESET_DESIGN.md mandatory risk #1: an
        // `Applying` transaction's receiver must never be dropped merely
        // because the Cheats & Mods page isn't the one currently rendered
        // (Gamer View's action panel and a Gamer/Advanced mode switch both
        // legitimately leave this page without the transaction being done).
        // Only a genuinely stale transaction (a different archive/adapter/
        // profile/key than what's now selected) is reset while in flight -
        // that reset is a correctness rule, not a page-visibility one, and
        // is unchanged here.
        let transaction_in_flight =
            matches!(workflow.transaction, CheatTransactionState::Applying { .. });
        if (!identity_page_is_current && !transaction_in_flight) || transaction_is_stale {
            workflow.transaction = CheatTransactionState::Idle;
        } else if let CheatTransactionState::Applying { key, receiver } = &workflow.transaction {
            let key = key.clone();
            match receiver.try_recv() {
                Ok(Ok(result)) => {
                    let outcome = match result.journal.status {
                        SharedApplyStatus::Success => ActivityOutcome::Completed,
                        SharedApplyStatus::PartialFailure => ActivityOutcome::Failed,
                        SharedApplyStatus::Failed => ActivityOutcome::Failed,
                        SharedApplyStatus::DryRun => ActivityOutcome::Skipped,
                    };
                    let final_targets = result
                        .journal
                        .entries
                        .iter()
                        .map(|entry| {
                            format!(
                                "{}/{}",
                                entry.plan_entry.destination_root.display,
                                entry.plan_entry.destination_relative_path.display
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatInstall,
                        Some(workflow.archive_path.clone()),
                        outcome,
                        format!(
                            "Shared apply '{}' finished with {:?}. Live target(s): {}.",
                            result.journal.operation_id, result.journal.status, final_targets
                        ),
                    ));
                    if result.journal.status == SharedApplyStatus::Success {
                        let managed_after = match &workflow.preview {
                            CheatStepResource::Ready(response) => response
                                .gamecube_gamehacking_generated
                                .as_ref()
                                .map(|generated| {
                                    archivefs_core::patch_manager::managed_names(
                                        &parse_dolphin_ini(&generated.staged.contents),
                                    )
                                }),
                            _ => None,
                        };
                        if let (Some(managed), CheatStepResource::Ready(state)) =
                            (managed_after, &mut workflow.gamecube_gamehacking)
                        {
                            for entry in &mut state.selection.entries {
                                entry.already_managed = managed.contains(&entry.dolphin_name);
                            }
                        }
                    }
                    if workflow.adapter == CheatEmulatorAdapter::Pcsx2
                        && result.journal.status == SharedApplyStatus::Success
                    {
                        legacy_migration_history_entry =
                            apply_pcsx2_pending_legacy_migration(workflow, &result);
                    }
                    workflow.transaction = CheatTransactionState::Result { key, result };
                }
                Ok(Err(message)) => {
                    preview_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatInstall,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("Shared apply worker failed: {message}"),
                    ));
                    workflow.transaction_notice = Some(format!("Install failed: {message}"));
                    workflow.transaction = CheatTransactionState::Idle;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.transaction_notice = Some(
                        "Install failed: the apply worker stopped before reporting a result."
                            .to_string(),
                    );
                    workflow.transaction = CheatTransactionState::Idle;
                }
            }
        }
        if let CheatStepResource::Loading { receiver } = &workflow.source_list {
            match receiver.try_recv() {
                Ok(Ok(list)) => {
                    if workflow.selected_source_id.is_none() {
                        let enabled: Vec<&CheatSourceListEntry> = list
                            .entries
                            .iter()
                            .filter(|entry| entry.source.enabled)
                            .collect();
                        if enabled.len() == 1 {
                            workflow.selected_source_id = Some(enabled[0].source.source_id.clone());
                        }
                    }
                    workflow.source_list = CheatStepResource::Ready(list);
                }
                Ok(Err(message)) => {
                    workflow.source_list = CheatStepResource::Failed(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.source_list = CheatStepResource::Failed(
                        "Trusted-source listing stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
        if let CheatStepResource::Loading { receiver } = &workflow.existing_library {
            match receiver.try_recv() {
                Ok(Ok(inspection)) => {
                    workflow.existing_library = CheatStepResource::Ready(inspection);
                }
                Ok(Err(message)) => {
                    workflow.existing_library = CheatStepResource::Failed(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.existing_library = CheatStepResource::Failed(
                        "RetroArch library inspection stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
        if let Some(receiver) = workflow.pcsx2_activation_receiver.take() {
            match receiver.try_recv() {
                Ok(Ok(value)) => {
                    workflow.pcsx2_activation = CheatActivationReadiness::from_bool(value);
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    workflow.pcsx2_activation = CheatActivationReadiness::Unknown;
                }
                Err(TryRecvError::Empty) => workflow.pcsx2_activation_receiver = Some(receiver),
            }
        }
        if let Some(receiver) = workflow.dolphin_activation_receiver.take() {
            match receiver.try_recv() {
                Ok(Ok(value)) => {
                    workflow.dolphin_activation = CheatActivationReadiness::from_bool(value);
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    workflow.dolphin_activation = CheatActivationReadiness::Unknown;
                }
                Err(TryRecvError::Empty) => workflow.dolphin_activation_receiver = Some(receiver),
            }
        }
        let mut pcsx2_history_entry = None;
        if let CheatStepResource::Loading { receiver } = &workflow.pcsx2_inventory {
            match receiver.try_recv() {
                Ok(Ok(inventory)) => {
                    if workflow.adapter == CheatEmulatorAdapter::Pcsx2
                        && workflow.selected_pcsx2_profile_id.as_deref()
                            == Some(inventory.profile_id.as_str())
                        && workflow.pcsx2_inventory_profile_id.as_deref()
                            == Some(inventory.profile_id.as_str())
                    {
                        pcsx2_history_entry = Some(HistoryEntry::new(
                            ActivityAction::Pcsx2PnachInspection,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Completed,
                            format!(
                                "PCSX2 PNACH inspection found {} files ({} warnings).",
                                inventory.files.len(),
                                inventory.warnings.len()
                            ),
                        ));
                        workflow.pcsx2_inventory = CheatStepResource::Ready(inventory);
                    } else {
                        workflow.pcsx2_inventory = CheatStepResource::NotLoaded;
                    }
                }
                Ok(Err(message)) => {
                    workflow.pcsx2_activation = CheatActivationReadiness::Unknown;
                    pcsx2_history_entry = Some(HistoryEntry::new(
                        ActivityAction::Pcsx2PnachInspection,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("PCSX2 PNACH inspection failed: {message}"),
                    ));
                    workflow.pcsx2_inventory = CheatStepResource::Failed(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.pcsx2_activation = CheatActivationReadiness::Unknown;
                    workflow.pcsx2_inventory = CheatStepResource::Failed(
                        "PCSX2 PNACH inspection stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
        let mut dolphin_history_entry = None;
        if let CheatStepResource::Loading { receiver } = &workflow.dolphin_inventory {
            match receiver.try_recv() {
                Ok(Ok(inventory)) => {
                    if workflow.adapter == CheatEmulatorAdapter::Dolphin
                        && workflow.selected_dolphin_profile_id.as_deref()
                            == Some(inventory.profile_id.as_str())
                        && workflow.dolphin_inventory_profile_id.as_deref()
                            == Some(inventory.profile_id.as_str())
                    {
                        dolphin_history_entry = Some(HistoryEntry::new(
                            ActivityAction::DolphinGameIniInspection,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Completed,
                            format!(
                                "Dolphin GameSettings inspection found {} INI files ({} warnings).",
                                inventory.files.len(),
                                inventory.warnings.len()
                            ),
                        ));
                        workflow.dolphin_inventory = CheatStepResource::Ready(inventory);
                    } else {
                        workflow.dolphin_inventory = CheatStepResource::NotLoaded;
                    }
                }
                Ok(Err(message)) => {
                    workflow.dolphin_activation = CheatActivationReadiness::Unknown;
                    dolphin_history_entry = Some(HistoryEntry::new(
                        ActivityAction::DolphinGameIniInspection,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("Dolphin GameSettings inspection failed: {message}"),
                    ));
                    workflow.dolphin_inventory = CheatStepResource::Failed(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.dolphin_activation = CheatActivationReadiness::Unknown;
                    workflow.dolphin_inventory = CheatStepResource::Failed(
                        "Dolphin GameSettings inspection stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
        // Stage 4 result. Bound to the request key that produced it, so a
        // match that finishes after the archive, profile, or snapshot
        // changed is dropped rather than shown against the new context.
        let mut candidate_history_entry = None;
        if let CheatStepResource::Loading { receiver } = &workflow.candidates {
            let current_key = workflow.candidates_request.clone();
            match receiver.try_recv() {
                Ok(Ok(stage)) if Some(&stage.key) == current_key.as_ref() => {
                    let installable = stage.list.installable().count();
                    candidate_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatPreview,
                        Some(workflow.archive_path.clone()),
                        if installable == 0 {
                            ActivityOutcome::Rejected
                        } else {
                            ActivityOutcome::Completed
                        },
                        format!(
                            "Match completed: {} candidate(s) shown of {} matched, {installable} installable.",
                            stage.list.candidates.len(),
                            stage.list.total_matched
                        ),
                    ));
                    // A single verified-exact best candidate is the one case
                    // where choosing for the user is safe; anything else
                    // stays an explicit choice.
                    let automatic = stage
                        .list
                        .automatic_choice()
                        .map(|candidate| candidate.catalogue_relative_path.clone());
                    workflow.candidates = CheatStepResource::Ready(stage);
                    if let Some(relative_path) = automatic {
                        automatic_candidate = Some(relative_path);
                    }
                }
                Ok(Ok(_)) => {
                    workflow.candidates = CheatStepResource::NotLoaded;
                    workflow.candidates_request = None;
                }
                Ok(Err(message)) => {
                    candidate_history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatPreview,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("Catalogue matching failed: {message}"),
                    ));
                    workflow.candidates = CheatStepResource::Failed(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.candidates = CheatStepResource::Failed(
                        "Catalogue matching stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
        let mut history_entry = None;
        if let CheatStepResource::Loading { receiver } = &workflow.source_fetch {
            match receiver.try_recv() {
                Ok(Ok(result)) => {
                    if workflow.selected_source_id.as_deref()
                        == Some(result.source.source_id.as_str())
                    {
                        history_entry = Some(HistoryEntry::new(
                            ActivityAction::CheatSourceRetrieval,
                            Some(workflow.archive_path.clone()),
                            ActivityOutcome::Completed,
                            format!(
                                "Cheat source '{}': {} (digest {}).",
                                result.source.source_id,
                                cheat_fetch_status_label(result.status),
                                result.manifest.archive_sha256
                            ),
                        ));
                        workflow.source_fetch = CheatStepResource::Ready(result);
                    } else {
                        // The selection changed while this retrieval ran;
                        // its result no longer applies to anything shown.
                        workflow.source_fetch = CheatStepResource::NotLoaded;
                    }
                }
                Ok(Err(message)) => {
                    history_entry = Some(HistoryEntry::new(
                        ActivityAction::CheatSourceRetrieval,
                        Some(workflow.archive_path.clone()),
                        ActivityOutcome::Failed,
                        format!("Cheat source retrieval failed: {message}"),
                    ));
                    workflow.source_fetch = CheatStepResource::Failed(message);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    workflow.source_fetch = CheatStepResource::Failed(
                        "Catalogue retrieval stopped unexpectedly.".to_string(),
                    );
                }
            }
        }
        if let Some(entry) = candidate_history_entry {
            self.history.record(entry);
        }
        if let Some(relative_path) = automatic_candidate {
            self.apply_cheat_candidate_choice(&relative_path);
        }
        if let Some(entry) = history_entry {
            self.history.record(entry);
        }
        if let Some(entry) = pcsx2_history_entry {
            self.history.record(entry);
        }
        if let Some(entry) = dolphin_history_entry {
            self.history.record(entry);
        }
        if let Some(entry) = preview_history_entry {
            self.history.record(entry);
        }
        if let Some(entry) = legacy_migration_history_entry {
            self.history.record(entry);
        }
        // Dolphin: try the local catalogue/cache first, synchronously and
        // read-only - never gated behind `cfg!(test)` since it never spawns
        // a thread or touches the network, only EmuWiz's own cache
        // files (which simply won't exist under `cargo test`, so this is a
        // fast no-op there exactly like every other "no catalogue" case).
        // Per the Dolphin cheat catalogue design, if that finds nothing,
        // no automatic network request follows - only an explicit fetch
        // (Details > Refresh) reaches the network.
        if need_dolphin_provider_fetch {
            self.try_resolve_dolphin_provider_from_local_sources(&dolphin_profile_paths);
        }
        if !cfg!(test) && need_wii_gamehacking_match {
            self.start_gamecube_gamehacking_fetch_mode(
                context.clone(),
                false,
                WiiGameHackingFetchMode::CacheOnly,
            );
        }
        // The real background fetch is skipped under `cargo test`: an
        // automatic trigger firing from a plain identity-ready + provider
        // "not loaded" fixture would otherwise spawn a real network
        // thread (`ureq`) from ordinary unit tests, which never happened
        // before this was automatic (no existing test called
        // `start_xenia_provider_fetch` directly - it always asserts
        // against a fixture already in a `Ready`/`Failed`/`Loading`
        // state). `need_xenia_provider_fetch` itself is an ordinary
        // boolean and stays fully covered by direct unit tests on
        // `CheatWorkflowState`.
        if !cfg!(test) && need_xenia_provider_fetch {
            self.start_xenia_provider_fetch(context.clone(), false);
        }
    }

    /// Keeps the Cheats & Mods workspace in sync with `selected_archive`,
    /// the one authoritative "which archive" field shared with Library and
    /// Mount (see the module-level note on archive-selection continuity).
    /// Runs every frame while this page is open, so entering the page,
    /// switching the Library selection while already here, and navigating
    /// away and back all converge on the same archive without a stale
    /// "No archive context selected" state. An explicit in-page "Choose
    /// archive" pick writes back to `selected_archive` itself (see
    /// `apply_cheat_archive_choice`), so this never fights a user's
    /// explicit in-workspace choice - it only ever catches up to it.
    pub(crate) fn reconcile_cheats_mods_context(&mut self, context: &egui::Context) {
        if self.view != MainView::CheatsMods {
            return;
        }
        match self.archive_context.active_cheats().map(Path::to_path_buf) {
            Some(path)
                if !self
                    .cheat_workflow
                    .as_ref()
                    .is_some_and(|workflow| workflow.archive_path == path) =>
            {
                self.open_cheats_mods_workspace(context, path);
            }
            None => self.cheat_workflow = None,
            Some(_) => {}
        }
        let context_is_current =
            self.cheat_workflow
                .as_ref()
                .is_some_and(|workflow| match &self.state {
                    LoadState::Ready(data) => data
                        .records
                        .iter()
                        .any(|record| record.mount_plan.archive.path == workflow.archive_path),
                    LoadState::Loading { .. } | LoadState::Error(_) => true,
                });
        if !context_is_current {
            self.cheat_workflow = None;
            return;
        }
        let live_platform = self
            .cheat_workflow
            .as_ref()
            .and_then(|workflow| match &self.state {
                LoadState::Ready(data) => data
                    .records
                    .iter()
                    .find(|record| record.mount_plan.archive.path == workflow.archive_path)
                    .map(|record| record.identity.platform.clone()),
                LoadState::Loading { previous, .. } => previous.as_deref().and_then(|data| {
                    data.records
                        .iter()
                        .find(|record| record.mount_plan.archive.path == workflow.archive_path)
                        .map(|record| record.identity.platform.clone())
                }),
                LoadState::Error(_) => None,
            });
        if let Some(live_platform) = live_platform {
            let route_changed = self.cheat_workflow.as_ref().is_some_and(|workflow| {
                workflow.platform != live_platform
                    && workflow.adapter != cheat_adapter_route(live_platform.as_deref())
            });
            if route_changed {
                let archive = self
                    .cheat_workflow
                    .as_ref()
                    .map(|workflow| workflow.archive_path.clone());
                self.cheat_workflow = None;
                if let Some(archive) = archive {
                    self.open_cheats_mods_workspace(context, archive);
                }
            } else if let Some(workflow) = self.cheat_workflow.as_mut()
                && workflow.platform != live_platform
            {
                workflow.platform = live_platform;
                workflow.identity_request = None;
                workflow.identity = CheatStepResource::NotLoaded;
                clear_cheat_candidate_state(workflow);
            }
        }
    }

    /// Applies the profile chooser's highlighted candidate: selects it,
    /// records the outcome as an explicit choice (so it stays selected on
    /// the next scan even before persistence lands), and remembers it for
    /// next time. Never called implicitly - only from the chooser's own
    /// "Use selected profile" button.
    pub(crate) fn confirm_dolphin_profile_choice(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let Some(profile_id) = workflow.dolphin_profile_choice.clone() else {
            return;
        };
        let root = match &self.dolphin_profiles {
            DolphinProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.profile_id == profile_id)
                .map(|profile| profile.configuration_path.clone()),
            _ => None,
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.selected_dolphin_profile_id = Some(profile_id.clone());
        workflow.dolphin_inventory_profile_id = None;
        workflow.dolphin_inventory = CheatStepResource::NotLoaded;
        workflow.dolphin_activation = CheatActivationReadiness::Unknown;
        workflow.dolphin_activation_receiver = None;
        workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
            profile_id: profile_id.clone(),
            reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::ExplicitChoice,
        });
        if let DolphinProfilesState::Ready(discovery) = &self.dolphin_profiles {
            reconcile_dolphin_provider_selection(workflow, discovery);
        }
        if let Some(root) = root {
            self.persist_remembered_profile("dolphin", &profile_id, &root);
        }
    }

    /// Xenia's counterpart to `confirm_dolphin_profile_choice`.
    pub(crate) fn confirm_xenia_profile_choice(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let Some(profile_id) = workflow.xenia_profile_choice.clone() else {
            return;
        };
        let root = match &self.xenia_profiles {
            XeniaProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.profile_id == profile_id)
                .map(|profile| profile.configuration_path.clone()),
            XeniaProfilesState::NotScanned => None,
        };
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.selected_xenia_profile_id = Some(profile_id.clone());
        workflow.xenia_profile_selection = Some(EmulatorProfileSelection::Auto {
            profile_id: profile_id.clone(),
            reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::ExplicitChoice,
        });
        if let Some(root) = root {
            self.persist_remembered_profile("xenia", &profile_id, &root);
        }
    }

    /// The beginner "Install selected" button for Dolphin: builds the
    /// install preview and immediately moves it into the review stage, so
    /// the ordinary compatible-install path never needs a separate
    /// technical Preview click before the beginner confirmation dialog
    /// can appear. Both steps are synchronous local operations already
    /// used by the technical Details flow - this only chains them.
    pub(crate) fn start_beginner_install_dolphin(&mut self) {
        self.start_dolphin_install_preview();
        self.review_cheat_apply();
        if let Some(workflow) = self.cheat_workflow.as_mut() {
            workflow.dolphin_show_exact_changes = false;
        }
    }

    /// Xenia's counterpart to `start_beginner_install_dolphin`.
    pub(crate) fn start_beginner_install_xenia(&mut self) {
        self.start_xenia_install_preview();
        self.review_cheat_apply();
        if let Some(workflow) = self.cheat_workflow.as_mut() {
            workflow.xenia_show_exact_changes = false;
        }
    }


}

/// Runs the legacy CRC-only PNACH migration (staged alongside the primary
/// install as `workflow.preview`'s `pcsx2_generated.legacy_migration_report`)
/// as its own chained shared-apply operation, immediately after the
/// primary install this belongs to succeeds. Deliberately a *separate*
/// `execute_shared_apply` call with its own operation ID and journal: two
/// verified-exact entries for one identity in a single PCSX2 preview/plan
/// is treated as an unresolvable ambiguity elsewhere in this pipeline (see
/// `PreviewBlockerKind::MultipleExactMatches`), so migration cleanup can
/// never be folded into the primary plan. Its journal lands in the same
/// shared history root as the primary apply, so it is already visible and
/// independently undoable from History & Logs without any bespoke UI.
/// Returns the `HistoryEntry` to record, or `None` if no migration was
/// pending.
pub(crate) fn apply_pcsx2_pending_legacy_migration(
    workflow: &CheatWorkflowState,
    primary_result: &SharedApplyResult,
) -> Option<HistoryEntry> {
    let CheatStepResource::Ready(response) = &workflow.preview else {
        return None;
    };
    let legacy_report = response
        .pcsx2_generated
        .as_ref()
        .and_then(|generated| generated.legacy_migration_report.clone())?;
    let archive_path = Some(workflow.archive_path.clone());
    let approved_source_root = match primary_result.journal.approved_source_root.to_path_buf() {
        Ok(path) => path,
        Err(message) => {
            return Some(HistoryEntry::new(
                ActivityAction::CheatInstall,
                archive_path,
                ActivityOutcome::Failed,
                format!("Legacy PNACH migration path could not be reconstructed: {message:?}"),
            ));
        }
    };
    let plan = match build_shared_transaction_plan(
        &legacy_report,
        &primary_result.journal.context.profile_id,
        &primary_result.journal.context.source_mode,
        &approved_source_root,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return Some(HistoryEntry::new(
                ActivityAction::CheatInstall,
                archive_path,
                ActivityOutcome::Failed,
                format!(
                    "Legacy PNACH migration could not be planned: {}",
                    error.detail
                ),
            ));
        }
    };
    let (history_root, backup_root) =
        match (default_shared_history_root(), default_shared_backup_root()) {
            (Ok(history_root), Ok(backup_root)) => (history_root, backup_root),
            _ => {
                return Some(HistoryEntry::new(
                    ActivityAction::CheatInstall,
                    archive_path,
                    ActivityOutcome::Failed,
                    "Legacy PNACH migration could not resolve the shared history/backup root"
                        .to_string(),
                ));
            }
        };
    let operation_id = format!("{}-legacy-migration", primary_result.journal.operation_id);
    let timestamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let result = execute_shared_apply(
        &plan,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: true,
                replacement_approved: true,
            }),
            operation_id: operation_id.clone(),
            timestamp_unix_seconds: timestamp,
            current_context: plan.context.clone(),
            history_root,
            backup_root,
        },
    );
    let outcome = match result.journal.status {
        SharedApplyStatus::Success => ActivityOutcome::Completed,
        SharedApplyStatus::PartialFailure | SharedApplyStatus::Failed => ActivityOutcome::Failed,
        SharedApplyStatus::DryRun => ActivityOutcome::Skipped,
    };
    Some(HistoryEntry::new(
        ActivityAction::CheatInstall,
        archive_path,
        outcome,
        format!(
            "Legacy PNACH migration '{}' finished with {:?} (undo available from History & Logs).",
            result.journal.operation_id, result.journal.status
        ),
    ))
}

pub(crate) fn run_bsfree_operation(
    operation: &BsFreeOperation,
) -> Result<BsFreeOperationResult, archivefs_core::patch_manager::BsFreeError> {
    let paths = BsFreePaths::at(default_bsfree_source_root()?);
    match operation {
        BsFreeOperation::LoadStatus => inspect_bsfree_source(&paths)
            .map(Box::new)
            .map(BsFreeOperationResult::Status),
        BsFreeOperation::Download => download_bsfree_database(
            &paths,
            &BsFreeDownloadOptions::default(),
            &HttpsCheatSourceTransport::new(),
        )
        .map(|result| BsFreeOperationResult::Status(Box::new(result.status))),
        BsFreeOperation::Import(source) => import_local_bsfree_database(&paths, source)
            .map(|result| BsFreeOperationResult::Status(Box::new(result.status))),
        BsFreeOperation::Validate => validate_installed_bsfree_source(&paths)
            .map(Box::new)
            .map(BsFreeOperationResult::Status),
        BsFreeOperation::SetEnabled(enabled) => set_bsfree_enabled(&paths, *enabled)
            .map(Box::new)
            .map(BsFreeOperationResult::Status),
        BsFreeOperation::Remove => {
            remove_local_bsfree_source(&paths, true)?;
            Ok(BsFreeOperationResult::Removed)
        }
        BsFreeOperation::Search(request) => BsFreeCatalogue::open_installed(&paths)?
            .search_games(request)
            .map(BsFreeOperationResult::Search),
        BsFreeOperation::LoadSystems => BsFreeCatalogue::open_installed(&paths)?
            .systems(PageRequest {
                offset: 0,
                limit: PageRequest::HARD_LIMIT,
            })
            .map(BsFreeOperationResult::Systems),
        BsFreeOperation::LoadGame {
            upstream_uid,
            offset,
        } => {
            let catalogue = BsFreeCatalogue::open_installed(&paths)?;
            let game = catalogue.game(*upstream_uid)?.ok_or_else(|| {
                archivefs_core::patch_manager::BsFreeError {
                    kind: archivefs_core::patch_manager::BsFreeErrorKind::Query,
                    message: "BSFree game is no longer present".to_string(),
                }
            })?;
            let cheats = catalogue.cheats(*upstream_uid, PageRequest::cheats(*offset))?;
            Ok(BsFreeOperationResult::Game(game, cheats))
        }
    }
}

/// The design's per-archive validation label on the Mount page - a pure
/// mapping from the live `MountState`, so the preview can never disagree
/// with what the batch engine will actually do (`Pending` is the only
/// state `queued_pending_paths` lets through to a mount attempt).
pub(crate) fn mount_validation_label(state: MountState) -> &'static str {
    match state {
        MountState::Pending => "Ready to mount",
        MountState::Mounted => "Already mounted — will be skipped",
        MountState::MountPathExists => "Destination already exists — will be skipped",
        MountState::NotMountable => "Loose ROM · no EmuWiz mount required",
    }
}
