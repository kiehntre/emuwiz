//! The application type.
//!
//! `ArchiveFsApp` is the composition root: it owns the shell's own state and
//! holds every feature's state bundle, and its `eframe::App` implementation
//! runs one frame in the order `update` lays out. The feature modules own
//! their workers, their pages and the methods that drive them - many of them
//! written as further `impl ArchiveFsApp` blocks in their own files - so this
//! module is the type, its construction, its shutdown and its frame loop,
//! not a second home for feature logic.
//!
//! `main.rs` keeps the module declarations, the shared policy types and free
//! helpers that have not yet reached a feature owner, the CLI entry points
//! and the native launch.

use crate::*;

pub(crate) struct ArchiveFsApp {
    pub(crate) state: LoadState,
    pub(crate) library_ui: LibraryUiState,
    /// The sole owner of primary archive identity across Library, Selected,
    /// Mount, and Cheats & Mods. Mount state remains derived from live
    /// records and is intentionally not stored here.
    pub(crate) archive_context: ArchiveContext,
    pub(crate) mount_ui: MountUiState,
    /// The History & Logs page's filter/sort state.
    pub(crate) history_filters: HistoryLogFilters,
    pub(crate) shared_history: SharedHistoryState,
    pub(crate) shared_history_operation: Option<String>,
    pub(crate) shared_rollback: SharedRollbackState,
    pub(crate) doctor_repair: DoctorRepairState,
    pub(crate) emulator_readiness: EmulatorReadinessState,
    /// The Tape Inspector library browser's persisted search/platform/format
    /// filter state, retained across page re-renders.
    pub(crate) tape_inspector_filter: tape_analysis_page::LibraryTapeFilterState,
    /// The full-page Cheats & Mods workspace's current archive and
    /// trusted-catalogue state. It survives ordinary page navigation so
    /// returning to the same exact archive does not discard a completed
    /// cache inspection or retrieval.
    pub(crate) cheat_workflow: Option<CheatWorkflowState>,
    /// Independent read-only review state for user-supplied .cht/.pnach files.
    pub(crate) user_cheat_import_page: user_cheat_import_page::UserCheatImportPageState,
    /// The Dolphin texture-mod panel's own state - deliberately separate
    /// from `cheat_workflow` (a texture mod is not a cheat) and keyed by
    /// `{ archive_path, profile_id, verified_game_id }` internally, so it
    /// never reuses state describing a different game or profile. See
    /// `dolphin_texture_mod_page`'s own module doc comment.
    pub(crate) dolphin_texture_mod: dolphin_texture_mod_page::DolphinTextureModPageState,
    pub(crate) ppsspp_texture_mod: ppsspp_texture_mod_page::PpssppTextureModPageState,
    pub(crate) local_mod_package: local_mod_package_page::LocalModPackagePageState,
    /// The Launch Readiness panel's "Launch RetroArch" tracker - see
    /// `launch_readiness_page`'s own module doc comment. Deliberately not
    /// reset on archive/page navigation: a running or just-exited process
    /// must still be reaped/shown correctly even after the user selects a
    /// different game.
    pub(crate) launch_retroarch: launch_readiness_page::RetroArchLaunchState,
    /// The Launch Readiness panel's "Launch Dolphin" tracker - the same
    /// reasoning as `launch_retroarch` above applies unchanged.
    pub(crate) launch_dolphin: launch_readiness_page::DolphinLaunchState,
    /// The Launch Readiness panel's "Launch PCSX2" tracker - the same
    /// reasoning as `launch_retroarch` above applies unchanged.
    pub(crate) launch_pcsx2: launch_readiness_page::Pcsx2LaunchState,
    pub(crate) launch_standalone: launch_readiness_page::StandaloneLaunchState,
    pub(crate) launch_amiga_whdload: launch_readiness_page::AmigaWHDLoadLaunchState,
    /// A tentative archive choice is isolated here until the picker is
    /// applied. It never mutates Library focus or multi-selection.
    pub(crate) cheat_archive_picker: Option<CheatArchivePickerState>,
    /// A different archive requires confirmation when fetched catalogue
    /// state would otherwise be discarded.
    pub(crate) confirm_cheat_archive_change: Option<PathBuf>,
    pub(crate) feedback: Option<ActionFeedback>,
    pub(crate) history: OperationHistory,

    /// Set once this session has seen a diagnostics report where the
    /// config file was confirmed present and readable. Lets the Setup
    /// screen tell a genuine first run apart from a config that
    /// disappeared after previously being found - the latter may mean
    /// something was deleted, unmounted, or is otherwise a real problem,
    /// and must never be presented with the same reassuring "you have not
    /// configured this yet" framing as a fresh install.

    /// First-run onboarding: loaded once at startup from
    /// `onboarding_state.txt` (see `onboarding.rs`), advanced only through
    /// `onboarding::*` helper methods, and persisted back on every
    /// transition. Never duplicates source/DAT/emulator state - it only
    /// tracks which step of the guided tour the user is on.

    /// One-shot: whether the auto-open check (first genuine run only, see
    /// `maybe_auto_open_onboarding`) has already run this session.

    /// Doctor Stage 1A: the current read-only diagnostic scan. Entirely
    /// separate from `self.state`/`self.refresh`, so running Doctor never
    /// reloads the application.

