use crate::*;

impl ArchiveFsApp {
    /// Whether a new platform-assignment action may start: not already
    /// running one (single-row *or* bulk - only one platform-metadata
    /// writer at a time, since both ultimately write the same
    /// `platform_assignments` table), and not in the middle of a database
    /// load/scan (the same "one database writer at a time" convention
    /// `start_database_action`'s own UI already enforces by disabling its
    /// buttons while loading - see `show_database_panel`). This never
    /// touches `is_busy()`/mount safety - platform assignment is
    /// metadata-only and deliberately independent of it.
    pub(crate) fn platform_action_available(&self) -> bool {
        self.platform_action.is_none()
            && self.bulk_platform_action.is_none()
            && self.alias_action.is_none()
            && self.missing_removal.is_none()
            && self.source_action.is_none()
            && self.library_view_action.is_none()
            && !self.database_state.is_loading()
    }

    /// The bulk counterpart to `platform_action_available` - see its doc
    /// comment for why single-row and bulk platform actions share one
    /// "no concurrent writer" gate.
    pub(crate) fn bulk_platform_action_available(&self) -> bool {
        self.platform_action_available()
    }

    pub(crate) fn start_platform_action(
        &mut self,
        context: egui::Context,
        archive_path: PathBuf,
        action: PlatformAction,
    ) {
        if !self.platform_action_available() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.history.record(HistoryEntry::new(
            ActivityAction::PlatformAssignment,
            Some(archive_path.clone()),
            ActivityOutcome::Started,
            match &action {
                PlatformAction::Set(platform) => format!("Setting platform to {platform}."),
                PlatformAction::Clear => "Clearing manual platform.".to_string(),
            },
        ));
        self.platform_action = Some(RunningPlatformAction {
            archive_path: archive_path.clone(),
            receiver,
        });
        thread::spawn(move || {
            let result =
                apply_platform_action(&archive_path, &action).map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll_platform_action(&mut self, context: &egui::Context) {
        let result = self.platform_action.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.archive_path.clone(), result))
        });
        let Some((archive_path, result)) = result else {
            return;
        };
        self.platform_action = None;
        match result {
            Ok(change) => {
                let message = format!(
                    "Platform changed from {} to {}.",
                    describe_platform_assignment(
                        change.old_platform.as_deref(),
                        change.old_source.as_deref()
                    ),
                    describe_platform_assignment(
                        change.new_platform.as_deref(),
                        change.new_source.as_deref()
                    )
                );
                self.history.record(HistoryEntry::new(
                    ActivityAction::PlatformAssignment,
                    Some(archive_path),
                    ActivityOutcome::Completed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                // Refresh only the cached database row - never the live
                // snapshot (self.state), which this action never touches.
                self.start_database_action(context.clone(), false);
            }
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::PlatformAssignment,
                    Some(archive_path),
                    ActivityOutcome::Failed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
            }
        }
    }

