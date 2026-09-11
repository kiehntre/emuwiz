use crate::*;

impl ArchiveFsApp {
    /// GUI Batch A: starts (or restarts, on a new selection) the real,
    /// off-UI-thread evidence gather for the Selected page's identity
    /// panel - see `selected_evidence_page::gather_selected_evidence`.
    /// Started automatically when the selected game changes so identity
    /// evidence is available in the selected-game details surface and to
    /// the launch planner. The worker remains generation-guarded and
    /// read-only.
    pub(crate) fn start_selected_evidence_load(&mut self, context: egui::Context, path: PathBuf) {
        self.cancel_selected_evidence_work();
        self.selected_evidence_generation += 1;
        let generation = self.selected_evidence_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.selected_evidence_cancel = Some(Arc::clone(&cancel));
        let (sender, receiver) = mpsc::channel();
        self.selected_evidence = selected_evidence_page::SelectedEvidenceState::Loading {
            generation,
            path: path.clone(),
            receiver,
        };
        // A new selection invalidates any in-flight or completed enrichment
        // pass for the previous file.
        self.selected_evidence_enrichment = SelectedEvidenceEnrichmentState::Idle;
        let platform_hint = match &self.state {
            LoadState::Ready(data) => data
                .records
                .iter()
                .find(|record| record.mount_plan.archive.path == path)
                .and_then(|record| {
                    record
                        .metadata
                        .platform
                        .as_deref()
                        .or(record.identity.platform.as_deref())
                })
                .map(str::to_owned),
            LoadState::Loading { .. } | LoadState::Error(_) => None,
        };
        thread::spawn(move || {
            // Fast pass only: bounded header read + the bounded per-platform
            // identity inspector. For loose files, the whole-file checksum
            // and No-Intro DAT resolution are the deferred enrichment pass
            // (`start_selected_evidence_enrichment`). Compressed archives
            // terminate after bounded member evidence, so neither path can
            // hold back the visible identity.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                selected_evidence_page::gather_selected_evidence_fast(
                    &path,
                    platform_hint.as_deref(),
                )
            }))
            .unwrap_or_else(|_| {
                Err(
                    "the identity worker stopped unexpectedly while inspecting this file"
                        .to_string(),
                )
            });
            let result = if cancel.load(Ordering::Relaxed) {
                Err("identity inspection was cancelled after the selection changed".to_string())
            } else {
                result
            };
            let _ = sender.send((generation, result));
            context.request_repaint();
        });
    }

    pub(crate) fn cancel_selected_evidence_work(&mut self) {
        if let Some(cancel) = self.selected_evidence_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Cancels and detaches evidence work as soon as Library focus no longer
    /// names the path represented by the state machine. Generation checks
    /// still guard every reply; cancellation additionally stops the costly
    /// hash instead of allowing stale I/O to continue in the background.
    pub(crate) fn reconcile_selected_evidence_selection(&mut self) {
        let state_path = match &self.selected_evidence {
            selected_evidence_page::SelectedEvidenceState::Loading { path, .. }
            | selected_evidence_page::SelectedEvidenceState::Error { path, .. } => Some(path),
            selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
                Some(&report.path)
            }
            selected_evidence_page::SelectedEvidenceState::Idle => None,
        };
        if state_path.map(PathBuf::as_path) != self.archive_context.focused.as_deref()
            && state_path.is_some()
        {
            self.cancel_selected_evidence_work();
            self.selected_evidence = selected_evidence_page::SelectedEvidenceState::Idle;
            self.selected_evidence_enrichment = SelectedEvidenceEnrichmentState::Idle;
        }
    }

    /// Starts the deferred enrichment pass for an already-visible `Ready`
    /// loose-file report whose `hashes` are not yet filled in: the whole-file
    /// checksum and the No-Intro DAT lookup, both resolved entirely off the UI
    /// thread. Generation-guarded like every other background loader; a new
    /// selection (which bumps `selected_evidence_generation` and resets the
    /// enrichment state to `Idle`) makes a late result be discarded. Archive
    /// reports never call this method.
    pub(crate) fn start_selected_evidence_enrichment(
        &mut self,
        context: egui::Context,
        path: PathBuf,
        generation: u64,
        platform: Option<String>,
    ) {
        let (sender, receiver) = mpsc::channel();
        self.selected_evidence_enrichment = SelectedEvidenceEnrichmentState::Loading {
            generation,
            path: path.clone(),
            receiver,
        };
        let cancel = Arc::clone(
            self.selected_evidence_cancel
                .get_or_insert_with(|| Arc::new(AtomicBool::new(false))),
        );
        let no_intro_source_cache = Arc::clone(&self.no_intro_source_cache);
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if cancel.load(Ordering::Relaxed) {
                    return Err(
                        "additional evidence was cancelled after the selection changed".to_string(),
                    );
                }
                let config_path = archivefs_core::dat::sources::default_dat_sources_config_path();
                let resolved_sources = config_path
                    .as_deref()
                    .ok()
                    .and_then(|config_path| {
                        archivefs_core::dat::sources::load_dat_sources_config_from(config_path).ok()
                    })
                    .map(|config| {
                        archivefs_core::dat::sources::DatSourceRegistry::from_config(&config).0
                    })
                    .map(|registry| {
                        no_intro_source_cache
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .resolve_all(&registry, platform.as_deref())
                            .to_vec()
                    })
                    .unwrap_or_default();
                if cancel.load(Ordering::Relaxed) {
                    return Err(
                        "additional evidence was cancelled after the selection changed".to_string(),
                    );
                }
                let source_refs: Vec<_> = resolved_sources
                    .iter()
                    .map(|(label, source)| (Some(label), source.as_ref()))
                    .collect();
                selected_evidence_page::compute_selected_evidence_enrichment_from_sources(
                    &path,
                    &source_refs,
                    Some(&cancel),
                )
            }))
            .unwrap_or_else(|_| {
                Err("the additional-evidence worker stopped unexpectedly".to_string())
            });
            let _ = sender.send((generation, result));
            context.request_repaint();
        });
    }

    /// If the fast pass has produced a `Ready` report for the current
    /// selection whose whole-file `hashes` are still unset, and no
    /// enrichment pass has been started or finished for it, start one. Kept
    /// out of `poll_selected_evidence` so it can read the freshly-settled
    /// `Ready` state and the live record set in the same frame.
    pub(crate) fn maybe_start_selected_evidence_enrichment(&mut self, context: &egui::Context) {
        let selected_evidence_page::SelectedEvidenceState::Ready {
            generation, report, ..
        } = &self.selected_evidence
        else {
            return;
        };
        if !matches!(
            report.enrichment,
            selected_evidence_page::SelectedEvidenceEnrichmentStatus::Pending
        ) {
            return;
        }
        let generation = *generation;
        let path = report.path.clone();
        let already = match &self.selected_evidence_enrichment {
            SelectedEvidenceEnrichmentState::Idle => false,
            SelectedEvidenceEnrichmentState::Loading {
                generation: g,
                path: p,
                ..
            }
            | SelectedEvidenceEnrichmentState::Done {
                generation: g,
                path: p,
            } => *g == generation && *p == path,
        };
        if already {
            return;
        }
        let platform = report
            .identity
            .platform
            .map(str::to_owned)
            .or_else(|| Some(report.game_identity_report.platform.label().to_string()));
        self.start_selected_evidence_enrichment(context.clone(), path, generation, platform);
    }

    /// GUI Batch A: the explicit "Check Hasheous" action - a real network
    /// call, always off the UI thread, never started automatically. Uses
    /// the adapter's own default host/timeout constants; clicking the
    /// button is itself the opt-in this batch requires.
    pub(crate) fn start_selected_hasheous_check(&mut self, context: egui::Context) {
        let selected_evidence_page::SelectedEvidenceState::Ready {
            generation, report, ..
        } = &mut self.selected_evidence
        else {
            return;
        };
        let Some(sha1) = report.hashes.as_ref().map(|hashes| hashes.sha1.clone()) else {
            return;
        };
        let generation = *generation;
        let (sender, receiver) = mpsc::channel();
        if let selected_evidence_page::SelectedEvidenceState::Ready { hasheous, .. } =
            &mut self.selected_evidence
        {
            *hasheous = selected_evidence_page::HasheousState::Loading {
                generation,
                receiver,
            };
        }
        thread::spawn(move || {
            use archivefs_core::identity_source::hasheous::client::{
                HASHEOUS_DEFAULT_BASE_URL, HasheousConfig, REQUEST_TIMEOUT,
            };
            let config = HasheousConfig {
                enabled: true,
                base_url: HASHEOUS_DEFAULT_BASE_URL.to_string(),
                timeout: REQUEST_TIMEOUT,
            };
            let outcome = selected_evidence_page::run_hasheous_check_live(&config, &sha1);
            let _ = sender.send((generation, outcome));
            context.request_repaint();
        });
    }

    /// GUI Batch A: applies the pure [`selected_evidence_page::SelectedEvidenceAction`]
    /// the panel returned this frame - the only two things it can ever ask
    /// for, both read-only.
    pub(crate) fn handle_selected_evidence_action(
        &mut self,
        context: &egui::Context,
        action: Option<selected_evidence_page::SelectedEvidenceAction>,
    ) {
        match action {
            Some(selected_evidence_page::SelectedEvidenceAction::Load(path)) => {
                self.start_selected_evidence_load(context.clone(), path);
            }
            Some(selected_evidence_page::SelectedEvidenceAction::CheckHasheous) => {
                self.start_selected_hasheous_check(context.clone());
            }
            None => {}
        }
    }

    /// GUI Batch A: drains a completed evidence-load or Hasheous-check
    /// message, discarding anything whose generation is no longer current
    /// (the same stale-result guard every other background loader in this
    /// app uses).
    pub(crate) fn poll_selected_evidence(&mut self) {
        let base_result = match &self.selected_evidence {
            selected_evidence_page::SelectedEvidenceState::Loading {
                generation,
                receiver,
                ..
            } => match receiver.try_recv() {
                Ok((message_generation, result)) => Some((*generation, message_generation, result)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    *generation,
                    *generation,
                    Err("the identity worker stopped without returning a result".to_string()),
                )),
            },
            _ => None,
        };
        if let Some((state_generation, message_generation, result)) = base_result
            && state_generation == self.selected_evidence_generation
            && message_generation == state_generation
        {
            let path = match &self.selected_evidence {
                selected_evidence_page::SelectedEvidenceState::Loading { path, .. } => path.clone(),
                _ => return,
            };
            self.selected_evidence = match result {
                Ok(report) => selected_evidence_page::SelectedEvidenceState::Ready {
                    generation: message_generation,
                    report: Box::new(report),
                    hasheous: selected_evidence_page::HasheousState::Idle,
                },
                Err(message) => selected_evidence_page::SelectedEvidenceState::Error {
                    generation: message_generation,
                    path,
                    message,
                },
            };
        }
        // Two separate borrows of `self.selected_evidence` (read to poll the
        // channel, then a fresh mutable one to write the result) rather than
        // one collapsed condition - the write must start after the read
        // borrow above has already ended.
        #[allow(clippy::collapsible_if)]
        if let selected_evidence_page::SelectedEvidenceState::Ready { hasheous, .. } =
            &self.selected_evidence
            && let selected_evidence_page::HasheousState::Loading {
                generation,
                receiver,
            } = hasheous
            && let Ok((message_generation, outcome)) = receiver.try_recv()
            && message_generation == *generation
        {
            if let selected_evidence_page::SelectedEvidenceState::Ready { hasheous, .. } =
                &mut self.selected_evidence
            {
                *hasheous = selected_evidence_page::HasheousState::Done {
                    generation: message_generation,
                    outcome,
                };
            }
        }

        // Drain a completed deferred enrichment pass (whole-file checksum +
        // No-Intro lookup) and merge it into the `Ready` report the panel is
        // already showing, if the selection has not moved on since.
        let enrichment_result = match &self.selected_evidence_enrichment {
            SelectedEvidenceEnrichmentState::Loading {
                generation,
                receiver,
                ..
            } => match receiver.try_recv() {
                Ok((message_generation, result)) => Some((*generation, message_generation, result)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    *generation,
                    *generation,
                    Err(
                        "the additional-evidence worker stopped without returning a result"
                            .to_string(),
                    ),
                )),
            },
            SelectedEvidenceEnrichmentState::Idle
            | SelectedEvidenceEnrichmentState::Done { .. } => None,
        };
        if let Some((state_generation, message_generation, result)) = enrichment_result
            && state_generation == self.selected_evidence_generation
            && message_generation == state_generation
        {
            let (generation, path) = match &self.selected_evidence_enrichment {
                SelectedEvidenceEnrichmentState::Loading {
                    generation, path, ..
                } => (*generation, path.clone()),
                SelectedEvidenceEnrichmentState::Idle
                | SelectedEvidenceEnrichmentState::Done { .. } => return,
            };
            if let selected_evidence_page::SelectedEvidenceState::Ready {
                generation: ready_generation,
                report,
                ..
            } = &mut self.selected_evidence
                && *ready_generation == generation
                && report.path == path
            {
                match result {
                    Ok(enrichment) => {
                        selected_evidence_page::apply_selected_evidence_enrichment(
                            report, enrichment,
                        );
                    }
                    Err(message) => {
                        selected_evidence_page::apply_selected_evidence_enrichment_error(
                            report, message,
                        );
                    }
                }
            }
            self.selected_evidence_enrichment =
                SelectedEvidenceEnrichmentState::Done { generation, path };
        }
    }

    /// GUI Batch B: starts (or refreshes) the read-only "Sources &
    /// Providers" status load - see `identity_sources_page`'s own module
    /// doc. Explicit only (a button press); never called automatically.
    pub(crate) fn start_identity_sources_load(&mut self, context: egui::Context) {
        self.identity_sources_generation += 1;
        let generation = self.identity_sources_generation;
        let (sender, receiver) = mpsc::channel();
        self.identity_sources = identity_sources_page::IdentitySourcesState::Loading {
            generation,
            receiver,
        };
        thread::spawn(move || {
            let config_path = archivefs_core::dat::sources::default_dat_sources_config_path();
            let status =
                identity_sources_page::gather_no_intro_sources_status(config_path.as_deref().ok());
            let _ = sender.send((generation, status));
            context.request_repaint();
        });
    }

    /// GUI Batch B: applies the pure
    /// [`identity_sources_page::IdentitySourcesAction`] the panel returned
    /// this frame - the only thing it can ever ask for, and read-only.
    pub(crate) fn handle_identity_sources_action(
        &mut self,
        context: &egui::Context,
        action: Option<identity_sources_page::IdentitySourcesAction>,
    ) {
        if let Some(identity_sources_page::IdentitySourcesAction::Load) = action {
            self.start_identity_sources_load(context.clone());
        }
    }

    /// GUI Batch B: drains a completed sources-status load, discarding
    /// anything whose generation is no longer current - the same
    /// stale-result guard `poll_selected_evidence` already uses.
    pub(crate) fn poll_identity_sources(&mut self) {
        if let identity_sources_page::IdentitySourcesState::Loading {
            generation,
            receiver,
        } = &self.identity_sources
            && let Ok((message_generation, status)) = receiver.try_recv()
            && message_generation == *generation
        {
            self.identity_sources = identity_sources_page::IdentitySourcesState::Ready {
                generation: message_generation,
                status,
            };
        }
    }


}