    /// The Cheat Sources page, loaded lazily the first time it is opened so
    /// that starting the GUI never reads the preferences file for a page the
    /// user has not visited.
    /// Read-only review of core-produced duplicate/conflict reports.
    pub(crate) cheat_reconciliation_review:
        cheat_reconciliation_review::CheatReconciliationReviewState,
    /// The browse-only CheatBase panel embedded in Cheats & Mods. Its setup,
    /// search, and inspection work is explicit and independent of emulator
    /// cheat installation workflows.
    pub(crate) cheatbase_page: cheatbase_page::CheatBasePageState,
    /// The approval-bound managed-emulator download section shown inside
    /// Emulator Setup. Downloads only run after an explicit confirmation
    /// click; installing an emulator never implies it is launch-ready.
    pub(crate) emulator_download_page: emulator_download_page::EmulatorDownloadPageState,
    pub(crate) rom_organisation_page: Option<rom_organisation_page::RomOrganisationPageState>,
    pub(crate) publisher_profile_page: Option<publisher_profile_page::PublisherProfilePageState>,
    /// The Repair Review page, loaded lazily on first visit. Preview-only;
    /// it never applies anything.
    pub(crate) repair_review_page: Option<repair_review_page::RepairReviewPageState>,
    /// The Repair History page, loaded lazily on first visit: recent rename
    /// transactions journaled through the Repair Center, re-read from disk
    /// on every refresh.
    pub(crate) repair_history_page: Option<repair_history_page::RepairHistoryPageState>,
    /// The Exact Duplicate Review page, loaded lazily on first visit:
    /// starts with no source folder chosen and no scan run, exactly like
    /// `RepairReviewPageState::default()` starts with no plan loaded.
    pub(crate) exact_duplicate_review_page:
        Option<exact_duplicate_review_page::ExactDuplicateReviewPageState>,
    pub(crate) optical_conversion_page: Option<optical_conversion_page::OpticalConversionPageState>,
    pub(crate) storage_health_page: storage_health_page::StorageHealthPageState,
    /// The Library View History page, loaded lazily on first visit:
    /// durable Library View apply/remove records, re-read from disk on
    /// every refresh. Distinct from `history` (`OperationHistory`) below,
    /// which is in-memory only.
    pub(crate) library_view_history_page:
        Option<library_view_history_page::LibraryViewHistoryPageState>,
    /// Unsubmitted Cheat Sources text and disclosure state. Held here rather
    /// than in the page state because none of it is policy - see
    /// `CheatSourcesPageUi`.
    /// The DAT Sources page, loaded lazily on first visit for the same reason
    /// Cheat Sources is: starting the GUI should not read a registry file for
    /// a page nobody has opened.
    /// Unsubmitted DAT Sources text and disclosure state. Held here rather
    /// than in the page state because none of it is policy.
    /// The finding whose evidence panel is open, by stable finding id.

    /// The repair awaiting confirmation, if any.

    /// The most recent repair result, kept on screen next to the finding it
    /// was for.