    /// Starts a bulk platform action for every archive in
    /// `archive_paths` (the current multi-selection - see
    /// `show_bulk_platform_action_bar`) on a background thread, exactly
    /// like `start_platform_action` for a single archive. A no-op if
    /// `archive_paths` is empty or a platform-metadata write is already
    /// in progress (`bulk_platform_action_available`).
    pub(crate) fn start_bulk_platform_action(
        &mut self,
        context: egui::Context,
        archive_paths: Vec<PathBuf>,
        kind: BulkPlatformActionKind,
    ) {
        if !self.bulk_platform_action_available() || archive_paths.is_empty() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let requested_paths = archive_paths.len();
        self.history.record(HistoryEntry::new(
            ActivityAction::BulkPlatformAssignment,
            None,
            ActivityOutcome::Started,
            match &kind {
                BulkPlatformActionKind::Set(platform) => format!(
                    "Setting platform to {platform} for {requested_paths} selected archives."
                ),
                BulkPlatformActionKind::Clear => {
                    format!("Clearing manual platform for {requested_paths} selected archives.")
                }
            },
        ));
        self.bulk_platform_action = Some(RunningBulkPlatformAction {
            kind: kind.clone(),
            requested_paths,
            receiver,
        });
        thread::spawn(move || {
            let result = apply_bulk_platform_action(&archive_paths, &kind)
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Mirrors `poll_platform_action`: on success, refreshes only the
    /// cached database snapshot (never the live archive snapshot, never a
    /// scan - see `start_database_action(.., false)`). The actual
    /// selection pruning (requirement 7's "remove selections that no
    /// longer exist in the loaded catalogue") happens once that reload
    /// settles, in `poll_load`/`poll_database_load` via `prune_selection`,
    /// not here, since the reload is itself asynchronous and has not
    /// necessarily completed yet when this returns.
    pub(crate) fn poll_bulk_platform_action(&mut self, context: &egui::Context) {
        let result = self.bulk_platform_action.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.kind.clone(), running.requested_paths, result))
        });
        let Some((kind, requested_paths, result)) = result else {
            return;
        };
        self.bulk_platform_action = None;
        match result {
            Ok(outcome) => {
                let action_word = match &kind {
                    BulkPlatformActionKind::Set(platform) => format!("set to {platform}"),
                    BulkPlatformActionKind::Clear => "cleared".to_string(),
                };
                let mut message = format!(
                    "Platform {action_word} for {} of {requested_paths} selected archive(s) ({} unchanged, {} missing from the database",
                    outcome.result.changed,
                    outcome.result.unchanged,
                    outcome.result.missing.len(),
                );
                if outcome.unresolved_paths > 0 {
                    message.push_str(&format!(
                        ", {} not yet scanned into the database",
                        outcome.unresolved_paths
                    ));
                }
                message.push(')');
                self.history.record(HistoryEntry::new(
                    ActivityAction::BulkPlatformAssignment,
                    None,
                    ActivityOutcome::Completed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                // Refresh only the cached database row - never the live
                // snapshot (self.state), which this action never touches.
                self.start_database_action(context.clone(), false);
            }
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::BulkPlatformAssignment,
                    None,
                    ActivityOutcome::Failed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                // Deliberately does not touch database_state or
                // selected_archives - requirement 8: a failed bulk action
                // must preserve both the prior cached rows and the
                // selection exactly as they were.
            }
        }
    }

    /// Whether a new custom-platform-alias action may start: not already
    /// running one, and not in the middle of a database load/scan - the
    /// same "one database writer at a time" convention
    /// `platform_action_available` already enforces for individual
    /// archive platform assignment. This never touches `is_busy()`/mount
    /// safety - alias management is metadata-only and deliberately
    /// independent of it, exactly like platform assignment.
    pub(crate) fn alias_action_available(&self) -> bool {
        self.alias_action.is_none()
            && self.platform_action.is_none()
            && self.bulk_platform_action.is_none()
            && self.missing_removal.is_none()
            && self.source_action.is_none()
            && self.library_view_action.is_none()
            && !self.database_state.is_loading()
    }

