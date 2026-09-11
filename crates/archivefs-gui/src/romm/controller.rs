use crate::*;

impl ArchiveFsApp {
    /// Starts one RomM operation, or declines.
    ///
    /// Declining is the point: a second click while something is running must not
    /// launch a duplicate, and a mutating operation must not overlap another. A
    /// status load is allowed to be the exception only because it writes nothing.
    pub(crate) fn start_romm_operation(&mut self, context: egui::Context, operation: RommOperation) -> bool {
        if let Some(running) = &self.romm_operation {
            // A status load asked for while something else runs is dropped rather
            // than queued: the operation that finishes will refresh anyway.
            let _ = running;
            return false;
        }
        self.romm_generation = self.romm_generation.wrapping_add(1);
        let generation = self.romm_generation;
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        let (progress_sender, progress_receiver) = mpsc::channel();
        // A fresh operation supersedes the previous result, so the card never shows
        // an old outcome beside new progress.
        self.romm_ui.last_outcome = None;
        if operation.is_mutating() {
            self.history.record(HistoryEntry::new(
                ActivityAction::RommSource,
                None,
                ActivityOutcome::Started,
                format!("{}.", operation.label()),
            ));
        }
        self.romm_operation = Some(RunningRommOperation {
            generation,
            operation: operation.clone(),
            cancellation,
            receiver,
            progress_receiver,
            progress: operation.reports_progress().then(RommProgress::default),
            cancellation_requested: false,
        });
        let trusted_roots = self.gui_config.source_roots().map(|roots| roots.to_vec());
        let database_path = database_state_path(&self.database_state);
        let progress_context = context.clone();
        thread::spawn(move || {
            let report = |event: RommProgressEvent| {
                let _ = progress_sender.send((generation, event));
                progress_context.request_repaint();
            };
            let result = run_romm_operation(
                &operation,
                &trusted_roots,
                database_path.as_deref(),
                generation,
                &worker_cancellation,
                &report,
            );
            let _ = sender.send((generation, result));
            context.request_repaint();
        });
        true
    }

    /// Asks the running operation to stop. Only the current one: an older
    /// generation has already been forgotten.
    pub(crate) fn cancel_romm_operation(&mut self) {
        if let Some(running) = self.romm_operation.as_mut() {
            running.cancellation.store(true, Ordering::Release);
            running.cancellation_requested = true;
        }
    }