    /// When the last repair finished, alongside (never replacing) the scan's
    /// own timestamp.
    pub(crate) setup_action: Option<RunningSetupAction>,
    pub(crate) refresh_error: Option<String>,
    pub(crate) snapshot_stale: bool,
    pub(crate) refresh_generation: RefreshGeneration,
    pub(crate) snapshot_generation: Option<RefreshGeneration>,
    pub(crate) database_state: DatabaseState,
    pub(crate) database_generation: DatabaseGeneration,
    pub(crate) needs_attention: needs_attention::AttentionWorkspace,
    /// A `ScanPersistSummary` from a just-completed Sources-page scan
    /// (`SourceActionOutcome::Scanned`), waiting to be carried into the
    /// `DatabaseState::Ready.last_scan_summary` produced by the plain
    /// snapshot reload that `poll_source_action` always triggers afterward
    /// (`DatabaseOutcome::Loaded`, which otherwise has no scan summary of
    /// its own). Consumed (taken) by the very next `poll_database_load`
    /// completion regardless of its outcome, so a summary can never attach
    /// to an unrelated, later reload.
    /// The Sources page's persistent echo of its most recent scan result
    /// (see [`SourcesLastScan`]) - unlike `pending_source_scan_summary`
    /// above, this is never consumed/cleared by a reload; it stays visible
    /// on the Sources page until superseded by a newer Sources-page scan.
    pub(crate) health_duplicate_ui: HealthDuplicateUiState,
    /// The real OS clipboard backing every text field's context menu -
    /// see `NativeClipboard`'s doc comment for why this is kept for the
    /// app's whole lifetime rather than opened per click.
    pub(crate) clipboard: NativeClipboard,
    /// Which of the four primary destinations is currently showing - see
    /// `MainView`'s doc comment. Never reset except by an explicit
    /// navigation click; every page's own state (filters/sort/selection)
    /// lives in its own fields below, independent of this one.
    pub(crate) view: MainView,
    /// The last Library-area tab the user was on - see `LibraryTab`'s doc
    /// comment for the synchronization rule with `view`. Drives which tab
    /// the unified Library shell shows.
    pub(crate) library_tab: LibraryTab,
    /// The last "Problems & Repair" tab the user was on - see
    /// `ProblemsRepairTab`'s doc comment for the synchronization rule with
    /// `view`, identical to `library_tab`'s.
    pub(crate) problems_repair_tab: ProblemsRepairTab,
    /// The last "Sources" tab the user was on - see `SourcesTab`'s doc
    /// comment for the synchronization rule with `view`, identical to
    /// `library_tab`'s.
    pub(crate) sources_tab: SourcesTab,
    /// Which "Tools" screen (if any) is showing in front of `view` - see
    /// `ToolsOverlay`'s doc comment.
    pub(crate) tools_overlay: ToolsOverlay,
    pub(crate) show_activity: bool,
    /// Whether the Help "About EmuWiz" window is open.
    pub(crate) show_about: bool,
    /// Whether the "Skipped files" drill-down window (opened from the
    /// Database Status overlay's "Skipped N" detail) is open. Read-only:
    /// opening it never re-scans, re-classifies, or mutates anything - it
    /// only displays `ScanPersistSummary::skipped_files` from the most
    /// recently completed scan this session already produced.
    pub(crate) show_skipped_files: bool,
    /// The active reason filter for the skipped-files window. `None` is
    /// "All reasons".
    pub(crate) skipped_files_filter: Option<archivefs_core::SkipReason>,
    /// A one-shot signal from the Library menu's "Select all visible" item.
    /// `show_loaded_data` consumes and clears it the same way it already
    /// consumes a Ctrl+A keypress or the inline button, calling the exact
    /// same `select_all_visible` helper (see its own call site). Needed
    /// because the menu bar renders before `show_loaded_data` computes
    /// this frame's `visible_indices`, so the request cannot be applied
    /// directly from the menu's own click handler.
    pub(crate) select_all_visible_requested: bool,
    /// The Sources page's currently running background action, if any -
    /// mirrors `alias_action` exactly, including the "one writer at a
    /// time" convention `source_action_available` enforces.
    /// A folder picked for the temporary preparation root but not yet
    /// applied. Picking or cancelling never writes config.toml.
    /// The visible outcome of the most recent "Apply folder" for the
    /// temporary preparation root, rendered in the Sources -> Libraries
    /// mount-root card. Set from the background `SetupAction::SetMountRoot`
    /// result; cleared when a new apply starts.
    pub(crate) catalogue_bsfree_ui: CatalogueBsFreeUiState,
    /// Loaded once for GUI use. RomM rendering and cached browsing borrow this
    /// snapshot instead of reading `config.toml` on every frame.
    pub(crate) gui_config: GuiConfigSnapshot,
    /// Session-only ScreenScraper metadata-provider settings and connection
    /// status. Credentials are deliberately never loaded from disk.
    pub(crate) screenscraper_page: screenscraper_page::ScreenScraperPageState,
    pub(crate) screenscraper_enrichment: screenscraper_enrichment_page::ScreenScraperEnrichmentState,
    /// The last authoritative RomM snapshot. `None` until the first status load,
    /// so the card shows "reading" rather than a screenful of zeroes.
    pub(crate) romm_ui: RommUiState,
    pub(crate) selected_evidence_ui: SelectedEvidenceUiState,
    /// The "Add Folder" dialog's open/closed state and its own fields -
    /// see `SourcesAddDialogState`.
    /// Set when Gamer View's first-run "Add games" action dispatches a
    /// `SourceAction::Add` for this exact path - so the resulting
    /// `SourceActionOutcome::Added` knows to immediately chain a
    /// `SourceAction::ScanOne` for the same folder (one seamless "pick a
    /// folder, see your games" flow, reusing the existing Sources
    /// add/scan machinery unchanged rather than duplicating it) instead of
    /// leaving a newly-added, never-scanned source silently empty. Cleared
    /// once the chained scan is started, so a normal Advanced View Sources
    /// page "Add" never chains an unwanted scan.
    /// Set when a scan requested from Gamer View finishes with existing
    /// skipped/ambiguous/failed detail that the user can review in Sources ->
    /// Discovery. This is presentation state only; the scan itself is still
    /// the shared SourceAction::ScanAll/ScanOne path.
    pub(crate) gamer_view_scan_review_available: bool,
    /// Distinguishes a Gamer View scan from a scan started elsewhere while
    /// the shared source worker is running, so its completion can use the
    /// beginner-facing summary without changing scan semantics.
    pub(crate) gamer_view_scan_pending_review: bool,
    /// The Remove-source confirmation dialog's open/closed state - see
    /// `SourcesRemoveDialogState`.
    pub(crate) sources_ui: SourcesUiState,
    /// Every configured Library View - loaded at startup and refreshed
    /// after every add/edit/enable/disable/remove action completes (see
    /// `reload_library_views`). Independent of `database_state`'s cached
    /// catalogue snapshot: views are a small flat config file, not a
    /// derived database read, so they are never stale behind a scan.
    pub(crate) library_views: Vec<LibraryViewConfig>,
    /// The Library Views page's currently running background action, if
    /// any - mirrors `source_action` exactly, including the "one writer at
    /// a time" convention.
    pub(crate) library_view_action: Option<RunningLibraryViewAction>,
    /// The most recently computed Preview for a view, if any - both what
    /// the Library Views page shows in its plan table and what "Apply"/
    /// "Repair" act on for that view, and what the Library page's "Show in
    /// Library View preview" hook (see `RowContextMenuAction`) reads to
    /// decide whether "Copy planned view path" is available for a given
    /// archive. Cleared whenever a different view is previewed, or after
    /// Apply/Repair/Remove changes the view it belongs to (its plan may no
    /// longer be accurate).
    pub(crate) library_view_last_plan: Option<(LibraryViewConfig, LibraryViewPlan)>,
    /// The Add View / Edit View dialog's open/closed state and its own
    /// fields - see `LibraryViewFormDialogState`. `editing_id` distinguishes
    /// the two (`None` = Add, `Some(id)` = Edit), following the same
    /// "one `Option` field is the dialog's open/closed flag" convention as
    /// every other dialog in this app.
    pub(crate) library_view_form_dialog: Option<LibraryViewFormDialogState>,
    /// The Remove-view confirmation dialog's open/closed state - see
    /// `LibraryViewRemoveDialogState`.
    pub(crate) library_view_remove_dialog: Option<LibraryViewRemoveDialogState>,
    /// The Library page's "Show in Library View preview" hook - the exact
    /// archive path the Library Views page shows a read-only status banner
    /// for once navigated there (see `library_view_planned_entry_for`).
    /// Unlike a one-shot flag, this deliberately persists across frames -
    /// clearing it the instant it is shown would make the banner disappear
    /// before the user could read it, since every frame re-renders. It is
    /// only ever replaced by a newer "Show in Library View preview" click.
    pub(crate) library_view_focus_archive: Option<PathBuf>,
    /// The Library Views page's Preview details filter - see
    /// `LibraryViewPlanFilter`.
    pub(crate) library_view_plan_filter: LibraryViewPlanFilter,
    /// The Archive Inspector overlay's state for whichever archive it was
    /// last opened for, if any - `None` means it has never been opened
    /// this session. Independent of `tools_overlay`: closing the overlay
    /// (setting `tools_overlay` back to `None`) deliberately leaves this
    /// as-is, so reopening it shows the same archive's already-loaded
    /// report instead of re-inspecting from scratch.
    pub(crate) archive_inspector: Option<ArchiveInspectorState>,
    pub(crate) archive_inspector_generation: RefreshGeneration,
    pub(crate) archive_preparation: ArchivePreparationState,
    pub(crate) archive_preparation_generation: RefreshGeneration,
    /// docs/GUI_NAVIGATION_RESET_DESIGN.md's mode switch - a view-layer
    /// concept only, persisted independently of every other field (see
    /// `load_gui_mode`/`save_gui_mode`).
    pub(crate) ui_mode: GuiMode,
    /// Which of Gamer View's two screens is showing - never persisted.
    pub(crate) gamer_view_screen: GamerViewScreen,
    /// The typed count for Mount All's >25-item confirmation gate - see
    /// `bulk_action_confirm_enabled`. Cleared whenever the dialog closes.
    pub(crate) missing_removal_typed_count: String,
    /// Row-context-menu "Mount selected" (audit finding: this previously
    /// dispatched with no confirmation at all, unlike "Unmount selected").
    /// The exact paths are re-derived fresh from the live snapshot at
    /// confirm time, never trusted from when the dialog opened.
    /// Bulk platform assignment/clear (audit finding: this previously
    /// dispatched instantly with no confirmation at all, from both the
    /// selection action bar and the row context menu).
    pub(crate) confirm_bulk_platform_action: Option<(Vec<PathBuf>, BulkPlatformActionKind)>,
    pub(crate) focus_bulk_platform_cancel: bool,
    pub(crate) bulk_platform_action_typed_count: String,
    /// RomM cover artwork for the Gamer View game list: what has been asked
    /// for, what has been answered, and which library generation those
    /// answers belong to. Holds no thread of its own - see `gamer_cover_worker`.
    /// Selected-Details RomM screenshots, sharing the cover worker and
    /// ArtworkCache security path while remaining separate from cover slots.
    /// The thread that resolves those covers, started on the first frame that
    /// actually draws the list so a session that never opens Gamer View never
    /// Museum's own navigation state (grid vs. one platform's detail view) -
    /// see `museum_page`'s own module doc.
    /// opens the catalogue. `None` until then.
    /// Whether a cover worker may be started at all. Always true in the running
    /// application.
    ///
    /// Tests set it false. Starting the worker opens the real per-user identity
    /// cache under `$HOME` and, for a developer who has RomM configured, can
    /// reach their instance - neither of which a `cargo test` run may do. It
    /// also made cover tests racy: the worker answered the very rows the test
    /// was driving by hand, so a reply could overwrite the slot under test
    /// between one frame and the next.
    /// The `config_identity` the cover cache's answers were resolved against.
    /// A change means the same path may now be a different archive, so every
    /// answer is discarded - see `GamerCoverCache::library_changed`.
    /// Enrichment (synopsis/genre/players/rating/release year) for the
    /// currently selected/featured Gamer View game, if any was found. Holds
    /// at most one game's worth of data - see
    /// `crate::game_metadata::GameMetadataWorker`.
    /// The thread that resolves enrichment lookups, started lazily like
    /// `gamer_cover_worker`. `None` until Gamer View first needs it.
    /// Mirrors `gamer_cover_worker_allowed`: tests set this false so a
    /// `cargo test` run never opens the real per-user identity cache.
    /// The Gamer View browsing rail's A-Z jump strip index - see
    /// [`crate::gamer_view::AlphaJumpIndex`]. Persisted here (like
    /// `gamer_covers`) because it caches a sort/bucket rebuild across
    /// frames, rebuilding only when the visible result set changes.
    pub(crate) artwork_media: ArtworkMediaState,
}

