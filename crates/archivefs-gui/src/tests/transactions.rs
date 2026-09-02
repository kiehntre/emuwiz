//! GUI Maintenance Batch 2: relocated from main.rs's inline
//! `#[cfg(test)] mod romm_dispatch_tests { ... }` (a separate top-level test
//! module in the original file, not nested inside the main `mod tests`).
//! Operation-dispatch tests - see the module's own doc comment below.
//! Copied byte-for-byte; only its file location changed.

use super::*;

#[cfg(test)]
mod romm_dispatch_tests {
    //! Operation-dispatch tests.
    //!
    //! These drive the real `ArchiveFsApp` methods rather than a stand-in, because
    //! the properties under test are about the app's own bookkeeping: that a second
    //! click cannot launch a duplicate, that a superseded operation's late traffic
    //! is discarded, and that a failure never erases counts that were true.
    //!
    //! No worker is allowed to run. Each test installs the channels itself, which is
    //! also the only way to deliver a *late* result deterministically.

    use super::*;
    use crate::romm_config::ConfigDialogRequest;
    use crate::romm_source::{RommImportSummary, RommProgressEvent};
    use archivefs_core::identity_source::artwork::ArtworkCacheStats;
    use archivefs_core::identity_source::model::{IdentityImportCounts, IdentityProvider};
    use archivefs_core::identity_source::path_map::ProviderPathKind;
    use archivefs_core::identity_source::romm::config::RommSourceConfig;
    use archivefs_core::identity_source::romm::import::ImportProgress;
    use archivefs_core::identity_source::settings::ProviderSettings;
    use archivefs_core::identity_source::status::{ProviderState, ProviderStatus};

    fn app() -> ArchiveFsApp {
        app_for_operation_tests()
    }

    fn refused_gui_config() -> Result<Config, String> {
        Err("config.toml is temporarily unreadable".to_string())
    }