    pub(crate) fn poll_romm_operation(&mut self, context: &egui::Context) {
        // Progress first, discarding anything from a superseded generation.
        if let Some(running) = self.romm_operation.as_mut() {
            let current = running.generation;
            for (generation, event) in running.progress_receiver.try_iter() {
                if generation != current {
                    continue;
                }
                let progress = running.progress.get_or_insert_with(RommProgress::default);
                match event {
                    RommProgressEvent::Import(import) => progress.absorb(import),
                    RommProgressEvent::Note(note) => progress.note(note),
                    RommProgressEvent::StaleProgress { probed, total } => {
                        self.romm_stale_progress =
                            Some(crate::romm_browse::StaleProgress { probed, total });
                    }
                    RommProgressEvent::Hashing(hashing) => {
                        self.romm_hash_progress = Some(hashing);
                    }
                }
            }
        }

        let Some((generation, operation, result)) =
            self.romm_operation.as_ref().and_then(|running| {
                running
                    .receiver
                    .try_recv()
                    .ok()
                    .map(|(generation, result)| (generation, running.operation.clone(), result))
            })
        else {
            return;
        };
        if generation != self.romm_generation {
            // A result from an operation that has already been superseded. Dropping
            // it is what stops it overwriting the current one's outcome.
            self.romm_operation = None;
            return;
        }
        self.romm_operation = None;
        self.romm_hash_progress = None;

        let previous_outcome = self.romm_ui.last_outcome.take();
        let offline_usable = self.romm_snapshot.as_deref().is_some_and(|snapshot| {
            snapshot.status.state
                == archivefs_core::identity_source::status::ProviderState::ReadyOffline
        });
        self.romm_ui.last_outcome = Some(romm_source::build_result_view(
            &operation,
            result.as_ref().map_err(String::as_str),
            offline_usable,
        ));
        if let Ok(RommOperationOutcome::Linkage(report)) = &result {
            self.romm_ui.linkage_report = Some(report.clone());
        }
        if let Ok(RommOperationOutcome::MappingPlan(plan)) = &result {
            self.romm_ui.mapping_plan = Some(plan.clone());
        }
        if operation.is_mutating() {
            // A failed connection test while imported identity is still being
            // served is the offline case working as intended: it must not
            // surface as a scary global "Failed". The technical reason stays
            // in the message, so History & Logs preserves it exactly.
            let offline_friendly =
                offline_usable && operation == RommOperation::TestConnection && result.is_err();
            self.history.record(HistoryEntry::new(
                ActivityAction::RommSource,
                None,
                if offline_friendly {
                    ActivityOutcome::OfflineUsable
                } else {
                    match &result {
                        Ok(_) => ActivityOutcome::Completed,
                        Err(_) => ActivityOutcome::Failed,
                    }
                },
                if offline_friendly {
                    format!(
                        "RomM could not be reached, but the offline copy still works. \
                         Technical detail: {message}",
                        message = result.as_ref().unwrap_err()
                    )
                } else {
                    match &result {
                        Ok(_) => format!("{} completed.", operation.label()),
                        Err(message) => format!("{} failed: {message}", operation.label()),
                    }
                },
            ));
        }
        // Browsing results belong to the panel rather than to the card, and each is
        // only accepted if it still answers what the panel is asking for.
        if let Ok(outcome) = &result {
            let landed = match outcome {
                RommOperationOutcome::Records(page) => {
                    match self.romm_browse.as_mut() {
                        Some(state) => {
                            if state.accepts_page(page, &page.cache) {
                                state.needs_reload = false;
                                state.page = Some(page.clone());
                            } else {
                                // The filters or the cache moved on while this was in
                                // flight. Mixing generations would show a page that
                                // answers a question nobody is asking any more.
                                state.needs_reload = true;
                            }
                            true
                        }
                        // The panel was closed before the page arrived.
                        None => true,
                    }
                }
                RommOperationOutcome::RecordDetail(detail) => {
                    if let Some(state) = self.romm_browse.as_mut() {
                        let requested_id = match &operation {
                            RommOperation::LoadRecordDetail { romm_game_id } => romm_game_id,
                            _ => unreachable!("a detail result must come from a detail request"),
                        };
                        if state.accepts_detail(requested_id, detail.as_ref().as_ref()) {
                            state.pending_detail_id = None;
                            state.detail = detail.as_ref().clone().map(Box::new);
                            state.detail_problem = detail.is_none().then(|| {
                                format!(
                                    "RomM record {requested_id} is no longer present in the \
                                     published cache. Refresh the records view."
                                )
                            });
                        }
                    }
                    true
                }
                RommOperationOutcome::Conflicts(page) => {
                    if let Some(state) = self.romm_browse.as_mut() {
                        if state.accepts_conflicts(page, &page.cache) {
                            state.conflicts = Some(page.clone());
                        } else {
                            state.needs_reload = true;
                        }
                    }
                    true
                }
                RommOperationOutcome::Stale(view) => {
                    if let Some(state) = self.romm_browse.as_mut() {
                        if state.accepts_stale(view, &view.cache) {
                            state.stale = Some(view.clone());
                        } else {
                            state.needs_reload = true;
                        }
                    }
                    self.romm_stale_progress = None;
                    true
                }
                RommOperationOutcome::GameIdentity(panel) => {
                    if self.romm_game.accepts_panel(panel) {
                        self.romm_game.panel = Some(panel.clone());
                        self.romm_game.needs_reload = false;
                    } else {
                        // The selection moved while this was in flight. Drawing it
                        // would attach one game's evidence to another's file.
                        self.romm_game.needs_reload = true;
                    }
                    true
                }
                RommOperationOutcome::Cover(outcome) => {
                    if self.romm_game.accepts_cover(outcome) {
                        // The texture is dropped here rather than in the renderer, so
                        // a cover cleared mid-fetch cannot leave the old pixels up.
                        self.romm_game.cover_texture = None;
                        self.romm_game.cover_key = None;
                        self.romm_game.cover = outcome.state.clone();
                        self.romm_game.cover_cache =
                            Some((outcome.cached_items, outcome.cached_bytes));
                    }
                    if let Some(state) = self.romm_browse.as_mut()
                        && state.accepts_cover(outcome)
                    {
                        state.detail_cover_texture = None;
                        state.detail_cover_key = None;
                        state.detail_cover = outcome.state.clone();
                    }
                    // A stale cover is deliberately discarded, but it is still a
                    // panel result rather than a source-card outcome banner.
                    true
                }
                RommOperationOutcome::Screenshot(outcome) => {
                    if self.romm_game.accepts_cover(outcome) {
                        self.romm_game.screenshot_texture = None;
                        self.romm_game.screenshot_key = None;
                        self.romm_game.screenshot = outcome.state.clone();
                    }
                    true
                }
                _ => false,
            };
            if landed {
                // Not a card result: browsing produces no outcome banner.
                self.romm_ui.last_outcome = previous_outcome;
                return;
            }
        }
        if let (RommOperation::LoadRecordDetail { romm_game_id }, Err(message)) =
            (&operation, &result)
            && let Some(state) = self.romm_browse.as_mut()
            && state.pending_detail_id.as_deref() == Some(romm_game_id)
        {
            state.pending_detail_id = None;
            state.detail_problem = Some(message.clone());
        }
        if let Ok(RommOperationOutcome::Verified(outcome)) = &result {
            // Both a card result and panel state: the panel inside it was rebuilt
            // from the verification that was just stored, so the verdict on screen is
            // recomputed rather than assumed.
            if self.romm_game.accepts_verification(outcome) {
                self.romm_game.panel = Some(outcome.panel.clone());
                self.romm_game.verification = Some(outcome.clone());
                self.romm_game.needs_reload = false;
            }
            self.romm_hash_progress = None;
        }
        // An import that actually *published* replaced the identity cache, so what any
        // local path resolves to may have changed. Gamer View's worker holds its path
        // index in memory - rebuilding it costs a full read of 36,259 records, which
        // is why it is rebuilt on this signal rather than on a timer or per frame.
        //
        // `published` is the exact condition, not merely "an import finished". A
        // sample import deliberately never publishes, and an import that failed
        // leaves the previous cache in place; refreshing for either would withdraw
        // every cover on screen to revalidate against a catalogue that had not
        // changed. This is also what keeps a failed import from discarding a working
        // index.
        if matches!(&result, Ok(RommOperationOutcome::Import(summary)) if summary.published) {
            if let Ok(RommOperationOutcome::Import(summary)) = &result {
                self.verify_romm_summary = Some(VerifyRommSummary::from_import(summary));
            }
            // Ready covers become `Revalidating`: their textures are kept so an
            // unchanged record costs no decode, but the placeholder is drawn until
            // the refreshed catalogue confirms the record, so a path whose provider
            // id moved cannot show the old cover even for one frame.
            self.gamer_covers.identity_refreshed();
            self.gamer_screenshots.library_changed();
            if let Some(worker) = self.gamer_cover_worker.as_ref() {
                worker.reindex();
            }
            // The same worker may have enriched platform metadata in the
            // existing catalogue. Reload the read-only snapshot so Library and
            // Gamer View show it immediately; no rescan or ROM read is needed.
            self.start_database_action(context.clone(), false);
        }
        if let Ok(RommOperationOutcome::Preview(summary)) = &result {
            // A preview is only meaningful while the dialog that asked for it is
            // open; if it has been closed, the result is dropped.
            if self.romm_config_draft.is_some() {
                self.romm_preview = Some(summary.clone());
            }
        }
        if let Ok(RommOperationOutcome::Saved(_)) = &result {
            // Saved, so the dialog has served its purpose and the card is reloaded
            // from disk rather than from what was typed.
            self.close_romm_configuration();
            self.verify_romm_summary = None;
            // This is a deliberate reload boundary. Rendering never reloads the
            // application configuration, and a failed reload retains the previous
            // usable snapshot instead of replacing it with an empty one.
            if let Err(error) = self.gui_config.reload_default() {
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message: format!(
                        "RomM was saved, but EmuWiz could not reload its main configuration: \
                         {error}. The previous in-memory configuration is still in use."
                    ),
                    cleanup: None,
                    warning: None,
                    more_information: Some(
                        "Fix config.toml, then use an explicit refresh or save again.".to_string(),
                    ),
                });
            }
        }
        if let Ok(RommOperationOutcome::Snapshot(snapshot)) = &result {
            // The one result that is not a user-visible outcome: it *is* the card's
            // state. A snapshot never overwrites a real result view.
            self.romm_snapshot = Some(snapshot.clone());
            self.verify_romm_summary = snapshot.verify_summary;
            self.romm_ui.last_outcome = previous_outcome;
            return;
        }
        // A mutating operation may have changed what the card shows, so the card is
        // refreshed from authoritative state rather than from an optimistic counter.
        // A failure leaves the previous snapshot in place until the reload replaces
        // it, so a failed refresh never erases counts that were true.
        if operation.is_mutating() {
            self.start_romm_status_load(context.clone());
        }
    }

    /// Loads the snapshot. Separate from `start_romm_operation` so it can run
    /// straight after another operation completes.
    pub(crate) fn start_romm_status_load(&mut self, context: egui::Context) {
        if self.romm_operation.is_some() {
            return;
        }
        self.romm_generation = self.romm_generation.wrapping_add(1);
        let generation = self.romm_generation;
        let cancellation = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let (_progress_sender, progress_receiver) = mpsc::channel();
        self.romm_operation = Some(RunningRommOperation {
            generation,
            operation: RommOperation::LoadStatus,
            cancellation,
            receiver,
            progress_receiver,
            progress: None,
            cancellation_requested: false,
        });
        thread::spawn(move || {
            let result = load_romm_snapshot()
                .map(|snapshot| RommOperationOutcome::Snapshot(Box::new(snapshot)));
            let _ = sender.send((generation, result));
            context.request_repaint();
        });
    }

    /// Opens the configuration dialog on whatever is actually stored.
    ///
    /// Opening it twice is impossible: the draft *is* the open flag, so a second
    /// request while one is open is a no-op rather than a second dialog.
    pub(crate) fn open_romm_configuration(&mut self) {
        if self.romm_config_draft.is_some() {
            return;
        }
        let draft = match self.romm_snapshot.as_deref() {
            Some(snapshot) => RommConfigDraft::from_snapshot(snapshot),
            // A source that has never been configured still needs the dialog - that
            // is the only way it ever gets configured.
            None => RommConfigDraft::blank(),
        };
        self.romm_config_draft = Some(Box::new(draft));
        self.romm_preview = None;
    }

    /// Opens the existing safe configuration editor with only the reviewed
    /// RomM mappings changed. The final Save button remains the explicit apply
    /// confirmation and its worker validates the whole configuration again.
    pub(crate) fn open_romm_mapping_plan(&mut self) {
        if self.romm_config_draft.is_some() {
            return;
        }
        let (Some(snapshot), Some(plan)) = (
            self.romm_snapshot.as_deref(),
            self.romm_ui.mapping_plan.as_deref(),
        ) else {
            return;
        };
        let mut proposed = snapshot.clone();
        proposed.settings.source.mappings = plan.proposed_mappings.clone();
        let mut draft = RommConfigDraft::from_snapshot(&proposed);
        draft.dirty = true;
        self.romm_config_draft = Some(Box::new(draft));
        self.romm_preview = None;
    }

    /// Draws the configuration dialog.
    ///
    /// Split out so the borrow of the draft is over before a request is handled -
    /// which is what lets a save or a close mutate the same state the dialog was
    /// just drawn from.
    pub(crate) fn show_romm_configuration(&mut self, ui: &mut egui::Ui) -> Option<ConfigDialogRequest> {
        let source_roots_result = self.gui_config.source_roots().map(Vec::from);
        let source_roots = source_roots_result.clone().unwrap_or_default();
        let busy = self
            .romm_operation
            .as_ref()
            .is_some_and(|running| running.operation.blocks_actions());
        let preview_running = self
            .romm_operation
            .as_ref()
            .is_some_and(|running| matches!(running.operation, RommOperation::Preview { .. }));
        let previous = self
            .romm_snapshot
            .as_deref()
            .map(|snapshot| snapshot.settings.clone());
        let preview = self.romm_preview.clone();

        let draft = self.romm_config_draft.as_mut()?;
        // The token file's verdict comes from the core loader, and only its verdict:
        // the contents are never read into the GUI.
        let token_state = {
            let trimmed = draft.token_path.trim();
            if trimmed.is_empty() {
                None
            } else {
                let path = PathBuf::from(trimmed);
                Some(token_field_state(
                    archivefs_core::identity_source::settings::load_token_file(Some(&path))
                        .map(|_| ()),
                    trimmed,
                ))
            }
        };
        let validation = validate_draft(draft, token_state.as_ref(), &source_roots);
        let mappings = build_mappings_view(&draft.mappings, draft.path_kind, &source_roots);
        if let Err(problem) = source_roots_result {
            widgets::banner(
                ui,
                "EmuWiz configuration unavailable",
                &format!(
                    "The previous in-memory configuration is being preserved, but this dialog \
                     cannot validate source-folder mappings: {problem}"
                ),
                widgets::StatusTone::Blocked,
            );
        }
        show_config_dialog(
            ui,
            draft,
            &crate::romm_config::ConfigDialogInputs {
                validation: &validation,
                mappings: &mappings,
                preview: preview.as_deref(),
                previous: previous.as_ref(),
                busy,
                preview_running,
            },
            &mut self.clipboard,
        )
    }

    /// The configuration dialog's fixed footer - Save and Cancel.
    ///
    /// Split from `show_romm_configuration` so the window wrapper can place it
    /// outside the body's scroll area. It re-derives the same validation the
    /// body did rather than caching it, because the body may have just changed
    /// a field this frame and a stale `can_save` would be worse than the very
    /// small cost of recomputing it.
    pub(crate) fn show_romm_configuration_footer(&mut self, ui: &mut egui::Ui) -> Option<ConfigDialogRequest> {
        let source_roots = self
            .gui_config
            .source_roots()
            .map(Vec::from)
            .unwrap_or_default();
        let busy = self
            .romm_operation
            .as_ref()
            .is_some_and(|running| running.operation.blocks_actions());
        let preview_running = self
            .romm_operation
            .as_ref()
            .is_some_and(|running| matches!(running.operation, RommOperation::Preview { .. }));
        let previous = self
            .romm_snapshot
            .as_deref()
            .map(|snapshot| snapshot.settings.clone());
        let preview = self.romm_preview.clone();

        let draft = self.romm_config_draft.as_mut()?;
        let token_state = {
            let trimmed = draft.token_path.trim();
            if trimmed.is_empty() {
                None
            } else {
                let path = PathBuf::from(trimmed);
                Some(token_field_state(
                    archivefs_core::identity_source::settings::load_token_file(Some(&path))
                        .map(|_| ()),
                    trimmed,
                ))
            }
        };
        let validation = validate_draft(draft, token_state.as_ref(), &source_roots);
        let mappings = build_mappings_view(&draft.mappings, draft.path_kind, &source_roots);
        crate::romm_config::show_config_dialog_footer(
            ui,
            draft,
            &crate::romm_config::ConfigDialogInputs {
                validation: &validation,
                mappings: &mappings,
                preview: preview.as_deref(),
                previous: previous.as_ref(),
                busy,
                preview_running,
            },
        )
    }

    /// Draws the existing configuration content as an actual persistent window.
    ///
    /// Previously the content was appended below the source card, outside the
    /// visible scroll position. The draft still remains the single open/closed flag;
    /// this wrapper only gives that state a visible destination.
    ///
    /// # Window shape
    ///
    /// The same arrangement the RomM record Details window uses, for the same
    /// reason: a title-bar close (`.open`), a body that scrolls within a
    /// viewport-clamped window, and a footer holding Save and Cancel that is
    /// drawn *after* the body's scroll area so it can never scroll away. The
    /// previous version had none of these - the actions were the last widgets
    /// inside the scrolling body, so at TV resolution the dialog offered no
    /// visible exit at all and Escape was the only way out.
    pub(crate) fn show_romm_configuration_window(
        &mut self,
        context: &egui::Context,
    ) -> Option<ConfigDialogRequest> {
        self.romm_config_draft.as_ref()?;
        let mut request = None;
        let viewport = context.input(|input| input.screen_rect().size());
        let (initial, maximum) = romm_dialog_sizes(viewport, egui::vec2(640.0, 760.0));
        // egui only reports a title-bar close by clearing this flag, so it has
        // to outlive the closure.
        let mut open = true;
        egui::Window::new("Configure RomM")
            .id(egui::Id::new("romm_configuration_dialog"))
            .collapsible(false)
            .resizable(true)
            .open(&mut open)
            .default_size(initial)
            .max_size(maximum)
            // egui keeps a constrained window fully inside the screen, which
            // is what guarantees the fixed footer stays reachable.
            .constrain(true)
            .default_pos(egui::pos2(24.0, 24.0))
            .show(context, |ui| {
                let footer_height = crate::romm_config::CONFIG_FOOTER_HEIGHT;
                // Bounded from both directions, and both bounds matter.
                //
                // `available_height` alone is not enough: with content taller
                // than the screen it is effectively unbounded, the scroll area
                // claimed all of it, and the window grew past its own maximum
                // until the fixed footer sat below the bottom of the screen.
                //
                // The window maximum alone is not enough either: sizing the
                // body from it consumed the whole content area, and the
                // separator and footer were then laid out past the window's
                // clip rect, where they were never painted at all.
                //
                // Taking the smaller of the two keeps the footer inside the
                // window *and* the window inside the viewport.
                let body_height = crate::romm_config::config_body_height(
                    ui.available_height().min(romm_window_body_cap(maximum)),
                    footer_height,
                );
                egui::ScrollArea::vertical()
                    .id_salt("romm_configuration_body")
                    .scroll_bar_visibility(
                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                    )
                    .max_height(body_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| request = self.show_romm_configuration(ui));
                ui.separator();
                if let Some(found) = self.show_romm_configuration_footer(ui) {
                    request = Some(found);
                }
            });
        // A title-bar close and Escape are both plain "leave without writing":
        // `Close` discards the draft and performs no save, no import and no
        // request. Neither is allowed to overwrite a Save the footer just
        // produced in this same frame.
        if request.is_none()
            && (!open || context.input(|input| input.key_pressed(egui::Key::Escape)))
        {
            request = Some(ConfigDialogRequest::Close);
        }
        request
    }

    /// Opens the browsing panel on one view, or switches an open one to it.
    ///
    /// Opening it twice is impossible for the same reason the configuration dialog
    /// cannot be: the state *is* the open flag.
    pub(crate) fn open_romm_browse(&mut self, view: crate::romm_browse::BrowseView) {
        match self.romm_browse.as_mut() {
            Some(state) if state.view != view => {
                state.view = view;
                state.detail = None;
                state.pending_detail_id = None;
                state.detail_problem = None;
            }
            Some(_) => {}
            None => {
                self.romm_browse = Some(Box::new(crate::romm_browse::BrowseState::opened_at(view)));
            }
        }
    }

    pub(crate) fn close_romm_browse(&mut self) {
        self.romm_browse = None;
        self.romm_stale_progress = None;
    }

    /// Draws the RomM records browser as its own persistent window.
    ///
    /// # Why a window
    ///
    /// It used to be appended to the Sources page underneath the RomM source
    /// card. On a page already taller than the viewport that put the browser
    /// entirely below the fold, so one click on "Browse records" produced no
    /// visible change whatsoever and read as a dead button. A window appears
    /// where the user is looking, on the frame the button is pressed.
    ///
    /// Opening twice cannot duplicate it: `self.romm_browse` *is* the open
    /// flag (see `open_romm_browse`), and the window carries a fixed id, so a
    /// second click at most switches which view is shown.
    ///
    /// Nothing here changes what browsing *does*: it still reads the published
    /// identity cache only. No transport is constructed, no token is read and
    /// no request is made - that is `handle_romm_browse_request`'s business,
    /// and it is untouched.
    pub(crate) fn show_romm_browse_window(
        &mut self,
        context: &egui::Context,
    ) -> Option<crate::romm_browse::BrowseRequest> {
        self.romm_browse.as_ref()?;
        let busy = self
            .romm_operation
            .as_ref()
            .is_some_and(|running| running.operation.blocks_actions());
        let progress = self.romm_stale_progress;
        let viewport = context.input(|input| input.screen_rect().size());
        let (initial, maximum) = romm_dialog_sizes(viewport, egui::vec2(900.0, 780.0));
        let title = self
            .romm_browse
            .as_ref()
            .map(|state| state.view.title())
            .unwrap_or("RomM records");
        let mut request = None;
        let mut open = true;
        egui::Window::new(title)
            .id(egui::Id::new("romm_browse_window"))
            .collapsible(false)
            .resizable(true)
            .open(&mut open)
            .default_size(initial)
            .max_size(maximum)
            // egui keeps a constrained window fully inside the screen, which
            // is what guarantees the fixed footer stays reachable.
            .constrain(true)
            .default_pos(egui::pos2(24.0, 24.0))
            .show(context, |ui| {
                let footer_height = crate::romm_browse::BROWSE_FOOTER_HEIGHT;
                // See `show_romm_configuration_window` for why both bounds
                // are needed. The records list is the case that proves it:
                // its content is far taller than any screen.
                let body_height = crate::romm_config::config_body_height(
                    ui.available_height().min(romm_window_body_cap(maximum)),
                    footer_height,
                );
                egui::ScrollArea::vertical()
                    .id_salt("romm_browse_body")
                    .scroll_bar_visibility(
                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                    )
                    .max_height(body_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(found) = self.romm_browse.as_mut().and_then(|state| {
                            crate::romm_browse::show_browse_panel(
                                ui,
                                state,
                                busy,
                                progress.as_ref(),
                            )
                        }) {
                            request = Some(found);
                        }
                    });
                ui.separator();
                if let Some(found) = crate::romm_browse::show_browse_panel_footer(ui) {
                    request = Some(found);
                }
            });
        // Escape belongs to the record Details window whenever one is open -
        // it closes that, not the browser underneath it - so the browser only
        // consumes Escape when nothing is layered on top.
        let detail_open = self
            .romm_browse
            .as_ref()
            .is_some_and(|state| state.detail.is_some());
        if request.is_none()
            && (!open
                || (!detail_open && context.input(|input| input.key_pressed(egui::Key::Escape))))
        {
            request = Some(crate::romm_browse::BrowseRequest::Close);
        }
        request
    }

    /// Routes one request from the browsing panel.
    pub(crate) fn handle_romm_browse_request(
        &mut self,
        context: &egui::Context,
        request: crate::romm_browse::BrowseRequest,
    ) {
        use crate::romm_browse::BrowseRequest;
        match request {
            BrowseRequest::LoadRecords { offset, limit } => {
                if let Some(state) = self.romm_browse.as_mut() {
                    state.invalidate_detail_request();
                }
                let filters = self
                    .romm_browse
                    .as_ref()
                    .map(|state| state.filters.clone())
                    .unwrap_or_default();
                self.start_romm_operation(
                    context.clone(),
                    RommOperation::LoadRecords {
                        filters: Box::new(filters),
                        offset,
                        limit,
                    },
                );
            }
            BrowseRequest::OpenDetail { romm_game_id } => {
                let requested_id = romm_game_id.clone();
                if self.start_romm_operation(
                    context.clone(),
                    RommOperation::LoadRecordDetail { romm_game_id },
                ) && let Some(state) = self.romm_browse.as_mut()
                {
                    state.begin_detail(requested_id);
                }
            }
            BrowseRequest::CloseDetail => {
                if let Some(state) = self.romm_browse.as_mut() {
                    state.detail = None;
                    state.pending_detail_id = None;
                    state.detail_problem = None;
                    state.detail_cover = crate::romm_game::CoverState::Idle;
                    state.detail_cover_texture = None;
                    state.detail_cover_key = None;
                }
            }
            BrowseRequest::LoadDetailCover {
                local_path,
                romm_game_id,
            } => {
                if self.start_romm_operation(
                    context.clone(),
                    RommOperation::LoadCover {
                        local_path,
                        romm_game_id,
                    },
                ) && let Some(state) = self.romm_browse.as_mut()
                {
                    state.detail_cover = crate::romm_game::CoverState::Loading;
                }
            }
            BrowseRequest::LoadConflicts { offset } => {
                self.start_romm_operation(context.clone(), RommOperation::LoadConflicts { offset });
            }
            BrowseRequest::RunStaleSummary => {
                self.romm_stale_progress = None;
                self.start_romm_operation(context.clone(), RommOperation::StaleSummary);
            }
            BrowseRequest::Cancel => self.cancel_romm_operation(),
            BrowseRequest::Switch(view) => self.open_romm_browse(view),
            BrowseRequest::Close => self.close_romm_browse(),
        }
    }

    /// What EmuWiz itself says the selected archive's platform is.
    ///
    /// Read from the catalogue the GUI already holds. `manual` is the whole point: it
    /// makes the assignment count as verified local evidence, which is what stops a
    /// disagreeing RomM record from being presented as a correction.
    pub(crate) fn romm_local_platform(&self, path: &Path) -> crate::romm_game::LocalPlatformClaim {
        let persisted = self.database_state.snapshot().and_then(|snapshot| {
            snapshot
                .archives
                .iter()
                .find(|archive| archive.absolute_path == path)
        });
        match persisted {
            Some(archive) => crate::romm_game::LocalPlatformClaim {
                platform: archive.platform.clone(),
                manual: archive.platform_source.as_deref() == Some(MANUAL_PLATFORM_SOURCE),
            },
            None => crate::romm_game::LocalPlatformClaim::default(),
        }
    }

    /// Draws the selected game's RomM panel, and follows the selection.
    pub(crate) fn show_romm_game_panel(&mut self, context: &egui::Context, ui: &mut egui::Ui) {
        // Following the selection discards the previous game's panel, cover and
        // verification, but starts nothing: a lookup is a button press.
        self.romm_game
            .focus(self.archive_context.focused.as_deref());
        let running = self
            .romm_operation
            .as_ref()
            .map(|running| &running.operation);
        let busy = running.is_some_and(RommOperation::blocks_actions);
        let busy_reason = running.map(|operation| operation.label());
        let cache_present = self
            .romm_snapshot
            .as_deref()
            .is_some_and(|snapshot| snapshot.status.records_imported > 0);
        let hash_progress = self.romm_hash_progress.clone();
        let request = crate::romm_game::show_game_identity_panel(
            ui,
            &mut self.romm_game,
            &crate::romm_game::GamePanelInputs {
                busy,
                busy_reason,
                hash_progress: hash_progress.as_ref(),
                cache_present,
            },
        );
        if let Some(request) = request {
            self.handle_romm_game_request(context, request);
        }
    }

    pub(crate) fn handle_romm_game_request(
        &mut self,
        context: &egui::Context,
        request: crate::romm_game::GamePanelRequest,
    ) {
        use crate::romm_game::GamePanelRequest;

        let Some(local_path) = self.romm_game.local_path.clone() else {
            return;
        };
        let local_platform = Box::new(self.romm_local_platform(&local_path));
        match request {
            GamePanelRequest::Resolve => {
                self.start_romm_operation(
                    context.clone(),
                    RommOperation::ResolveGame {
                        local_path,
                        local_platform,
                        chosen_game_id: self.romm_game.chosen_game_id.clone(),
                    },
                );
            }
            GamePanelRequest::Choose { romm_game_id } => {
                // A choice is recorded and then re-resolved, so the verdict on screen
                // is the one that record actually earns rather than a relabelling.
                self.romm_game.chosen_game_id = Some(romm_game_id.clone());
                self.romm_game.verification = None;
                self.romm_game.cover = crate::romm_game::CoverState::Idle;
                self.romm_game.cover_texture = None;
                self.romm_game.cover_key = None;
                self.start_romm_operation(
                    context.clone(),
                    RommOperation::ResolveGame {
                        local_path,
                        local_platform,
                        chosen_game_id: Some(romm_game_id),
                    },
                );
            }
            GamePanelRequest::Verify { romm_game_id } => {
                self.start_romm_operation(
                    context.clone(),
                    RommOperation::VerifyLocalFile {
                        local_path,
                        romm_game_id,
                        local_platform,
                        chosen_game_id: self.romm_game.chosen_game_id.clone(),
                    },
                );
            }
            GamePanelRequest::LoadCover { romm_game_id } => {
                if self.start_romm_operation(
                    context.clone(),
                    RommOperation::LoadCover {
                        local_path,
                        romm_game_id,
                    },
                ) {
                    self.romm_game.cover = crate::romm_game::CoverState::Loading;
                }
            }
            GamePanelRequest::LoadScreenshot { romm_game_id } => {
                if self.start_romm_operation(
                    context.clone(),
                    RommOperation::LoadScreenshot {
                        local_path,
                        romm_game_id,
                    },
                ) {
                    self.romm_game.screenshot = crate::romm_game::CoverState::Loading;
                }
            }
            GamePanelRequest::OpenManual { romm_game_id } => {
                self.start_romm_operation(
                    context.clone(),
                    RommOperation::OpenManual {
                        local_path,
                        romm_game_id,
                    },
                );
            }
            GamePanelRequest::Cancel => self.cancel_romm_operation(),
            GamePanelRequest::Close => {
                // Closed for this selection only. Choosing a different archive brings
                // it back, which is what someone expects from a per-game panel.
                self.romm_game.dismissed = true;
            }
            GamePanelRequest::Reopen => {
                self.romm_game.dismissed = false;
            }
        }
    }

    pub(crate) fn close_romm_configuration(&mut self) {
        self.romm_config_draft = None;
        self.romm_preview = None;
    }

    /// Routes one request from the configuration dialog.
    pub(crate) fn handle_romm_config_request(
        &mut self,
        context: &egui::Context,
        request: ConfigDialogRequest,
    ) {
        match request {
            ConfigDialogRequest::Save(settings) => {
                // Declined while anything else runs, so a save cannot race an import.
                if self.start_romm_operation(
                    context.clone(),
                    RommOperation::SaveConfiguration(settings),
                ) {
                    // The dialog stays open until the save succeeds, so a refused
                    // save does not lose what was typed.
                }
            }
            ConfigDialogRequest::Preview { limit } => {
                self.start_romm_operation(context.clone(), RommOperation::Preview { limit });
            }
            ConfigDialogRequest::CancelPreview => self.cancel_romm_operation(),
            ConfigDialogRequest::Close => self.close_romm_configuration(),
        }
    }


}