impl ArchiveFsApp {
    pub(crate) fn is_busy(&self) -> bool {
        self.mount_ui.operation.is_some()
            || self.mount_ui.mount_all.is_some()
            || self.mount_ui.unmount_all.is_some()
            || self.setup_action.is_some()
    }

    pub(crate) fn new(context: egui::Context) -> Self {
        theme::apply(&context);
        let gui_config = GuiConfigSnapshot::load_default();
        let generation = RefreshGeneration::INITIAL;
        let database_generation = DatabaseGeneration::INITIAL;
        let mut history = OperationHistory::default();
        history.record(HistoryEntry::new(
            ActivityAction::Refresh,
            None,
            ActivityOutcome::Started,
            "Loading your library.",
        ));
        Self {
            state: start_load(context.clone(), generation, None),
            database_state: start_database_load(context.clone(), database_generation, None, false),
            database_generation,
            needs_attention: needs_attention::AttentionWorkspace::default(),
            cheat_reconciliation_review:
                cheat_reconciliation_review::CheatReconciliationReviewState::default(),
            cheatbase_page: cheatbase_page::CheatBasePageState::default(),
            emulator_download_page: emulator_download_page::EmulatorDownloadPageState::default(),
            rom_organisation_page: None,
            publisher_profile_page: None,
            repair_review_page: None,
            repair_history_page: None,
            exact_duplicate_review_page: None,
            optical_conversion_page: None,
            library_view_history_page: None,
            sources_ui: SourcesUiState::default(),
            archive_context: ArchiveContext::default(),
            library_ui: LibraryUiState::default(),
            mount_ui: MountUiState::default(),
            history_filters: HistoryLogFilters::default(),
            shared_history: SharedHistoryState::NotLoaded,
            shared_history_operation: None,
            shared_rollback: SharedRollbackState::Idle,
            doctor_repair: DoctorRepairState::new(context.clone(), generation),
            emulator_readiness: EmulatorReadinessState::new(),
            storage_health_page: storage_health_page::StorageHealthPageState::default(),
            tape_inspector_filter: tape_analysis_page::LibraryTapeFilterState::default(),
            cheat_workflow: None,
            user_cheat_import_page: user_cheat_import_page::UserCheatImportPageState::default(),
            dolphin_texture_mod: dolphin_texture_mod_page::DolphinTextureModPageState::default(),
            ppsspp_texture_mod: ppsspp_texture_mod_page::PpssppTextureModPageState::default(),
            local_mod_package: local_mod_package_page::LocalModPackagePageState::default(),
            launch_retroarch: launch_readiness_page::RetroArchLaunchState::default(),
            launch_dolphin: launch_readiness_page::DolphinLaunchState::default(),
            launch_pcsx2: launch_readiness_page::Pcsx2LaunchState::default(),
            launch_standalone: launch_readiness_page::StandaloneLaunchState::default(),
            launch_amiga_whdload: launch_readiness_page::AmigaWHDLoadLaunchState::default(),
            cheat_archive_picker: None,
            confirm_cheat_archive_change: None,
            feedback: None,
            history,
            setup_action: None,
            refresh_error: None,
            snapshot_stale: false,
            refresh_generation: generation,
            snapshot_generation: None,
            health_duplicate_ui: HealthDuplicateUiState::default(),
            clipboard: NativeClipboard::new(),
            view: MainView::default(),
            library_tab: LibraryTab::default(),
            problems_repair_tab: ProblemsRepairTab::default(),
            sources_tab: SourcesTab::default(),
            tools_overlay: ToolsOverlay::default(),
            show_activity: ACTIVITY_EXPANDED_BY_DEFAULT,
            show_about: false,
            show_skipped_files: false,
            skipped_files_filter: None,
            select_all_visible_requested: false,
            catalogue_bsfree_ui: CatalogueBsFreeUiState::default(),
            gui_config,
            screenscraper_page: screenscraper_page::ScreenScraperPageState::default(),
            screenscraper_enrichment: screenscraper_enrichment_page::ScreenScraperEnrichmentState::default(),
            romm_ui: RommUiState::default(),
            selected_evidence_ui: SelectedEvidenceUiState::default(),
            gamer_view_scan_review_available: false,
            gamer_view_scan_pending_review: false,
            library_views: load_library_view_configs_default().unwrap_or_default(),
            library_view_action: None,
            library_view_last_plan: None,
            library_view_form_dialog: None,
            library_view_remove_dialog: None,
            library_view_focus_archive: None,
            library_view_plan_filter: LibraryViewPlanFilter::default(),
            archive_inspector: None,
            archive_inspector_generation: RefreshGeneration::INITIAL,
            archive_preparation: ArchivePreparationState::default(),
            archive_preparation_generation: RefreshGeneration::INITIAL,
            ui_mode: load_gui_mode(),
            gamer_view_screen: GamerViewScreen::default(),
            missing_removal_typed_count: String::new(),
            confirm_bulk_platform_action: None,
            focus_bulk_platform_cancel: false,
            bulk_platform_action_typed_count: String::new(),
            artwork_media: ArtworkMediaState::new(),
        }
    }