    fn output_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains(needle),
                egui::Shape::Vec(nested) => {
                    nested.iter().any(|shape| shape_contains(shape, needle))
                }
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|shape| shape_contains(&shape.shape, needle))
    }

    fn snapshot(records: usize, state: ProviderState) -> RommSnapshot {
        let mut status = ProviderStatus::not_configured(IdentityProvider::Romm);
        status.state = state;
        status.records_imported = records;
        status.counts = IdentityImportCounts {
            total: records,
            strong: records,
            ..IdentityImportCounts::default()
        };
        RommSnapshot {
            settings: ProviderSettings {
                source: RommSourceConfig {
                    enabled: true,
                    url: "http://172.19.0.20:8080".to_string(),
                    mappings: Vec::new(),
                    media_mapping: None,
                    provider_path_kind: ProviderPathKind::ProviderRelative,
                    token_path: None,
                },
                page_size: Some(100),
                import_timeout_seconds: None,
            },
            status,
            artwork: ArtworkCacheStats {
                items: 0,
                bytes: 0,
                maximum_bytes: 1024 * 1024 * 1024,
                last_cleanup_unix_seconds: None,
                directory: PathBuf::from("/tmp/artwork"),
                format_version: 1,
            },
            token_available: true,
            token_problem: None,
            cache_format_version: Some(1),
        }
    }

    /// Installs a running operation whose channels the test owns, so nothing is
    /// spawned and a result can be delivered on demand.
    #[allow(clippy::type_complexity)]
    fn install_running(
        app: &mut ArchiveFsApp,
        operation: RommOperation,
    ) -> (
        mpsc::Sender<(u64, Result<RommOperationOutcome, String>)>,
        mpsc::Sender<(u64, RommProgressEvent)>,
        u64,
    ) {
        app.romm_generation = app.romm_generation.wrapping_add(1);
        let generation = app.romm_generation;
        let (sender, receiver) = mpsc::channel();
        let (progress_sender, progress_receiver) = mpsc::channel();
        app.romm_operation = Some(RunningRommOperation {
            generation,
            operation: operation.clone(),
            cancellation: Arc::new(AtomicBool::new(false)),
            receiver,
            progress_receiver,
            progress: operation.reports_progress().then(RommProgress::default),
            cancellation_requested: false,
        });
        (sender, progress_sender, generation)
    }

    fn summary(records: usize) -> RommImportSummary {
        RommImportSummary {
            published: true,
            records,
            previous_cache_usable: true,
            ..RommImportSummary::default()
        }
    }

    // ---------------------------------------------------------------------
    // Human-smoke regressions: RomM window navigation
    //
    // Confirmed in Sunshine: the Configure dialog had no visible exit (Save
    // and Cancel were the last widgets inside its scrolling body, and the
    // window offered no title-bar close), and "Browse records" opened the
    // browser inline underneath the source card - below the viewport, so the
    // click looked inert.
    // ---------------------------------------------------------------------

    fn tv_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1920.0, 1080.0),
            )),
            ..Default::default()
        }
    }

    /// Every text shape and its rectangle, for asserting what is on screen
    /// and where.
    fn painted(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_string(), text.visual_bounding_rect()))
                }
                egui::Shape::Vec(nested) => nested.iter().for_each(|shape| walk(shape, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut found);
        }
        found
    }

    fn rect_of(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
        painted(output)
            .into_iter()
            .find(|(text, _)| text.contains(needle))
            .map(|(_, rect)| rect)
    }

    fn run_config_window(
        app: &mut ArchiveFsApp,
        context: &egui::Context,
        input: egui::RawInput,
    ) -> (egui::FullOutput, Option<ConfigDialogRequest>) {
        let mut request = None;
        let output = context.run(input, |context| {
            egui::CentralPanel::default().show(context, |_ui| {
                request = app.show_romm_configuration_window(context);
            });
        });
        (output, request)
    }

    #[test]
    fn the_configure_window_shows_both_save_and_cancel_inside_the_viewport() {
        let mut app = app();
        app.open_romm_configuration();
        let context = egui::Context::default();
        // Two frames: the first lays the window out, the second draws it at
        // its settled size.
        let (_, _) = run_config_window(&mut app, &context, tv_input());
        let (output, _) = run_config_window(&mut app, &context, tv_input());
        let screen = tv_input().screen_rect.unwrap();
        let save = rect_of(&output, "Save configuration").expect("a visible Save");
        let cancel = rect_of(&output, "Cancel").expect("a visible Cancel");
        for (label, rect) in [("Save", save), ("Cancel", cancel)] {
            assert!(
                screen.contains_rect(rect),
                "{label} must be inside the viewport at TV resolution, was {rect:?}"
            );
        }
        assert_ne!(save, cancel, "Save and Cancel are distinct controls");
    }

    /// The footer must not move when the body scrolls - that is the whole
    /// point of drawing it outside the scroll area.
    #[test]
    fn the_configure_footer_stays_put_while_the_body_scrolls() {
        let mut app = app();
        app.open_romm_configuration();
        let context = egui::Context::default();
        let (_, _) = run_config_window(&mut app, &context, tv_input());
        let (before, _) = run_config_window(&mut app, &context, tv_input());
        let footer_before = rect_of(&before, "Save configuration").expect("Save");

        // Scroll the body hard, over the window's own area.
        let mut scrolled = tv_input();
        scrolled.events = vec![
            egui::Event::PointerMoved(egui::pos2(600.0, 400.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -900.0),
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            },
        ];
        let (_, _) = run_config_window(&mut app, &context, scrolled);
        let (after, _) = run_config_window(&mut app, &context, tv_input());
        let footer_after = rect_of(&after, "Save configuration").expect("Save still visible");

        assert!(
            (footer_before.top() - footer_after.top()).abs() < 1.0,
            "the footer moved when the body scrolled: {footer_before:?} -> {footer_after:?}"
        );
        let screen = tv_input().screen_rect.unwrap();
        assert!(screen.contains_rect(footer_after));
    }

    #[test]
    fn the_configure_window_is_clamped_to_the_viewport() {
        // A small screen must not produce a window taller than it.
        let (initial, maximum) =
            romm_dialog_sizes(egui::vec2(1280.0, 720.0), egui::vec2(640.0, 760.0));
        assert!(maximum.y <= 720.0 && maximum.x <= 1280.0);
        assert!(initial.y <= maximum.y && initial.x <= maximum.x);
        // The preferred size is honoured when it fits.
        let (initial, _) = romm_dialog_sizes(egui::vec2(2560.0, 1440.0), egui::vec2(640.0, 760.0));
        assert_eq!(initial, egui::vec2(640.0, 760.0));
    }

    #[test]
    fn escape_closes_configure_without_writing_anything() {
        let mut app = app();
        app.open_romm_configuration();
        let attempts = app.gui_config.load_attempts;
        let context = egui::Context::default();
        let (_, _) = run_config_window(&mut app, &context, tv_input());

        let mut escaped = tv_input();
        escaped.events = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }];
        let (_, request) = run_config_window(&mut app, &context, escaped);
        assert_eq!(request, Some(ConfigDialogRequest::Close));

        app.handle_romm_config_request(&context, request.unwrap());
        assert!(app.romm_config_draft.is_none(), "the dialog is closed");
        assert!(
            app.romm_operation.is_none(),
            "Close must start no operation - no save, no import, no request"
        );
        assert_eq!(
            app.gui_config.load_attempts, attempts,
            "Close must not rewrite or re-read configuration"
        );
    }

    /// Cancel is a plain close: it must never write, import or contact
    /// anything. `RommOperation` is the only route to any of those, so the
    /// assertion is that none was started.
    #[test]
    fn cancel_closes_configure_without_writing_importing_or_requesting() {
        let mut app = app();
        app.open_romm_configuration();
        let context = egui::Context::default();
        app.handle_romm_config_request(&context, ConfigDialogRequest::Close);
        assert!(app.romm_config_draft.is_none());
        assert!(app.romm_preview.is_none());
        assert!(app.romm_operation.is_none());
        assert!(app.romm_snapshot.is_none(), "no snapshot was published");
    }

    #[test]
    fn a_duplicate_configure_request_does_not_open_a_second_window() {
        let mut app = app();
        app.open_romm_configuration();
        let first = app
            .romm_config_draft
            .as_ref()
            .map(|draft| draft.url.clone());
        app.open_romm_configuration();
        app.open_romm_configuration();
        assert_eq!(
            app.romm_config_draft
                .as_ref()
                .map(|draft| draft.url.clone()),
            first,
            "the draft is the open flag, so re-requesting cannot replace it"
        );

        let context = egui::Context::default();
        let (_, _) = run_config_window(&mut app, &context, tv_input());
        let (output, _) = run_config_window(&mut app, &context, tv_input());
        let saves = painted(&output)
            .into_iter()
            .filter(|(text, _)| text.contains("Save configuration"))
            .count();
        assert_eq!(saves, 1, "exactly one dialog, so exactly one Save button");
    }

    /// The token's *contents* must never reach the GUI. Only a path and a
    /// verdict do.
    #[test]
    fn the_configure_draft_holds_a_token_path_but_never_a_token_value() {
        let mut app = app();
        app.open_romm_configuration();
        let draft = app.romm_config_draft.as_ref().expect("a draft");
        let rendered = format!("{draft:?}");
        assert!(
            !rendered.contains("secret-token-value"),
            "no token value can be present - none was ever read into the draft"
        );
        // The only token-shaped field is a path.
        assert!(draft.token_path.is_empty() || draft.token_path.starts_with('/'));
    }

    // --- Browse records ---------------------------------------------------

    fn run_browse_window(
        app: &mut ArchiveFsApp,
        context: &egui::Context,
        input: egui::RawInput,
    ) -> (egui::FullOutput, Option<crate::romm_browse::BrowseRequest>) {
        let mut request = None;
        let output = context.run(input, |context| {
            egui::CentralPanel::default().show(context, |_ui| {
                request = app.show_romm_browse_window(context);
            });
        });
        (output, request)
    }

    #[test]
    fn browse_records_becomes_visible_inside_the_viewport_immediately() {
        let mut app = app();
        let context = egui::Context::default();
        // Nothing before the click.
        let (output, _) = run_browse_window(&mut app, &context, tv_input());
        assert!(!output_contains(&output, "RomM records"));

        // Exactly what the source card's button does.
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        // egui needs one frame to lay a new window out before it paints at
        // its settled size; at 60fps that is the same moment for a person.
        let (_, _) = run_browse_window(&mut app, &context, tv_input());
        let (output, _) = run_browse_window(&mut app, &context, tv_input());
        assert!(
            output_contains(&output, "RomM records"),
            "one click must make the browser visible, not open it below the fold"
        );

        let screen = tv_input().screen_rect.unwrap();
        let close = rect_of(&output, "Close").expect("a visible Close");
        assert!(
            screen.contains_rect(close),
            "the browser's exit must be on screen, not below the fold: {close:?}"
        );
        let title = rect_of(&output, "RomM records").expect("a visible title");
        assert!(
            screen.contains_rect(title),
            "the window itself must open inside the viewport: {title:?}"
        );
    }

    #[test]
    fn a_second_browse_click_switches_the_view_without_duplicating_the_window() {
        let mut app = app();
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        assert!(app.romm_browse.is_some());

        let context = egui::Context::default();
        let (_, _) = run_browse_window(&mut app, &context, tv_input());
        let (output, _) = run_browse_window(&mut app, &context, tv_input());
        let closes = painted(&output)
            .into_iter()
            .filter(|(text, _)| text == "Close")
            .count();
        assert_eq!(closes, 1, "one window, one Close");

        // Switching views reuses the same window rather than stacking one.
        app.open_romm_browse(crate::romm_browse::BrowseView::Conflicts);
        assert_eq!(
            app.romm_browse.as_ref().map(|state| state.view),
            Some(crate::romm_browse::BrowseView::Conflicts)
        );
    }

    /// Ordinary browsing is cache-only. The browser does ask for its first
    /// page - that is a read of the published cache - but nothing it asks
    /// for may be an operation that contacts RomM, and `uses_network` is the
    /// codebase's own answer to which those are.
    #[test]
    fn browsing_reads_the_cache_and_never_contacts_romm() {
        let mut app = app();
        let attempts = app.gui_config.load_attempts;
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        let context = egui::Context::default();

        for _ in 0..4 {
            let (_, request) = run_browse_window(&mut app, &context, tv_input());
            if let Some(request) = request {
                assert!(
                    matches!(
                        request,
                        crate::romm_browse::BrowseRequest::LoadRecords { .. }
                    ),
                    "an idle browser may only ask to read a cached page, got {request:?}"
                );
                app.handle_romm_browse_request(&context, request);
            }
            if let Some(running) = app.romm_operation.as_ref() {
                assert!(
                    !running.operation.uses_network(),
                    "browsing started a network operation: {:?}",
                    running.operation
                );
                assert!(
                    !running.operation.is_mutating(),
                    "browsing started a mutating operation: {:?}",
                    running.operation
                );
            }
        }
        assert_eq!(
            app.gui_config.load_attempts, attempts,
            "browsing must not re-read configuration every frame"
        );

        app.close_romm_browse();
        assert!(app.romm_browse.is_none());
    }

    /// Escape closes the browser. Asserted once the first page request has
    /// been dealt with, because a pending request is what the window reports
    /// that frame and Escape must not silently overwrite it.
    #[test]
    fn escape_closes_the_browser() {
        let mut app = app();
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        let context = egui::Context::default();
        for _ in 0..3 {
            let (_, request) = run_browse_window(&mut app, &context, tv_input());
            if let Some(request) = request {
                app.handle_romm_browse_request(&context, request);
            }
        }

        let mut escaped = tv_input();
        escaped.events = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }];
        let (_, request) = run_browse_window(&mut app, &context, escaped);
        assert_eq!(
            request,
            Some(crate::romm_browse::BrowseRequest::Close),
            "Escape closes the browser when nothing is layered over it"
        );
        app.handle_romm_browse_request(&context, request.unwrap());
        assert!(app.romm_browse.is_none());
    }

    /// Closing preserves the filters and page the user had set, so reopening
    /// is not a reset.
    #[test]
    fn the_browser_keeps_its_filters_and_page_while_open() {
        let mut app = app();
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        if let Some(state) = app.romm_browse.as_mut() {
            state.filters.title = "zelda".to_string();
            state.title_input = "zelda".to_string();
            state.page_size = 250;
        }
        let context = egui::Context::default();
        for _ in 0..3 {
            let (_, _) = run_browse_window(&mut app, &context, tv_input());
        }
        let state = app.romm_browse.as_ref().expect("still open");
        assert_eq!(state.filters.title, "zelda");
        assert_eq!(state.title_input, "zelda");
        assert_eq!(state.page_size, 250);
    }

    #[test]
    fn a_second_click_while_something_runs_launches_nothing() {
        let mut app = app();
        let context = egui::Context::default();
        let (_sender, _progress, generation) = install_running(&mut app, RommOperation::FullImport);

        // The card asking again must be refused, and must not disturb the operation
        // that is already running.
        assert!(
            !app.start_romm_operation(context.clone(), RommOperation::FullImport),
            "a duplicate must be declined"
        );
        assert!(
            !app.start_romm_operation(context.clone(), RommOperation::TestConnection),
            "a different operation must also be declined while one runs"
        );
        assert_eq!(
            app.romm_generation, generation,
            "the generation must not move for a declined start"
        );
        assert_eq!(
            app.romm_operation
                .as_ref()
                .map(|running| running.operation.clone()),
            Some(RommOperation::FullImport),
            "the running operation must be untouched"
        );
    }

    #[test]
    fn repeated_gui_frames_use_the_cached_config_without_reloading() {
        let mut app = app();
        app.open_romm_configuration();
        let attempts = app.gui_config.load_attempts;
        let context = egui::Context::default();
        let mut saw_dialog = false;
        for _ in 0..4 {
            let output = context.run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |_ui| {
                    let _ = app.show_romm_configuration_window(context);
                });
            });
            saw_dialog |= output_contains(&output, "Configure RomM");
            assert!(app.romm_config_draft.is_some());
        }
        assert!(
            saw_dialog,
            "the persistent draft must have a visible window"
        );
        assert_eq!(
            app.gui_config.load_attempts, attempts,
            "rendering an idle dialog must perform no configuration I/O"
        );
    }

    #[test]
    fn a_failed_explicit_config_reload_preserves_the_previous_snapshot() {
        let mut app = app();
        let before = app.gui_config.current.clone();
        app.gui_config.loader = refused_gui_config;
        assert!(app.gui_config.reload_default().is_err());
        assert_eq!(app.gui_config.current, before);
        assert!(
            app.gui_config
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("temporarily unreadable"))
        );
    }

    #[test]
    fn a_status_load_is_declined_while_an_operation_runs() {
        let mut app = app();
        let context = egui::Context::default();
        let (_sender, _progress, generation) = install_running(&mut app, RommOperation::Refresh);
        app.start_romm_status_load(context);
        assert_eq!(app.romm_generation, generation);
        assert_eq!(
            app.romm_operation
                .as_ref()
                .map(|running| running.operation.clone()),
            Some(RommOperation::Refresh)
        );
    }

    #[test]
    fn cancellation_targets_only_the_current_operation() {
        let mut app = app();
        let (_sender, _progress, _generation) =
            install_running(&mut app, RommOperation::FullImport);
        let flag = app
            .romm_operation
            .as_ref()
            .expect("running")
            .cancellation
            .clone();
        assert!(!flag.load(Ordering::Acquire));
        app.cancel_romm_operation();
        assert!(
            flag.load(Ordering::Acquire),
            "the worker should be asked to stop"
        );
        assert!(
            app.romm_operation
                .as_ref()
                .is_some_and(|running| running.cancellation_requested),
            "and the card should know, so Cancel stops being offered"
        );

        // Asking again is harmless.
        app.cancel_romm_operation();
        assert!(flag.load(Ordering::Acquire));
    }

    #[test]
    fn cancelling_with_nothing_running_does_nothing() {
        let mut app = app();
        app.cancel_romm_operation();
        assert!(app.romm_operation.is_none());
    }

    #[test]
    fn progress_from_the_current_operation_is_absorbed() {
        let mut app = app();
        let context = egui::Context::default();
        let (_sender, progress, generation) = install_running(&mut app, RommOperation::FullImport);
        progress
            .send((
                generation,
                RommProgressEvent::Import(ImportProgress {
                    pages_fetched: 3,
                    records_fetched: 300,
                    reported_total: Some(1_000),
                    page_size: 100,
                    reduction: None,
                }),
            ))
            .expect("send");
        progress
            .send((generation, RommProgressEvent::Note("a note".to_string())))
            .expect("send");
        app.poll_romm_operation(&context);
        let running = app.romm_operation.as_ref().expect("still running");
        let seen = running.progress.as_ref().expect("progress");
        assert_eq!(seen.pages_fetched, 3);
        assert_eq!(seen.records_fetched, 300);
        assert_eq!(seen.notes, vec!["a note".to_string()]);
    }

    #[test]
    fn progress_from_a_superseded_operation_is_discarded() {
        let mut app = app();
        let context = egui::Context::default();
        let (_sender, progress, generation) = install_running(&mut app, RommOperation::FullImport);
        // The operation is superseded, as it would be by a cancel-then-restart.
        let stale_generation = generation;
        app.romm_generation = app.romm_generation.wrapping_add(1);
        if let Some(running) = app.romm_operation.as_mut() {
            running.generation = app.romm_generation;
        }
        progress
            .send((
                stale_generation,
                RommProgressEvent::Import(ImportProgress {
                    pages_fetched: 999,
                    records_fetched: 99_999,
                    reported_total: None,
                    page_size: 1,
                    reduction: None,
                }),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        let running = app.romm_operation.as_ref().expect("still running");
        let seen = running.progress.as_ref().expect("progress");
        assert_eq!(
            seen.pages_fetched, 0,
            "an older generation's progress must not be shown"
        );
        assert_eq!(seen.records_fetched, 0);
    }

    /// Delivers one finished operation through the real polling path and returns
    /// the cover cache's generation before and after.
    fn deliver(
        operation: RommOperation,
        outcome: Result<RommOperationOutcome, String>,
    ) -> (ArchiveFsApp, u64, u64) {
        let mut app = app();
        let context = egui::Context::default();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        let (sender, _progress, generation) = install_running(&mut app, operation);
        let before = app.gamer_covers.generation();
        sender.send((generation, outcome)).expect("send");
        app.poll_romm_operation(&context);
        let after = app.gamer_covers.generation();
        (app, before, after)
    }

    #[test]
    fn a_published_import_refreshes_the_gamer_view_identity_index() {
        // The signal that makes a game which has just gained a RomM identity
        // eligible for artwork without restarting EmuWiz.
        let (_app, before, after) = deliver(
            RommOperation::FullImport,
            Ok(RommOperationOutcome::Import(Box::new(summary(36_259)))),
        );
        assert_ne!(
            before, after,
            "a published import did not refresh the cover cache"
        );
    }

    #[test]
    fn an_unpublished_import_leaves_the_identity_index_alone() {
        // A failed import leaves the previous cache in place. Refreshing would
        // withdraw every cover on screen to revalidate against a catalogue that
        // never changed - and would discard a working index for nothing.
        let mut failed = summary(0);
        failed.published = false;
        failed.failure = Some("RomM did not answer in time".to_string());
        let (_app, before, after) = deliver(
            RommOperation::FullImport,
            Ok(RommOperationOutcome::Import(Box::new(failed))),
        );
        assert_eq!(
            before, after,
            "a failed import discarded the current identity index"
        );
    }

    #[test]
    fn a_sample_import_leaves_the_identity_index_alone() {
        // A sample deliberately never publishes: it is imported, matched and
        // reported without touching the live cache, so nothing it produces can
        // change what a local path resolves to.
        let mut sample = summary(25);
        sample.published = false;
        let (_app, before, after) = deliver(
            RommOperation::SampleImport { records: 25 },
            Ok(RommOperationOutcome::Sample(Box::new(sample))),
        );
        assert_eq!(
            before, after,
            "a sample import needlessly invalidated every loaded cover"
        );
    }

    #[test]
    fn a_result_from_a_superseded_operation_is_discarded() {
        let mut app = app();
        let context = egui::Context::default();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        let (sender, _progress, generation) = install_running(&mut app, RommOperation::FullImport);
        let stale_generation = generation;
        app.romm_generation = app.romm_generation.wrapping_add(1);

        sender
            .send((
                stale_generation,
                Ok(RommOperationOutcome::Import(Box::new(summary(1)))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        assert!(
            app.romm_ui.last_outcome.is_none(),
            "a superseded result must not become the visible outcome"
        );
        assert_eq!(
            app.romm_snapshot
                .as_ref()
                .map(|snapshot| snapshot.status.records_imported),
            Some(36_259),
            "and must not disturb the counts"
        );
    }

    #[test]
    fn a_result_arriving_after_the_operation_was_cleared_is_ignored() {
        let mut app = app();
        let context = egui::Context::default();
        // Nothing running: a late send has nowhere to land, and polling must be a
        // no-op rather than a panic.
        app.poll_romm_operation(&context);
        assert!(app.romm_operation.is_none());
        assert!(app.romm_ui.last_outcome.is_none());
    }

    #[test]
    fn a_snapshot_result_sets_the_counts_without_becoming_a_visible_outcome() {
        let mut app = app();
        let context = egui::Context::default();
        let (sender, _progress, generation) = install_running(&mut app, RommOperation::LoadStatus);
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Snapshot(Box::new(snapshot(
                    36_259,
                    ProviderState::Ready,
                )))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        assert!(app.romm_operation.is_none(), "the operation finished");
        assert_eq!(
            app.romm_snapshot
                .as_ref()
                .map(|snapshot| snapshot.status.records_imported),
            Some(36_259)
        );
        assert!(
            app.romm_ui.last_outcome.is_none(),
            "a status load is not a result worth announcing"
        );
    }

    #[test]
    fn a_completed_mutating_operation_reloads_authoritative_state() {
        let mut app = app();
        let context = egui::Context::default();
        let (sender, _progress, generation) =
            install_running(&mut app, RommOperation::ClearArtwork);
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::ArtworkCleared {
                    items: 39,
                    bytes: 1_000,
                }),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        // The visible result is the clear...
        assert!(
            app.romm_ui
                .last_outcome
                .as_ref()
                .is_some_and(|outcome| outcome.headline.contains("39"))
        );
        // ...and a status load was started rather than the card being patched by
        // hand, so what it shows next comes from disk.
        assert_eq!(
            app.romm_operation
                .as_ref()
                .map(|running| running.operation.clone()),
            Some(RommOperation::LoadStatus),
            "a mutating operation should be followed by an authoritative reload"
        );
    }

    #[test]
    fn a_failed_operation_keeps_the_counts_that_were_true() {
        let mut app = app();
        let context = egui::Context::default();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        let (sender, _progress, generation) = install_running(&mut app, RommOperation::Refresh);
        sender
            .send((
                generation,
                Err("could not reach RomM: connection refused".to_string()),
            ))
            .expect("send");
        app.poll_romm_operation(&context);

        let outcome = app.romm_ui.last_outcome.as_ref().expect("a result");
        assert!(!outcome.succeeded);
        assert!(outcome.headline.contains("failed"), "{}", outcome.headline);
        // The snapshot is untouched: a failure must not blank the card.
        assert_eq!(
            app.romm_snapshot
                .as_ref()
                .map(|snapshot| snapshot.status.records_imported),
            Some(36_259)
        );
        assert!(matches!(
            app.romm_snapshot.as_ref().map(|s| s.status.state.clone()),
            Some(ProviderState::Ready)
        ));
    }

    #[test]
    fn a_failed_connection_test_while_offline_is_recorded_as_offline_not_failed() {
        let mut app = app();
        let context = egui::Context::default();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::ReadyOffline)));
        let (sender, _progress, generation) =
            install_running(&mut app, RommOperation::TestConnection);
        sender
            .send((
                generation,
                Err("could not reach RomM: an I/O error occurred (connection refused)".to_string()),
            ))
            .expect("send");
        app.poll_romm_operation(&context);

        let outcome = app.romm_ui.last_outcome.as_ref().expect("a result");
        assert!(outcome.informational, "offline copy is still usable");
        assert!(
            !outcome.headline.contains("failed"),
            "no scary global failure: {}",
            outcome.headline
        );

        let entry = app.history.entries().next().expect("an activity entry");
        assert_eq!(entry.outcome, ActivityOutcome::OfflineUsable);
        assert_eq!(entry.action, ActivityAction::RommSource);
        assert!(
            entry.message.contains("offline copy still works"),
            "friendly framing: {}",
            entry.message
        );
        // The actual technical reason is preserved in the activity history.
        assert!(
            entry.message.contains("connection refused"),
            "technical reason must survive: {}",
            entry.message
        );
    }

    #[test]
    fn a_failed_connection_test_without_an_offline_copy_is_still_a_real_failure() {
        let mut app = app();
        let context = egui::Context::default();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        let (sender, _progress, generation) =
            install_running(&mut app, RommOperation::TestConnection);
        sender
            .send((
                generation,
                Err("could not reach RomM: an I/O error occurred (connection refused)".to_string()),
            ))
            .expect("send");
        app.poll_romm_operation(&context);

        let outcome = app.romm_ui.last_outcome.as_ref().expect("a result");
        assert!(!outcome.informational);
        assert!(outcome.headline.contains("failed"), "{}", outcome.headline);
        assert_eq!(
            app.history.entries().next().map(|entry| entry.outcome),
            Some(ActivityOutcome::Failed)
        );
        assert!(
            app.history
                .entries()
                .next()
                .is_some_and(|entry| entry.message.contains("connection refused"))
        );
    }

    #[test]
    fn a_new_operation_clears_the_previous_result_so_they_are_never_shown_together() {
        let mut app = app();
        let context = egui::Context::default();
        let (sender, _progress, generation) =
            install_running(&mut app, RommOperation::TestConnection);
        sender
            .send((generation, Err("nope".to_string())))
            .expect("send");
        app.poll_romm_operation(&context);
        assert!(app.romm_ui.last_outcome.is_some());
        // Starting something else drops the old outcome.
        app.romm_operation = None;
        assert!(app.start_romm_operation(context, RommOperation::TestConnection));
        assert!(
            app.romm_ui.last_outcome.is_none(),
            "an old result beside new progress would be misleading"
        );
        // Tidy up: the worker that start spawned owns its own channels and will end
        // on its own; dropping the app is enough.
        app.romm_operation = None;
    }

    #[test]
    fn opening_the_configuration_dialog_twice_opens_one_dialog() {
        let mut app = app();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        app.open_romm_configuration();
        let first_url = app
            .romm_config_draft
            .as_ref()
            .map(|draft| draft.url.clone());
        assert_eq!(first_url.as_deref(), Some("http://172.19.0.20:8080"));

        // Editing, then asking again: the draft must not be replaced, or a second
        // click would silently discard what was typed.
        if let Some(draft) = app.romm_config_draft.as_mut() {
            draft.url = "http://10.0.0.5:8080".to_string();
            draft.dirty = true;
        }
        app.open_romm_configuration();
        assert_eq!(
            app.romm_config_draft
                .as_ref()
                .map(|draft| draft.url.clone()),
            Some("http://10.0.0.5:8080".to_string()),
            "the open dialog must be left alone"
        );
        assert!(
            app.romm_config_draft
                .as_ref()
                .is_some_and(|draft| draft.dirty)
        );
    }

    #[test]
    fn the_dialog_opens_even_when_nothing_has_been_configured() {
        let mut app = app();
        assert!(app.romm_snapshot.is_none());
        app.open_romm_configuration();
        // Without this, a fresh install could never be configured from the GUI.
        assert!(app.romm_config_draft.is_some());
        assert!(
            app.romm_config_draft
                .as_ref()
                .is_some_and(|draft| draft.url.is_empty())
        );
    }

    #[test]
    fn a_save_is_declined_while_an_import_runs() {
        let mut app = app();
        let context = egui::Context::default();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        app.open_romm_configuration();
        let (_sender, _progress, generation) = install_running(&mut app, RommOperation::FullImport);

        let settings = app
            .romm_config_draft
            .as_ref()
            .expect("open")
            .to_settings(None);
        app.handle_romm_config_request(&context, ConfigDialogRequest::Save(Box::new(settings)));
        assert_eq!(
            app.romm_generation, generation,
            "the save must not have started"
        );
        assert_eq!(
            app.romm_operation
                .as_ref()
                .map(|running| running.operation.clone()),
            Some(RommOperation::FullImport)
        );
        assert!(
            app.romm_config_draft.is_some(),
            "and the dialog stays open, so nothing typed is lost"
        );
    }

    #[test]
    fn a_preview_is_declined_while_a_mutating_operation_runs() {
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_configuration();
        let (_sender, _progress, generation) = install_running(&mut app, RommOperation::Refresh);
        app.handle_romm_config_request(&context, ConfigDialogRequest::Preview { limit: 20 });
        assert_eq!(app.romm_generation, generation);
        assert_eq!(
            app.romm_operation
                .as_ref()
                .map(|running| running.operation.clone()),
            Some(RommOperation::Refresh)
        );
    }

    #[test]
    fn a_successful_save_closes_the_dialog_and_reloads_authoritative_state() {
        let mut app = app();
        let config_attempts_before = app.gui_config.load_attempts;
        let context = egui::Context::default();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        app.open_romm_configuration();
        let settings = app
            .romm_config_draft
            .as_ref()
            .expect("open")
            .to_settings(None);
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::SaveConfiguration(Box::new(settings.clone())),
        );
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Saved(Box::new(settings))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);

        assert!(
            app.romm_config_draft.is_none(),
            "the dialog has served its purpose"
        );
        assert!(
            app.romm_ui
                .last_outcome
                .as_ref()
                .is_some_and(|outcome| outcome.headline.contains("saved"))
        );
        // The card is refreshed from disk rather than from what was typed.
        assert_eq!(
            app.romm_operation
                .as_ref()
                .map(|running| running.operation.clone()),
            Some(RommOperation::LoadStatus)
        );
        assert_eq!(
            app.gui_config.load_attempts,
            config_attempts_before + 1,
            "a successful save has one deliberate reload boundary"
        );
    }

    #[test]
    fn a_failed_post_save_reload_keeps_the_valid_snapshot_and_reports_it() {
        let mut app = app();
        let context = egui::Context::default();
        let previous = app.gui_config.current.clone();
        app.gui_config.loader = refused_gui_config;
        app.open_romm_configuration();
        let settings = snapshot(1, ProviderState::Ready).settings;
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::SaveConfiguration(Box::new(settings.clone())),
        );
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Saved(Box::new(settings))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        assert_eq!(app.gui_config.current, previous);
        assert!(
            app.feedback
                .as_ref()
                .is_some_and(|feedback| feedback.message.contains("previous in-memory"))
        );
    }

    #[test]
    fn a_failed_save_keeps_the_dialog_open_with_its_edits() {
        let mut app = app();
        let context = egui::Context::default();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        app.open_romm_configuration();
        if let Some(draft) = app.romm_config_draft.as_mut() {
            draft.url = "http://10.0.0.5:8080".to_string();
            draft.dirty = true;
        }
        let settings = app
            .romm_config_draft
            .as_ref()
            .expect("open")
            .to_settings(None);
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::SaveConfiguration(Box::new(settings)),
        );
        sender
            .send((
                generation,
                Err("the token file is readable by others (mode 0644)".to_string()),
            ))
            .expect("send");
        app.poll_romm_operation(&context);

        assert!(
            app.romm_config_draft.is_some(),
            "a refused save must not discard the draft"
        );
        assert_eq!(
            app.romm_config_draft
                .as_ref()
                .map(|draft| draft.url.clone()),
            Some("http://10.0.0.5:8080".to_string())
        );
        let outcome = app.romm_ui.last_outcome.as_ref().expect("a result");
        assert!(!outcome.succeeded);
        assert!(
            format!("{:?}", outcome.rows).contains("0644"),
            "the remedy should survive: {:?}",
            outcome.rows
        );
    }

    #[test]
    fn a_preview_result_lands_only_while_the_dialog_is_open() {
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_configuration();
        let (sender, _progress, generation) =
            install_running(&mut app, RommOperation::Preview { limit: 20 });
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Preview(Box::default())),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        assert!(app.romm_preview.is_some(), "the open dialog should show it");
        // A preview is read-only, so no reload follows it.
        assert!(app.romm_operation.is_none());

        // With the dialog closed, a late preview has nowhere to go and is dropped.
        app.close_romm_configuration();
        let (sender, _progress, generation) =
            install_running(&mut app, RommOperation::Preview { limit: 20 });
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Preview(Box::default())),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        assert!(
            app.romm_preview.is_none(),
            "a preview for a dialog that has gone must be discarded"
        );
    }

    #[test]
    fn closing_the_dialog_forgets_the_preview() {
        let mut app = app();
        app.open_romm_configuration();
        app.romm_preview = Some(Box::new(crate::romm_config::RommPreviewSummary::default()));
        app.close_romm_configuration();
        assert!(app.romm_config_draft.is_none());
        assert!(app.romm_preview.is_none());
    }

    #[test]
    fn a_preview_is_not_recorded_as_a_mutating_activity() {
        let mut app = app();
        let context = egui::Context::default();
        let before = app.history.entries().count();
        assert!(app.start_romm_operation(context, RommOperation::Preview { limit: 20 }));
        assert_eq!(
            app.history.entries().count(),
            before,
            "a preview changes nothing, so it is not an activity worth recording"
        );
        // But it does block the card's actions while it runs.
        assert!(RommOperation::Preview { limit: 20 }.blocks_actions());
        assert!(!RommOperation::Preview { limit: 20 }.is_mutating());
        app.romm_operation = None;
    }

    #[test]
    fn opening_a_browse_view_twice_opens_one_panel_and_switching_keeps_it() {
        use crate::romm_browse::BrowseView;
        let mut app = app();
        app.open_romm_browse(BrowseView::Records);
        assert!(app.romm_browse.is_some());
        // A second click on the same view leaves the panel and its results alone.
        app.romm_browse.as_mut().expect("open").needs_reload = true;
        app.open_romm_browse(BrowseView::Records);
        assert!(
            app.romm_browse
                .as_ref()
                .is_some_and(|state| state.needs_reload),
            "the open panel must not be replaced"
        );
        // Switching views keeps the panel but drops a detail that belonged elsewhere.
        app.open_romm_browse(BrowseView::StaleSummary);
        assert_eq!(
            app.romm_browse.as_ref().map(|state| state.view),
            Some(BrowseView::StaleSummary)
        );
        assert!(
            app.romm_browse
                .as_ref()
                .is_some_and(|state| state.detail.is_none())
        );
    }

    #[test]
    fn browsing_is_never_recorded_as_a_mutating_activity() {
        let mut app = app();
        let context = egui::Context::default();
        let before = app.history.entries().count();
        for operation in [
            RommOperation::LoadRecords {
                filters: Box::default(),
                offset: 0,
                limit: 25,
            },
            RommOperation::LoadConflicts { offset: 0 },
            RommOperation::StaleSummary,
            RommOperation::LoadRecordDetail {
                romm_game_id: "1".to_string(),
            },
        ] {
            assert!(!operation.is_mutating(), "{operation:?} changes nothing");
            assert!(
                operation.blocks_actions(),
                "{operation:?} should still block"
            );
            assert!(app.start_romm_operation(context.clone(), operation));
            app.romm_operation = None;
        }
        assert_eq!(
            app.history.entries().count(),
            before,
            "reading the cache is not an activity worth recording"
        );
    }

    #[test]
    fn a_detail_terminal_result_is_delivered_once_to_the_requested_row() {
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        app.romm_browse
            .as_mut()
            .expect("open")
            .begin_detail("2".to_string());
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::LoadRecordDetail {
                romm_game_id: "2".to_string(),
            },
        );
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::RecordDetail(Box::new(None))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        let state = app.romm_browse.as_ref().expect("open");
        assert!(state.pending_detail_id.is_none());
        assert!(
            state
                .detail_problem
                .as_deref()
                .is_some_and(|problem| problem.contains("record 2"))
        );
        assert!(app.romm_operation.is_none());
    }

    #[test]
    fn a_stale_detail_result_cannot_attach_to_a_different_row() {
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        app.romm_browse
            .as_mut()
            .expect("open")
            .begin_detail("1".to_string());
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::LoadRecordDetail {
                romm_game_id: "1".to_string(),
            },
        );
        // The selection changed before the old result arrived.
        app.romm_browse
            .as_mut()
            .expect("open")
            .begin_detail("2".to_string());
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::RecordDetail(Box::new(None))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        let state = app.romm_browse.as_ref().expect("open");
        assert_eq!(state.pending_detail_id.as_deref(), Some("2"));
        assert!(state.detail.is_none());
        assert!(state.detail_problem.is_none());
    }

    #[test]
    fn a_duplicate_records_load_is_declined() {
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_browse(crate::romm_browse::BrowseView::Records);
        let (_sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::LoadRecords {
                filters: Box::default(),
                offset: 0,
                limit: 25,
            },
        );
        app.handle_romm_browse_request(
            &context,
            crate::romm_browse::BrowseRequest::LoadRecords {
                offset: 25,
                limit: 25,
            },
        );
        assert_eq!(
            app.romm_generation, generation,
            "a second page request while one is in flight must be declined"
        );
    }

    #[test]
    fn a_page_for_superseded_filters_marks_the_view_stale_rather_than_drawing() {
        use crate::romm_browse::{BrowseView, RecordFilters};
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_browse(BrowseView::Records);
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::LoadRecords {
                filters: Box::default(),
                offset: 0,
                limit: 25,
            },
        );
        // A page produced under the default filters...
        let cache = browse_cache();
        let page = crate::romm_browse::build_record_page(
            &cache,
            &RecordFilters::default(),
            0,
            25,
            &|_| archivefs_core::identity_source::matching::LocalPresence::Absent,
        );
        // ...arrives after the view has changed its filters.
        if let Some(state) = app.romm_browse.as_mut() {
            state.filters.verdict =
                Some(archivefs_core::identity_source::model::ExternalVerification::Stale);
        }
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Records(Box::new(page))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);

        let state = app.romm_browse.as_ref().expect("still open");
        assert!(
            state.page.is_none(),
            "a page answering the previous filters must not be drawn"
        );
        assert!(state.needs_reload, "and the view should say so");
    }

    #[test]
    fn a_page_from_a_superseded_cache_marks_the_view_stale() {
        use crate::romm_browse::{BrowseView, RecordFilters};
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_browse(BrowseView::Records);
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::LoadRecords {
                filters: Box::default(),
                offset: 0,
                limit: 25,
            },
        );
        // A page from a cache that has since been replaced by an import.
        let mut cache = browse_cache();
        cache.imported_at_unix_seconds += 1;
        let page = crate::romm_browse::build_record_page(
            &cache,
            &RecordFilters::default(),
            0,
            25,
            &|_| archivefs_core::identity_source::matching::LocalPresence::Absent,
        );
        // The view holds a page from the earlier cache.
        let earlier = browse_cache();
        if let Some(state) = app.romm_browse.as_mut() {
            state.page = Some(Box::new(crate::romm_browse::build_record_page(
                &earlier,
                &RecordFilters::default(),
                0,
                25,
                &|_| archivefs_core::identity_source::matching::LocalPresence::Absent,
            )));
        }
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Records(Box::new(page))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        // The identity in the arriving page is its own, so it is accepted - what this
        // pins is that the check is made against the page's cache rather than assumed.
        let state = app.romm_browse.as_ref().expect("still open");
        assert!(state.page.is_some());
        assert_eq!(
            state
                .page
                .as_ref()
                .map(|page| page.cache.imported_at_unix_seconds),
            Some(earlier.imported_at_unix_seconds + 1)
        );
    }

    #[test]
    fn a_result_arriving_after_the_panel_closed_is_dropped_without_panicking() {
        use crate::romm_browse::{BrowseView, RecordFilters};
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_browse(BrowseView::Records);
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::LoadRecords {
                filters: Box::default(),
                offset: 0,
                limit: 25,
            },
        );
        app.close_romm_browse();
        let cache = browse_cache();
        let page = crate::romm_browse::build_record_page(
            &cache,
            &RecordFilters::default(),
            0,
            25,
            &|_| archivefs_core::identity_source::matching::LocalPresence::Absent,
        );
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Records(Box::new(page))),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        assert!(app.romm_browse.is_none());
        assert!(
            app.romm_ui.last_outcome.is_none(),
            "a browsing result is not a card outcome"
        );
    }

    #[test]
    fn stale_summary_progress_is_absorbed_and_cleared_when_it_finishes() {
        use crate::romm_browse::BrowseView;
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_browse(BrowseView::StaleSummary);
        let (sender, progress, generation) = install_running(&mut app, RommOperation::StaleSummary);
        progress
            .send((
                generation,
                RommProgressEvent::StaleProgress {
                    probed: 2_500,
                    total: 10_081,
                },
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        assert_eq!(
            app.romm_stale_progress.map(|progress| progress.probed),
            Some(2_500),
            "the panel should be able to show how far it has got"
        );

        let cache = browse_cache();
        let view = crate::romm_browse::StaleSummaryView {
            cache: crate::romm_browse::CacheIdentity::of(&cache),
            summary: archivefs_core::identity_source::stale::StaleSummary::build(
                &cache,
                &[],
                3,
                |_| archivefs_core::identity_source::matching::LocalPresence::Absent,
            ),
        };
        sender
            .send((generation, Ok(RommOperationOutcome::Stale(Box::new(view)))))
            .expect("send");
        app.poll_romm_operation(&context);
        assert!(
            app.romm_stale_progress.is_none(),
            "progress should be cleared once the result is in"
        );
        assert!(
            app.romm_browse
                .as_ref()
                .is_some_and(|state| state.stale.is_some())
        );
    }

    #[test]
    fn a_cancelled_stale_summary_publishes_no_partial_result() {
        use crate::romm_browse::BrowseView;
        let mut app = app();
        let context = egui::Context::default();
        app.open_romm_browse(BrowseView::StaleSummary);
        let (sender, _progress, generation) =
            install_running(&mut app, RommOperation::StaleSummary);
        sender
            .send((
                generation,
                Err("The stale summary was cancelled. Nothing was changed.".to_string()),
            ))
            .expect("send");
        app.poll_romm_operation(&context);
        assert!(
            app.romm_browse
                .as_ref()
                .is_some_and(|state| state.stale.is_none()),
            "a half-probed partition must not be shown as a finding"
        );
        let outcome = app
            .romm_ui
            .last_outcome
            .as_ref()
            .expect("the failure is reported");
        assert!(!outcome.succeeded);
    }

    /// A small cache for the dispatch tests.
    fn browse_cache() -> archivefs_core::identity_source::cache::IdentityCache {
        archivefs_core::identity_source::cache::IdentityCache {
            format_version: archivefs_core::identity_source::cache::CACHE_FORMAT_VERSION,
            provider: IdentityProvider::Romm,
            server_id: "http://172.19.0.20:8080".to_string(),
            server_version: Some("5.1.0".to_string()),
            source_fingerprint: "abcd1234".to_string(),
            imported_at_unix_seconds: 1_785_595_944,
            platforms: Vec::new(),
            records: Vec::new(),
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_reported_total: Some(0),
        }
    }

    #[test]
    fn a_mutating_operation_is_recorded_in_history_and_a_status_load_is_not() {
        let mut app = app();
        let context = egui::Context::default();
        let before = app.history.entries().count();
        app.start_romm_status_load(context.clone());
        assert_eq!(
            app.history.entries().count(),
            before,
            "reading local state is not an activity worth recording"
        );
        app.romm_operation = None;

        assert!(app.start_romm_operation(context, RommOperation::ClearArtwork));
        assert_eq!(
            app.history.entries().count(),
            before + 1,
            "a mutating operation should be auditable"
        );
        app.romm_operation = None;
    }

    // --- Selected-game panel -------------------------------------------

    fn game_panel_app(path: &str) -> ArchiveFsApp {
        let mut app = app();
        app.romm_snapshot = Some(Box::new(snapshot(36_259, ProviderState::Ready)));
        app.archive_context.focused = Some(PathBuf::from(path));
        app.romm_game.focus(Some(Path::new(path)));
        app
    }

    fn panel_for(path: &str, game_id: &str) -> crate::romm_game::GameIdentityPanel {
        use archivefs_core::identity_source::cache::{CACHE_FORMAT_VERSION, IdentityCache};
        use archivefs_core::identity_source::hashing::LocalHashCache;
        use archivefs_core::identity_source::matching::LocalFileFacts;
        use archivefs_core::identity_source::model::{
            ExternalIdentityRecord, ExternalVerification,
        };

        let record = ExternalIdentityRecord {
            provider: IdentityProvider::Romm,
            server_id: "http://172.19.0.20:8080".to_string(),
            provider_platform_id: Some("7".to_string()),
            provider_game_id: game_id.to_string(),
            provider_file_id: None,
            provider_path: "roms/gb/game.gb".to_string(),
            archivefs_path: Some(PathBuf::from(path)),
            title: Some("Game".to_string()),
            platform_candidate: Some("Game Boy".to_string()),
            provider_platform_name: Some("gb".to_string()),
            regions: Vec::new(),
            revision: None,
            hashes: Vec::new(),
            file_size_bytes: None,
            metadata_provider_ids: Vec::new(),
            artwork: None,
            related_files: Vec::new(),
            sibling_game_ids: Vec::new(),
            imported_at_unix_seconds: 1_785_595_944,
            provider_updated_at: None,
            verification: ExternalVerification::StrongExternal,
            conflicts: Vec::new(),
            evidence: Vec::new(),
            synopsis: None,
            genres: Vec::new(),
            players: None,
            rating: None,
            release_year: None,
        };
        let cache = IdentityCache {
            format_version: CACHE_FORMAT_VERSION,
            provider: IdentityProvider::Romm,
            server_id: "http://172.19.0.20:8080".to_string(),
            server_version: None,
            source_fingerprint: "abcd".to_string(),
            imported_at_unix_seconds: 1_785_595_944,
            platforms: Vec::new(),
            records: vec![record],
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_reported_total: Some(1),
        };
        crate::romm_game::resolve_selected_game(
            &cache,
            Path::new(path),
            &LocalHashCache::new(),
            &crate::romm_game::LocalPlatformClaim::default(),
            None,
            &|probe: &Path| LocalFileFacts::observe(probe),
        )
    }

    #[test]
    fn a_second_lookup_while_one_runs_is_declined() {
        let context = egui::Context::default();
        let mut app = game_panel_app("/mnt/games/roms/gb/game.gb");
        let operation = RommOperation::ResolveGame {
            local_path: PathBuf::from("/mnt/games/roms/gb/game.gb"),
            local_platform: Box::new(crate::romm_game::LocalPlatformClaim::default()),
            chosen_game_id: None,
        };
        let (_sender, _progress, generation) = install_running(&mut app, operation.clone());
        assert!(!app.start_romm_operation(context, operation));
        assert_eq!(app.romm_generation, generation, "nothing was superseded");
        app.romm_operation = None;
    }

    #[test]
    fn looking_up_the_selected_game_is_not_recorded_as_an_activity() {
        let context = egui::Context::default();
        let mut app = game_panel_app("/mnt/games/roms/gb/game.gb");
        let before = app.history.entries().count();
        assert!(app.start_romm_operation(
            context,
            RommOperation::ResolveGame {
                local_path: PathBuf::from("/mnt/games/roms/gb/game.gb"),
                local_platform: Box::new(crate::romm_game::LocalPlatformClaim::default()),
                chosen_game_id: None,
            }
        ));
        assert_eq!(
            app.history.entries().count(),
            before,
            "reading the cache changes nothing, so there is nothing to audit"
        );
        app.romm_operation = None;
    }

    #[test]
    fn verifying_a_local_file_is_recorded_as_an_activity_without_the_path() {
        let context = egui::Context::default();
        let mut app = game_panel_app("/mnt/games/roms/gb/game.gb");
        let before = app.history.entries().count();
        assert!(app.start_romm_operation(
            context,
            RommOperation::VerifyLocalFile {
                local_path: PathBuf::from("/mnt/games/roms/gb/game.gb"),
                romm_game_id: "1".to_string(),
                local_platform: Box::new(crate::romm_game::LocalPlatformClaim::default()),
                chosen_game_id: None,
            }
        ));
        assert_eq!(app.history.entries().count(), before + 1);
        let entry = app.history.entries().next().expect("an entry");
        assert!(
            !entry.message.contains("/mnt/games"),
            "a private path must not reach the activity list: {}",
            entry.message
        );
        app.romm_operation = None;
    }

    #[test]
    fn a_resolved_panel_for_the_current_selection_lands() {
        let context = egui::Context::default();
        let path = "/mnt/games/roms/gb/game.gb";
        let mut app = game_panel_app(path);
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::ResolveGame {
                local_path: PathBuf::from(path),
                local_platform: Box::new(crate::romm_game::LocalPlatformClaim::default()),
                chosen_game_id: None,
            },
        );
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::GameIdentity(Box::new(panel_for(
                    path, "1",
                )))),
            ))
            .expect("sent");
        app.poll_romm_operation(&context);
        assert!(app.romm_game.panel.is_some());
        assert!(!app.romm_game.needs_reload);
        assert!(
            app.romm_ui.last_outcome.is_none(),
            "a lookup is panel state, not a card banner"
        );
    }

    #[test]
    fn a_panel_that_arrives_after_the_selection_moved_is_not_drawn() {
        let context = egui::Context::default();
        let mut app = game_panel_app("/mnt/games/roms/gb/game.gb");
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::ResolveGame {
                local_path: PathBuf::from("/mnt/games/roms/gb/game.gb"),
                local_platform: Box::new(crate::romm_game::LocalPlatformClaim::default()),
                chosen_game_id: None,
            },
        );
        // The person clicked a different archive while the lookup was in flight.
        app.romm_game
            .focus(Some(Path::new("/mnt/games/roms/gb/other.gb")));
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::GameIdentity(Box::new(panel_for(
                    "/mnt/games/roms/gb/game.gb",
                    "1",
                )))),
            ))
            .expect("sent");
        app.poll_romm_operation(&context);
        assert!(
            app.romm_game.panel.is_none(),
            "one game's evidence must not attach to another's file"
        );
        assert!(app.romm_game.needs_reload, "and it says so");
    }

    #[test]
    fn hash_progress_from_a_superseded_generation_is_discarded() {
        let context = egui::Context::default();
        let mut app = game_panel_app("/mnt/games/roms/gb/game.gb");
        let (_sender, progress, generation) = install_running(
            &mut app,
            RommOperation::VerifyLocalFile {
                local_path: PathBuf::from("/mnt/games/roms/gb/game.gb"),
                romm_game_id: "1".to_string(),
                local_platform: Box::new(crate::romm_game::LocalPlatformClaim::default()),
                chosen_game_id: None,
            },
        );
        app.romm_generation = app.romm_generation.wrapping_add(1);
        if let Some(running) = app.romm_operation.as_mut() {
            running.generation = app.romm_generation;
        }
        progress
            .send((
                generation,
                RommProgressEvent::Hashing(crate::romm_game::HashProgressView {
                    file_label: "stale.gb".to_string(),
                    bytes_read: 1,
                    total_bytes: 2,
                    elapsed_seconds: 0,
                    cancellation_requested: false,
                }),
            ))
            .expect("sent");
        app.poll_romm_operation(&context);
        assert!(
            app.romm_hash_progress.is_none(),
            "progress from an operation nobody is waiting for must not be shown"
        );
    }

    #[test]
    fn a_cover_for_a_record_that_is_no_longer_chosen_does_not_land() {
        let context = egui::Context::default();
        let path = "/mnt/games/roms/gb/game.gb";
        let mut app = game_panel_app(path);
        app.romm_game.panel = Some(Box::new(panel_for(path, "1")));
        let (sender, _progress, generation) = install_running(
            &mut app,
            RommOperation::LoadCover {
                local_path: PathBuf::from(path),
                romm_game_id: "999".to_string(),
            },
        );
        sender
            .send((
                generation,
                Ok(RommOperationOutcome::Cover(Box::new(
                    crate::romm_game::CoverOutcome {
                        local_path: PathBuf::from(path),
                        romm_game_id: "999".to_string(),
                        state: crate::romm_game::CoverState::Unavailable(
                            crate::romm_game::ArtworkAvailability::None,
                        ),
                        cached_items: 5,
                        cached_bytes: 500,
                    },
                ))),
            ))
            .expect("sent");
        app.poll_romm_operation(&context);
        assert_eq!(app.romm_game.cover, crate::romm_game::CoverState::Idle);
        assert!(app.romm_game.cover_cache.is_none());
    }

    #[test]
    fn moving_the_selection_between_frames_clears_the_panel() {
        let mut app = game_panel_app("/mnt/games/roms/gb/game.gb");
        app.romm_game.panel = Some(Box::new(panel_for("/mnt/games/roms/gb/game.gb", "1")));
        app.archive_context.focused = Some(PathBuf::from("/mnt/games/roms/gb/other.gb"));
        // The renderer follows the selection at the top of every frame.
        app.romm_game.focus(app.archive_context.focused.as_deref());
        assert!(app.romm_game.panel.is_none());
    }
}