    pub(crate) fn start_alias_action(&mut self, context: egui::Context, action: AliasAction) {
        if !self.alias_action_available() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.history.record(HistoryEntry::new(
            ActivityAction::PlatformAliasManagement,
            None,
            ActivityOutcome::Started,
            match &action {
                AliasAction::Add { alias, platform } => {
                    format!("Adding platform alias '{alias}' -> {platform}.")
                }
                AliasAction::Remove { alias } => format!("Removing platform alias '{alias}'."),
            },
        ));
        self.alias_action = Some(RunningAliasAction {
            action: action.clone(),
            receiver,
        });
        thread::spawn(move || {
            let result = apply_alias_action(&action).map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Mirrors `poll_platform_action`: on success, refreshes only the
    /// cached database snapshot (`platform_aliases` is now part of it;
    /// see `load_snapshot_from`), never the live archive snapshot and
    /// never a scan. On a successful add, clears the input fields so the
    /// panel is ready for the next alias; a successful remove leaves
    /// them untouched (there is nothing to clear).
    pub(crate) fn poll_alias_action(&mut self, context: &egui::Context) {
        let result = self.alias_action.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.action.clone(), result))
        });
        let Some((action, result)) = result else {
            return;
        };
        self.alias_action = None;
        match result {
            Ok(()) => {
                let message = match &action {
                    AliasAction::Add { alias, platform } => {
                        format!(
                            "Alias added: '{alias}' -> {platform}. Run a library scan to apply it."
                        )
                    }
                    AliasAction::Remove { alias } => {
                        format!(
                            "Alias removed: '{alias}'. Run a library scan to apply this change."
                        )
                    }
                };
                self.history.record(HistoryEntry::new(
                    ActivityAction::PlatformAliasManagement,
                    None,
                    ActivityOutcome::Completed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                if matches!(action, AliasAction::Add { .. }) {
                    self.new_alias_text.clear();
                    self.new_alias_platform_choice = None;
                }
                self.start_database_action(context.clone(), false);
            }
            Err(message) => {
                let action_label = match &action {
                    AliasAction::Add { alias, .. } => format!("Add alias '{alias}'"),
                    AliasAction::Remove { alias } => format!("Remove alias '{alias}'"),
                };
                self.history.record(HistoryEntry::new(
                    ActivityAction::PlatformAliasManagement,
                    None,
                    ActivityOutcome::Failed,
                    format!("{action_label}: {message}"),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
            }
        }
    }

    /// Whether a new Sources-page action may start - the same "one
    /// database writer at a time" convention `alias_action_available`
    /// already enforces, extended to also block while a source action is
    /// already running (and vice versa via the other `*_available`
    /// checks, once updated).
    pub(crate) fn source_action_available(&self) -> bool {
        self.source_action.is_none()
            && self.alias_action.is_none()
            && self.platform_action.is_none()
            && self.bulk_platform_action.is_none()
            && self.missing_removal.is_none()
            && self.library_view_action.is_none()
            && !self.database_state.is_loading()
    }

    pub(crate) fn start_source_action(&mut self, context: egui::Context, action: SourceAction) {
        if !self.source_action_available() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.history.record(HistoryEntry::new(
            source_action_log_category(&action),
            source_action_path(&action),
            ActivityOutcome::Started,
            source_action_started_message(&action),
        ));
        self.source_action = Some(RunningSourceAction {
            action: action.clone(),
            receiver,
            worker: None,
        });
        let worker = thread::spawn(move || {
            let result = run_source_action(&action).map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
        self.source_action.as_mut().unwrap().worker = Some(worker);
    }

    /// Mirrors `poll_alias_action`: on success, refreshes only the cached
    /// database snapshot (never a live scan) - `source_views` is rebuilt
    /// fresh as part of that same snapshot load, which is also exactly
    /// what keeps the Health cache correctly invalidated (see
    /// `HealthReportCacheKey`'s doc comment: it already keys on the
    /// snapshot's pointer identity, and a source action always produces a
    /// new snapshot `Box`).
    pub(crate) fn poll_source_action(&mut self, context: &egui::Context) {
        let result = self.source_action.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.action.clone(), result))
        });
        let Some((action, result)) = result else {
            return;
        };
        let worker = self
            .source_action
            .take()
            .and_then(|mut running| running.worker.take());
        if let Some(worker) = worker {
            let _ = worker.join();
        }
        let log_category = source_action_log_category(&action);
        let path = source_action_path(&action);
        let gamer_scan_pending =
            self.gamer_view_scan_pending_review || self.gamer_view_pending_first_scan.is_some();
        match result {
            Ok(outcome) => {
                let message = source_action_success_message(&outcome);
                // Adding, removing, enabling or disabling a source is an explicit
                // configuration-changing event. Refresh the sole GUI snapshot once;
                // ordinary rendering never polls the file.
                let config_reload_warning = self.gui_config.reload_default().err().map(|error| {
                    format!(
                        "The source change succeeded, but config.toml could not be reloaded: \
                         {error}. The previous in-memory configuration is still in use."
                    )
                });
                self.history.record(HistoryEntry::new(
                    log_category,
                    path,
                    ActivityOutcome::Completed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message,
                    cleanup: None,
                    warning: config_reload_warning.clone(),
                    more_information: None,
                });
                if matches!(outcome, SourceActionOutcome::Added(_)) {
                    self.sources_add_dialog = None;
                }
                // Gamer View's "Add games" chains straight into a scan of
                // the exact folder just added - see
                // `gamer_view_pending_first_scan` - so a first-run person
                // never has to find a separate "Scan" step themselves, and
                // never sees the Advanced-View-flavored "Source added...
                // Use Scan to catalogue it" message this action would
                // otherwise set below. Only fires for the path Gamer View
                // itself just added; any other Add (including a normal
                // Advanced View Sources page add) leaves the pending path
                // at `None` and chains/overrides nothing. Left set (not
                // cleared here) so the *next* `Scanned` outcome - the one
                // this chains - is also recognised as Gamer View's, and
                // gets its own human-language override below instead of
                // "Scan complete: N source(s)... N archive(s)...".
                if let SourceActionOutcome::Added(added) = &outcome
                    && let Some(scan_action) = gamer_first_scan_after_add(
                        self.gamer_view_pending_first_scan.as_deref(),
                        added,
                    )
                {
                    self.feedback = Some(ActionFeedback {
                        succeeded: true,
                        message: "Looking through your folder...".to_string(),
                        cleanup: None,
                        warning: config_reload_warning.clone(),
                        more_information: None,
                    });
                    self.start_source_action(context.clone(), scan_action);
                } else if let SourceActionOutcome::Scanned(summary) = &outcome
                    && gamer_scan_pending
                {
                    self.gamer_view_pending_first_scan = None;
                    self.gamer_view_scan_pending_review = false;
                    self.gamer_view_scan_review_available = gamer_view_scan_needs_review(summary);
                    self.feedback = Some(ActionFeedback {
                        succeeded: true,
                        message: gamer_view_first_scan_message(summary),
                        cleanup: None,
                        warning: config_reload_warning,
                        more_information: None,
                    });
                }
                if matches!(action, SourceAction::Remove { .. }) {
                    self.sources_remove_dialog = None;
                }
                // Carry this scan's skip detail into the plain snapshot
                // reload triggered below, so Database Status -> Skipped
                // files -> Inspect... becomes reachable without a separate
                // Database Status -> Scan library run. Any other source
                // action clears it, so a stale summary can never attach to
                // an unrelated later reload.
                self.pending_source_scan_summary = match &outcome {
                    SourceActionOutcome::Scanned(summary) => Some(summary.clone()),
                    _ => None,
                };
                if let SourceActionOutcome::Scanned(summary) = &outcome {
                    let scope = match &action {
                        SourceAction::ScanOne(scanned_path) => {
                            SourcesScanScope::One(scanned_path.clone())
                        }
                        _ => SourcesScanScope::AllEnabled,
                    };
                    self.sources_last_scan = Some(SourcesLastScan {
                        scope,
                        archives_found: summary.counts.archives_seen,
                        skipped_total: summary.skipped_files_total(),
                        ingestion_stats: summary.ingestion_stats,
                    });
                }
                self.start_database_action(context.clone(), false);
            }
            Err(message) => {
                let gamer_add_failed = matches!(
                    &action,
                    SourceAction::Add(candidate)
                        if self.gamer_view_pending_first_scan.as_deref()
                            == Some(candidate.as_path())
                );
                if self.gamer_view_scan_pending_review {
                    self.gamer_view_scan_pending_review = false;
                }
                if gamer_add_failed {
                    // A failed add must never leave a stale pending path that
                    // could relabel a later unrelated scan as this folder's
                    // first scan. No scan is queued from the error branch.
                    self.gamer_view_pending_first_scan = None;
                }
                self.history.record(HistoryEntry::new(
                    log_category,
                    path,
                    ActivityOutcome::Failed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message: if gamer_add_failed {
                        GAMER_ADD_GAMES_FAILURE_MESSAGE.to_string()
                    } else {
                        message.clone()
                    },
                    cleanup: None,
                    warning: None,
                    more_information: gamer_add_failed.then_some(message),
                });
            }
        }
    }


}

/// Opens the default library database and applies one `PlatformAction`
/// to the archive at `archive_path` - the production entry point run on
/// the background thread `ArchiveFsApp::start_platform_action` spawns.
/// See [`apply_platform_action_at`] (the testable core, taking an
/// explicit database path - mirrors `load_database_snapshot`/
/// `load_database_snapshot_at`) for the actual logic.
pub(crate) fn apply_platform_action(
    archive_path: &Path,
    action: &PlatformAction,
) -> archivefs_core::Result<PlatformAssignmentChange> {
    let database_path = default_database_path()?;
    apply_platform_action_at(&database_path, archive_path, action)
}

/// Resolves `archive_path` to a stable persisted archive id by exact
/// path bytes first (never a lossy display string - see
/// `Database::find_archive_id_by_absolute_path`), then applies `action`.
/// Errors clearly if the archive has no persisted catalogue row (nothing to
/// assign a platform to) rather than silently doing nothing.
pub(crate) fn apply_platform_action_at(
    database_path: &Path,
    archive_path: &Path,
    action: &PlatformAction,
) -> archivefs_core::Result<PlatformAssignmentChange> {
    let mut database = Database::open_or_create(database_path)?;
    let archive_id = database
        .find_archive_id_by_absolute_path(archive_path)?
        .ok_or_else(|| {
            ArchiveFsError::Database(format!(
                "{} is not yet in the saved library catalogue - run a library scan before assigning a platform",
                archive_path.display()
            ))
        })?;
    match action {
        PlatformAction::Set(platform) => database.set_manual_platform(archive_id, platform),
        PlatformAction::Clear => database.clear_manual_platform(archive_id),
    }
}

/// Opens the default library database and applies one
/// `BulkPlatformActionKind` to `archive_paths` - the production entry
/// point run on the background thread `ArchiveFsApp::start_bulk_platform_action`
/// spawns. See [`apply_bulk_platform_action_at`] (the testable core,
/// mirrors `apply_platform_action`/`apply_platform_action_at`) for the
/// actual logic.
pub(crate) fn apply_bulk_platform_action(
    archive_paths: &[PathBuf],
    kind: &BulkPlatformActionKind,
) -> archivefs_core::Result<BulkPlatformActionOutcome> {
    let database_path = default_database_path()?;
    apply_bulk_platform_action_at(&database_path, archive_paths, kind)
}

/// Resolves every path in `archive_paths` to a stable persisted archive
/// id by exact path bytes (never a lossy display string - see
/// `Database::find_archive_id_by_absolute_path`), then applies `kind` to
/// every id that resolved in one database transaction (see
/// `Database::set_manual_platform_for_archives`/
/// `clear_manual_platform_for_archives`). Unlike the single-row
/// `apply_platform_action_at`, a path that does not resolve to any
/// database archive id (a live-only/not-yet-scanned row, for example) is
/// not a hard error here - it is counted in the returned
/// `BulkPlatformActionOutcome::unresolved_paths` instead, so one
/// unresolvable row in a large selection never blocks every other,
/// resolvable row in the same selection from being updated. This mirrors
/// the database bulk API's own "skip and report, don't abort" policy for
/// an archive id that turns out not to exist.
pub(crate) fn apply_bulk_platform_action_at(
    database_path: &Path,
    archive_paths: &[PathBuf],
    kind: &BulkPlatformActionKind,
) -> archivefs_core::Result<BulkPlatformActionOutcome> {
    let mut database = Database::open_or_create(database_path)?;
    let mut ids = Vec::with_capacity(archive_paths.len());
    let mut unresolved_paths = 0usize;
    for path in archive_paths {
        match database.find_archive_id_by_absolute_path(path)? {
            Some(id) => ids.push(id),
            None => unresolved_paths += 1,
        }
    }
    let result = match kind {
        BulkPlatformActionKind::Set(platform) => {
            database.set_manual_platform_for_archives(&ids, platform)?
        }
        BulkPlatformActionKind::Clear => database.clear_manual_platform_for_archives(&ids)?,
    };
    Ok(BulkPlatformActionOutcome {
        result,
        unresolved_paths,
    })
}

/// Opens the default library database and applies one `AliasAction` -
/// the production entry point run on the background thread
/// `ArchiveFsApp::start_alias_action` spawns. See
/// [`apply_alias_action_at`] (the testable core, taking an explicit
/// database path - mirrors `apply_platform_action`/
/// `apply_platform_action_at`) for the actual logic. Uses
/// `Database::open_or_create` (creating the database if it does not
/// exist yet) rather than requiring a pre-existing one: unlike manual
/// platform assignment, an alias is not attached to any specific
/// already-scanned archive, so there is nothing that requires the
/// database - or a scan - to already exist first. This matches
/// `library-scan`'s existing "open or create" write-command convention
/// on the CLI side.
pub(crate) fn apply_alias_action(action: &AliasAction) -> archivefs_core::Result<()> {
    let database_path = default_database_path()?;
    apply_alias_action_at(&database_path, action)
}

pub(crate) fn apply_alias_action_at(database_path: &Path, action: &AliasAction) -> archivefs_core::Result<()> {
    let mut database = Database::open_or_create(database_path)?;
    match action {
        AliasAction::Add { alias, platform } => {
            database.add_platform_alias(alias, platform)?;
        }
        AliasAction::Remove { alias } => {
            if !database.remove_platform_alias(alias)? {
                return Err(ArchiveFsError::Database(format!(
                    "no platform alias matches '{alias}'"
                )));
            }
        }
    }
    Ok(())
}

/// Formats a platform assignment for display as `"<platform>
/// (<provenance>)"`, or `"Unknown"` when there is none - the same shape
/// as the CLI's `format_platform_and_source`, kept as a small separate
/// copy here rather than a shared crate dependency between the two
/// binaries for two lines of formatting.
pub(crate) fn describe_platform_assignment(platform: Option<&str>, source: Option<&str>) -> String {
    match (platform, source) {
        (Some(platform), Some(source)) => format!("{platform} ({source})"),
        _ => "Unknown".to_string(),
    }
}

pub(crate) fn source_action_log_category(action: &SourceAction) -> ActivityAction {
    match action {
        SourceAction::Add(_) => ActivityAction::SourceAdded,
        SourceAction::SetEnabled { enabled: true, .. } => ActivityAction::SourceEnabled,
        SourceAction::SetEnabled { enabled: false, .. } => ActivityAction::SourceDisabled,
        SourceAction::ScanOne(_) | SourceAction::ScanAll | SourceAction::AssignPlatform { .. } => {
            ActivityAction::SourceScan
        }
        SourceAction::Remove { .. } => ActivityAction::SourceRemoved,
    }
}

pub(crate) fn source_action_path(action: &SourceAction) -> Option<PathBuf> {
    match action {
        SourceAction::Add(path)
        | SourceAction::SetEnabled { path, .. }
        | SourceAction::ScanOne(path)
        | SourceAction::AssignPlatform { path, .. }
        | SourceAction::Remove { path, .. } => Some(path.clone()),
        SourceAction::ScanAll => None,
    }
}

pub(crate) fn source_action_started_message(action: &SourceAction) -> String {
    match action {
        SourceAction::Add(path) => format!("Adding source '{}'.", path.display()),
        SourceAction::SetEnabled {
            path,
            enabled: true,
        } => format!("Enabling source '{}'.", path.display()),
        SourceAction::SetEnabled {
            path,
            enabled: false,
        } => format!("Disabling source '{}'.", path.display()),
        SourceAction::ScanOne(path) => format!("Scanning source '{}'.", path.display()),
        SourceAction::ScanAll => "Scanning all enabled sources.".to_string(),
        SourceAction::AssignPlatform { path, platform } => format!(
            "Assigning {platform} to source '{}' and rescanning compatible entries.",
            path.display()
        ),
        SourceAction::Remove {
            path,
            keep_catalogue: true,
        } => format!(
            "Removing source '{}' (keeping catalogue entries).",
            path.display()
        ),
        SourceAction::Remove {
            path,
            keep_catalogue: false,
        } => format!(
            "Removing source '{}' and its catalogue entries.",
            path.display()
        ),
    }
}

pub(crate) fn source_action_success_message(outcome: &SourceActionOutcome) -> String {
    match outcome {
        SourceActionOutcome::Added(source) => format!(
            "Source added: {}. Use Scan to catalogue it.",
            source.path.display()
        ),
        SourceActionOutcome::SetEnabled(outcome) => match &outcome.scan {
            Some(scan) => format!(
                "Source enabled: {}. Scan found {} archive(s), {} missing.",
                outcome.source.path.display(),
                scan.counts.archives_seen,
                scan.counts.archives_missing
            ),
            None => format!(
                "Source disabled: {}. Catalogue entries were preserved.",
                outcome.source.path.display()
            ),
        },
        SourceActionOutcome::Scanned(summary) => {
            let succeeded = summary.counts.source_folders_scanned;
            let failed = summary.folder_errors.len();
            if failed == 0 {
                format!(
                    "Scan complete: {succeeded} source(s) scanned, {} archive(s) found, {} \
                     missing.",
                    summary.counts.archives_seen, summary.counts.archives_missing
                )
            } else {
                format!(
                    "Scan complete: {succeeded} source(s) succeeded, {failed} failed. Existing \
                     catalogue entries were preserved for the failed source(s)."
                )
            }
        }
        SourceActionOutcome::PlatformAssigned { platform, scan } => format!(
            "Source assigned {platform}. Rescan found {} item(s); compatible Unknown entries were reclassified. {} incompatible item(s) remained visible and Unknown.",
            scan.counts.archives_seen,
            scan.platform_assignment_warnings.len()
        ),
        SourceActionOutcome::Removed(outcome) => match outcome.catalogue_rows_removed {
            Some(count) => format!(
                "Source removed: {}. {count} catalogue row(s) removed.",
                outcome.removed_source.path.display()
            ),
            None => format!(
                "Source removed: {}. Catalogue entries were preserved.",
                outcome.removed_source.path.display()
            ),
        },
    }
}

/// The one continuation decision behind Gamer View's seamless Add games
/// journey. It deliberately returns the existing `ScanOne` action only for
/// the exact path whose successful Add set the pending marker; Advanced View
/// adds and unrelated source results cannot start or steal this scan.
pub(crate) fn gamer_first_scan_after_add(
    pending_path: Option<&Path>,
    added: &SourceFolderConfig,
) -> Option<SourceAction> {
    (pending_path == Some(added.path.as_path())).then(|| SourceAction::ScanOne(added.path.clone()))
}

/// Runs one [`SourceAction`] against the default config/database paths -
/// the production entry point `ArchiveFsApp::start_source_action` runs on
/// a background thread. Every arm calls straight into the same, already
/// tested `archivefs_core` function the CLI's matching `source`/`sources`
/// subcommand calls (see `crates/archivefs-cli/src/main.rs`'s `source
/// add`/`enable`/`disable`/`scan`/`sources scan-all`/`source remove`
/// handlers) - never a second implementation of validation, scanning, or
/// persistence.
pub(crate) fn run_source_action(action: &SourceAction) -> archivefs_core::Result<SourceActionOutcome> {
    match action {
        SourceAction::Add(path) => add_source_folder_default(path).map(SourceActionOutcome::Added),
        SourceAction::SetEnabled { path, enabled } => {
            set_source_folder_enabled_default(path, *enabled).map(SourceActionOutcome::SetEnabled)
        }
        SourceAction::ScanOne(path) => {
            scan_source_folder_default(path).map(SourceActionOutcome::Scanned)
        }
        SourceAction::ScanAll => {
            scan_all_enabled_sources_default().map(SourceActionOutcome::Scanned)
        }
        SourceAction::AssignPlatform { path, platform } => {
            assign_source_platform_default(path, platform).map(|scan| {
                SourceActionOutcome::PlatformAssigned {
                    platform: platform.clone(),
                    scan,
                }
            })
        }
        SourceAction::Remove {
            path,
            keep_catalogue,
        } => remove_source_folder_default(path, *keep_catalogue).map(SourceActionOutcome::Removed),
    }
}