    /// The compatibility wrapper for navigating *by tab* - the write
    /// direction LibraryTab's synchronization rule reserves exclusively
    /// for this method (every other call site still navigates by setting
    /// `view` directly, exactly as before). Sets both fields together so
    /// they can never briefly disagree; `tools_overlay` is cleared to
    /// match every other navigation call site's behaviour (see the
    /// sidebar-click handler in `update`). Called by the unified Library
    /// shell's tab_row.
    pub(crate) fn navigate_to_library_tab(&mut self, tab: LibraryTab) {
        self.view = main_view_for_library_tab(tab);
        self.library_tab = tab;
        self.tools_overlay = ToolsOverlay::None;
    }

    pub(crate) fn navigate_to_missing_catalogue_review(&mut self) {
        self.navigate_to_library_tab(LibraryTab::Archives);
        administration_pages::set_missing_review_mode(&mut self.library_ui.library_filters, true);
        self.archive_context.clear_selection();
    }

    /// `ProblemsRepairTab`'s exact counterpart to `navigate_to_library_tab` -
    /// same synchronization rule, same reason for existing (called by the
    /// consolidated page's own tab row).
    pub(crate) fn navigate_to_problems_repair_tab(&mut self, tab: ProblemsRepairTab) {
        self.view = main_view_for_problems_repair_tab(tab);
        self.problems_repair_tab = tab;
        self.tools_overlay = ToolsOverlay::None;
    }

    /// `SourcesTab`'s exact counterpart to `navigate_to_library_tab`.
    pub(crate) fn navigate_to_sources_tab(&mut self, tab: SourcesTab) {
        self.view = main_view_for_sources_tab(tab);
        self.sources_tab = tab;
        self.tools_overlay = ToolsOverlay::None;
    }

    pub(crate) fn navigate_to_home_card(&mut self, card: home_page::HomeCard) {
        match card {
            home_page::HomeCard::RomM => {
                // The RomM provider integration is the RomM source card on
                // Sources -> Libraries. Route there so the card's
                // `romm_snapshot` readiness badge and its destination
                // describe the same subsystem. (The whole-collection Playing
                // Library planner remains reachable honestly via Library
                // Organisation -> "Build Playing Library".) Sidebar and
                // top-menu "RomM" converge on this exact call.
                self.navigate_to_sources_tab(SourcesTab::Libraries);
            }
            home_page::HomeCard::ConvertDiscs => {
                // First-class Disc Conversion destination. The dispatch
                // branch lazily creates `optical_conversion_page`, so no
                // pre-seeding is needed; done here too so the state exists
                // the instant the card is clicked.
                self.navigate_to_main_view(MainView::DiscConversion);
                self.optical_conversion_page.get_or_insert_with(
                    optical_conversion_page::OpticalConversionPageState::default,
                );
            }
            home_page::HomeCard::DuplicateReview => {
                // First-class Duplicate Finder destination - renders
                // standalone, without Repair Review / Repair History.
                self.navigate_to_main_view(MainView::ExactDuplicateReview);
            }
            _ => self.navigate_to_main_view(main_view_for_home_card(card)),
        }
    }

    /// The one sanctioned way to change `self.view` in response to a user
    /// action (a sidebar click, a Home card, a menu item) - shared so
    /// every navigation source applies the same special cases
    /// (`CheatsMods` clears any open tools overlay; `Library` restores
    /// whichever tab was last selected rather than resetting to Archives)
    /// instead of each call site re-deriving them, and so the *last*
    /// navigation call in a frame always wins over anything set earlier
    /// that frame.
    pub(crate) fn navigate_to_main_view(&mut self, target: MainView) {
        if self.ui_mode == GuiMode::Simple {
            match target {
                MainView::Sources => self.sources_tab = SourcesTab::Libraries,
                MainView::Library => self.library_tab = LibraryTab::Archives,
                MainView::Problems => self.problems_repair_tab = ProblemsRepairTab::Overview,
                _ => {}
            }
        }
        if target == MainView::CheatsMods {
            self.view = MainView::CheatsMods;
            self.tools_overlay = ToolsOverlay::None;
        } else if target == MainView::Library {
            self.navigate_to_library_tab(self.library_tab);
        } else if target == MainView::Problems {
            // Restores whichever "Problems & Repair" tab was last selected,
            // exactly like `Library` above - clicking the one sidebar entry
            // a second time should not reset Diagnostics/Repair progress
            // back to Overview.
            self.navigate_to_problems_repair_tab(self.problems_repair_tab);
        } else if target == MainView::Sources {
            // Restores whichever Sources tab was last selected, exactly
            // like `Library`/`Problems` above.
            self.navigate_to_sources_tab(self.sources_tab);
        } else {
            self.view = target;
            self.tools_overlay = ToolsOverlay::None;
        }
    }

    /// Gamer View's gear menu has no sidebar behind it, so "Advanced View"
    /// is the only realistic path a genuine first-run user has to reach
    /// the task list. It must always land on Home, never on whatever
    /// `self.view` happened to hold from an earlier Advanced View visit.
    ///
    /// Deliberately does not persist `ui_mode` itself - the caller does
    /// that (see the gear menu's "Advanced View" handler) - so this method
    /// stays a pure state transition, callable from a test without writing
    /// to the real per-user GUI-mode file.
    pub(crate) fn switch_to_advanced_view_at_home(&mut self) {
        self.ui_mode = GuiMode::AdvancedView;
        self.view = MainView::Home;
        self.tools_overlay = ToolsOverlay::None;
    }

    /// LibraryTab's synchronization rule, applied once. Called at the top
    /// of every frame in `update`, before anything else runs - see
    /// LibraryTab's doc comment for the rule itself. Broken out as its own
    /// method (rather than inlined in `update`) purely so it can be
    /// exercised directly by tests without going through a full
    /// `eframe::App::update` call, which nothing else in this codebase
    /// does either.
    pub(crate) fn reconcile_library_tab(&mut self) {
        if let Some(tab) = library_tab_for_main_view(self.view) {
            self.library_tab = tab;
        }
    }

    /// `ProblemsRepairTab`'s exact counterpart to `reconcile_library_tab`,
    /// called alongside it every frame.
    pub(crate) fn reconcile_problems_repair_tab(&mut self) {
        if let Some(tab) = problems_repair_tab_for_main_view(self.view) {
            self.problems_repair_tab = tab;
        }
    }

    /// `SourcesTab`'s exact counterpart to `reconcile_library_tab`, called
    /// alongside it every frame.
    pub(crate) fn reconcile_sources_tab(&mut self) {
        if let Some(tab) = sources_tab_for_main_view(self.view) {
            self.sources_tab = tab;
        }
    }

    pub(crate) fn refresh(&mut self, context: &egui::Context) {
        self.refresh_generation = self.refresh_generation.next();
        let generation = self.refresh_generation;
        self.history.record(HistoryEntry::new(
            ActivityAction::Refresh,
            None,
            ActivityOutcome::Started,
            "Refreshing your library.",
        ));
        let previous = match std::mem::replace(
            &mut self.state,
            LoadState::Error("refresh starting".to_string()),
        ) {
            LoadState::Ready(data) => Some(data),
            LoadState::Loading { previous, .. } => previous,
            LoadState::Error(_) => None,
        };
        self.refresh_diagnostics(context);
        self.state = start_load(context.clone(), generation, previous);
    }
    pub(crate) fn poll_load(&mut self, _context: &egui::Context) {
        let Some(result) = poll_load(
            &mut self.state,
            self.refresh_generation,
            self.database_state.snapshot(),
        ) else {
            return;
        };
        match result {
            LiveLibraryPoll::Completed { merged_rows } => {
                self.library_ui.filtered_rows =
                    matching_row_indices(&merged_rows, &self.library_ui.filter);
                self.prune_selection(&merged_rows);
                self.refresh_error = None;
                self.snapshot_stale = false;
                self.snapshot_generation = Some(self.refresh_generation);
                self.history.record(HistoryEntry::new(
                    ActivityAction::Refresh,
                    None,
                    ActivityOutcome::Completed,
                    "Your library was refreshed.",
                ));
            }
            LiveLibraryPoll::Failed {
                error,
                has_previous,
            } => {
                self.snapshot_stale = has_previous;
                self.refresh_error = Some(error.clone());
                self.history.record(HistoryEntry::new(
                    ActivityAction::Refresh,
                    None,
                    ActivityOutcome::Failed,
                    error,
                ));
                self.tools_overlay = ToolsOverlay::Diagnostics;
            }
        }
    }

    /// Starts a background database load. `run_scan_first =
    /// true` is "Scan library" (runs `scan_and_persist` before reloading);
    /// `false` is "Refresh database status" / "Retry database load" (a
    /// read-only reload). Never blocks the UI thread - mirrors
    /// `refresh`/`start_load` exactly.
    pub(crate) fn start_database_action(&mut self, context: egui::Context, run_scan_first: bool) {
        if self.library_ui.missing_removal.is_some() || self.database_state.is_loading() {
            return;
        }
        self.database_generation = self.database_generation.next();
        let generation = self.database_generation;
        let previous = match std::mem::replace(
            &mut self.database_state,
            DatabaseState::Error {
                message: "database action starting".to_string(),
                previous: None,
            },
        ) {
            DatabaseState::Ready { snapshot, .. } => Some(snapshot),
            DatabaseState::Outdated { previous, .. } | DatabaseState::Error { previous, .. } => {
                previous
            }
            DatabaseState::Loading { .. } => unreachable!("loading state was rejected above"),
            DatabaseState::NotCreated { .. } => None,
        };
        if run_scan_first {
            self.history.record(HistoryEntry::new(
                ActivityAction::LibraryDatabase,
                None,
                ActivityOutcome::Started,
                "Scanning configured source folders into the library database.",
            ));
        }
        self.database_state = start_database_load(context, generation, previous, run_scan_first);
    }

    pub(crate) fn apply_screenscraper_enrichment(
        &mut self,
        context: egui::Context,
        archive_id: i64,
        values: archivefs_core::screenscraper_enrichment::AcceptedScreenScraperMetadata,
        receipt: archivefs_core::screenscraper_enrichment::ScreenScraperEnrichmentReceipt,
    ) {
        let result = archivefs_core::default_database_path()
            .and_then(archivefs_core::Database::open_or_create)
            .and_then(|mut database| {
                database.apply_screenscraper_enrichment(archive_id, &values, &receipt)
            });
        match result {
            Ok(()) => {
                self.screenscraper_enrichment.mark_applied_for(archive_id);
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message: "Metadata was applied explicitly. Identity and source files were unchanged.".into(),
                    cleanup: None,
                    warning: None,
                    more_information: Some(format!("ScreenScraper provider record {}", receipt.provider_record_id)),
                });
                self.start_database_action(context, false);
            }
            Err(error) => {
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message: "Metadata could not be applied; the previous library state was preserved.".into(),
                    cleanup: None,
                    warning: Some(error.to_string()),
                    more_information: None,
                });
            }
        }
    }

    pub(crate) fn poll_database_load(&mut self, _context: &egui::Context) {
        let Some(settled) = database_load::poll_database_load(
            &mut self.database_state,
            self.database_generation,
            &mut self.sources_ui.pending_source_scan_summary,
        ) else {
            return;
        };
        if let Some(entry) = settled.history {
            self.history.record(entry);
        }
        if let Some(feedback) = settled.feedback {
            self.feedback = Some(feedback);
        }
        let duplicate_report = self
            .database_state
            .snapshot()
            .map(|snapshot| snapshot.duplicate_report.clone());
        prune_duplicate_review_selection(
            &mut self.health_duplicate_ui.selected_duplicate_group,
            &mut self.health_duplicate_ui.selected_duplicate_archive,
            duplicate_report.as_ref(),
        );

        // The merged row set may have just changed (a cache reload/scan
        // just settled) - recompute the cached filtered-index list against
        // it now rather than leaving it stale until the next live refresh
        // or filter-text edit. Only meaningful once a live snapshot
        // exists; the cache-only preview shown before that filters itself
        // fresh each frame instead (see `show_loaded_data`'s Loading
        // branch).
        if let LoadState::Ready(data) = &self.state {
            let merged =
                build_display_rows(&data.records, &data.rows, self.database_state.snapshot());
            self.library_ui.filtered_rows = matching_row_indices(&merged, &self.library_ui.filter);
            self.prune_selection(&merged);
        }
    }

    /// Removes any exact-identity entry from `selected_archives` - and
    /// clears `selected_archive` if it was the one that vanished - that
    /// no longer names any row in `merged_rows`, the just-recomputed
    /// live+cache catalogue (requirement 7: "remove selections that no
    /// longer exist in the loaded catalogue"). Called from both
    /// `poll_load` and `poll_database_load`, right where each already
    /// recomputes `filtered_rows` against the same merged list, so
    /// selection state is never one step behind what is actually on
    /// screen. Compares exact `PathBuf` identity only, never a lossy
    /// display string, and never touches row indices - there are none to
    /// go stale here in the first place.
    pub(crate) fn prune_selection(&mut self, merged_rows: &[ArchiveRow]) {
        self.archive_context.prune(merged_rows);
    }

    // --- Doctor Stage 1A ------------------------------------------------
}

impl Drop for ArchiveFsApp {
    fn drop(&mut self) {
        // Database and source actions include the GUI's catalogue scan paths.
        // Retaining and joining these handles prevents a normal window close
        // from abandoning SQLite in the middle of a write transaction. This
        // cannot run for SIGKILL, power loss, or process abort; SQLite's
        // rollback journal remains the safety boundary for those cases.
        if let DatabaseState::Loading { worker, .. } = &mut self.database_state
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
        if let Some(mut action) = self.sources_ui.source_action.take()
            && let Some(worker) = action.worker.take()
        {
            let _ = worker.join();
        }
    }
}

impl ArchiveFsApp {
    pub(crate) fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        app_polling::poll_and_reconcile(self, context);
        app_polling::start_view_gated_work(self, context);
        let app_frame::FrameReadiness {
            loading,
            busy,
            has_database,
            archive_actions_blocked,
            archive_action_block_reason,
            action_readiness_debug_lines,
            missing_removal_available,
        } = app_frame::frame_readiness(self, context);

        let navigation_request = app_shell::show_shell(
            context,
            app_shell::ShellInputs {
                advanced_view: self.ui_mode == GuiMode::AdvancedView,
                simple_view: self.ui_mode == GuiMode::Simple,
                view: self.view,
                tools_overlay: self.tools_overlay,
                loading,
                busy,
                has_database,
                selection_count: self.archive_context.selected.len(),
                show_activity: self.show_activity,
                source_actions_available: !busy && self.source_action_available(),
            },
        );
        app_reactions::apply_shell_request(self, context, navigation_request);

        app_overlays::show_global_overlays(self, context);

        let outcome = app_pages::show_pages(
            self,
            context,
            app_pages::PageDispatchInputs {
                busy,
                archive_actions_blocked,
                archive_action_block_reason,
                action_readiness_debug_lines,
                missing_removal_available,
            },
        );
        app_reactions::apply_page_requests(self, context, outcome);
    }
}

impl eframe::App for ArchiveFsApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.update(ui.ctx(), frame);
    }
}

/// The one GUI-owned EmuWiz configuration snapshot.
///
/// Rendering is deliberately unable to load this from disk. A failed deliberate
/// reload keeps the last usable value, while retaining an actionable error for the
/// configuration UI.
#[derive(Clone, Debug)]
pub(crate) struct GuiConfigSnapshot {
    pub(crate) current: Option<Config>,
    pub(crate) last_error: Option<String>,
    pub(crate) load_attempts: u64,
    pub(crate) loader: fn() -> Result<Config, String>,
}

impl GuiConfigSnapshot {
    pub(crate) fn load_with(loader: fn() -> Result<Config, String>) -> Self {
        match loader() {
            Ok(config) => Self {
                current: Some(config),
                last_error: None,
                load_attempts: 1,
                loader,
            },
            Err(error) => Self {
                current: None,
                last_error: Some(error),
                load_attempts: 1,
                loader,
            },
        }
    }

    pub(crate) fn load_default() -> Self {
        Self::load_with(load_default_gui_config)
    }

    pub(crate) fn reload_with(
        &mut self,
        loader: fn() -> Result<Config, String>,
    ) -> Result<(), String> {
        self.load_attempts = self.load_attempts.wrapping_add(1);
        match loader() {
            Ok(config) => {
                self.current = Some(config);
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                Err(error)
            }
        }
    }

    pub(crate) fn reload_default(&mut self) -> Result<(), String> {
        self.reload_with(self.loader)
    }

    pub(crate) fn source_roots(&self) -> Result<&[PathBuf], String> {
        self.current
            .as_ref()
            .map(|config| config.source_folders.as_slice())
            .ok_or_else(|| {
                self.last_error
                    .clone()
                    .unwrap_or_else(|| "EmuWiz configuration has not been loaded yet.".to_string())
            })
    }
}

pub(crate) fn load_default_gui_config() -> Result<Config, String> {
    Config::load_default().map_err(|error| error.to_string())
}

#[derive(Debug)]
pub(crate) enum AppOperationRequest {
    Archive(OperationRequest),
    MountAll(Vec<MountAllItem>),
    UnmountAll {
        items: Vec<UnmountAllItem>,
        cleanup_after_unmount: bool,
    },
    PlatformAssignment {
        archive_path: PathBuf,
        action: PlatformAction,
    },
    BulkPlatformAssignment {
        archive_paths: Vec<PathBuf>,
        kind: BulkPlatformActionKind,
    },
    RemoveMissing(Vec<PathBuf>),
    /// The moved-library "fix it here" card's navigation-only actions. None
    /// of these change catalogue rows, source configuration, or the
    /// filesystem themselves: they route the user to the existing explicit
    /// flow (`SourcesTab::Libraries` folder editor, `SourceAction::ScanAll`,
    /// missing-only review mode). The card's own "Clean up confirmed missing
    /// entries" button does not use this enum - it opens the existing
    /// `confirm_remove_missing` dialog directly, so the typed-count gate and
    /// explicit confirmation are unchanged.
    UpdateGameFolder,
    FullRescan,
    ReviewMissingGames,
    InspectArchive(PathBuf),
    /// Navigates to the Library Views page with this archive as the
    /// "Show in Library View preview" focus - see
    /// `ArchiveFsApp::library_view_focus_archive`'s doc comment. Never
    /// starts a Preview itself: it only helps the user find what a
    /// *previously run* preview already said about this archive, since
    /// jumping pages must never bypass Preview's own safety gate.
    ShowInLibraryViews(PathBuf),
    /// Opens the first-class Cheats & Mods workspace for this exact
    /// archive - see `ArchiveFsApp::open_cheats_mods_workspace`.
    OpenCheatsMods(PathBuf),
    /// Navigates to the existing Verify Games (DAT Sources) page - the
    /// selected-game DAT check section's own "Verify Games" call to
    /// action. Never starts an audit itself; it only opens the real,
    /// existing workflow that does.
    OpenDatSources,
    ApplyScreenScraperEnrichment(
        Box<crate::screenscraper_enrichment_page::ScreenScraperEnrichmentAction>,
    ),
}

pub(crate) struct ActionFeedback {
    pub(crate) succeeded: bool,
    pub(crate) message: String,
    pub(crate) cleanup: Option<CleanupFeedback>,
    pub(crate) warning: Option<String>,
    pub(crate) more_information: Option<String>,
}

pub(crate) struct CleanupFeedback {
    pub(crate) succeeded: bool,
    pub(crate) message: String,
}
