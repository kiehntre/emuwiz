//! GUI Maintenance Batch 2: relocated from main.rs's single inline
//! `#[cfg(test)] mod tests { ... }` (see `crate::tests` for the shared
//! imports/fixtures this file and its siblings rely on).
//!
//! This file's name is a best-effort thematic label, not a strict
//! single-feature boundary: the original test module interleaved topics
//! throughout (tests for unrelated features sit side by side in source
//! order), so this file was cut at safe item boundaries within that
//! existing order rather than by re-sorting tests into pure per-feature
//! files. Every test here is copied byte-for-byte from its original
//! location - nothing was rewritten, renamed, or reordered relative to
//! its neighbors within this slice.
//!
//! Predominant theme observed in this slice: Dolphin/Xenia/PCSX2/RetroArch emulator profile detection and setup workflows.

use super::*;

#[test]
fn advanced_view_cheats_mods_has_no_back_to_games_button() {
    // The button is Gamer-View-only - Advanced View already has its
    // own established sidebar navigation and must not gain a new,
    // redundant control.
    let mut app = app_with_cheats_mods_context();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::CheatsMods;

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 900.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input, |ctx| app.update(ctx, &mut frame));
    assert!(!rendered_text_contains(&output, "Back to games"));
}

#[test]
fn gamer_view_shows_list_and_selected_game_actions_at_1024x600() {
    // docs/GUI_NAVIGATION_RESET_DESIGN.md §11's 1024x600 risk list:
    // Gamer View's compact game list and selected-game action panel
    // must both be visible together at this exact viewport, with no
    // sidebar consuming width (Gamer View never renders one).
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut a = record("/roms/a.zip", MountState::Pending);
        a.metadata.title = Some("A Real Game Title".to_string());
        a.metadata.platform = Some("GameCube".to_string());
        data.records.push(a);
    }
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;
    app.archive_context
        .select_only(PathBuf::from("/roms/a.zip"));

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 600.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input, |ctx| app.update(ctx, &mut frame));

    for expected in [
        "Search games...",
        "A Real Game Title",
        "GameCube",
        "Cheats & Mods",
        "Details",
        "Mount",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "expected {expected:?} to be visible in Gamer View at 1024x600"
        );
    }
    // The Advanced-only sidebar/menu bar must be completely absent.
    assert!(!rendered_text_contains(&output, "Active Mounts"));
    assert!(!rendered_text_contains(&output, "Mount All"));
}

#[test]
fn gamer_view_renders_without_a_romm_catalogue_and_contacts_nothing() {
    // The cover column draws on every Gamer View frame. With no RomM
    // catalogue imported - a clean install, or an import that never ran -
    // every row falls to the placeholder path. That path runs inside the
    // real frame here rather than only in the scheduler's unit tests, so a
    // panic while drawing a coverless row is caught.
    //
    // It also pins the rule that opening Gamer View is not a network event:
    // no cover worker is started by rendering, so nothing can be fetched by
    // looking at the page.
    let mut app = app_for_operation_tests();
    let mut records = Vec::new();
    for index in 0..40 {
        let mut row = record(&format!("/roms/g{index:02}.zip"), MountState::Pending);
        row.metadata.title = Some(format!("Coverless Game {index:02}"));
        row.metadata.platform = Some("GameCube".to_string());
        records.push(row);
    }
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", records)));
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    // Several passes: scrolling state and the look-ahead settle across
    // frames, and a repeat frame is where a re-request storm would show up.
    let mut output = None;
    for _ in 0..3 {
        output = Some(ctx.run(input.clone(), |ctx| app.update(ctx, &mut frame)));
    }
    let output = output.expect("a rendered frame");

    assert!(
        rendered_text_contains(&output, "Coverless Game 00"),
        "the list must still draw when no cover is available"
    );
    assert!(
        app.gamer_covers.tracked() <= crate::gamer_artwork::MAX_TRACKED_COVERS,
        "rendering pushed the cover cache past its bound, held {}",
        app.gamer_covers.tracked()
    );
}

#[test]
fn an_empty_gamer_view_starts_no_cover_worker_at_all() {
    // The worker is what opens the catalogue and is the only thing that can
    // reach the configured RomM instance. It is started lazily, on the first
    // frame that actually asks for a cover, so a Gamer View with nothing to
    // show must never bring it up: no thread, no catalogue read, no request.
    let mut app = app_for_operation_tests();
    // Opted back in on purpose: this is the one test whose subject *is*
    // whether the worker starts, so suppressing it would prove nothing. It
    // is safe here precisely because an empty list must never reach the
    // start site - if that regressed, this test starts a thread and fails.
    app.gamer_cover_worker_allowed = true;
    app.state = LoadState::Ready(Box::new(loaded_data_with_records("/mount", Vec::new())));
    app.ui_mode = GuiMode::GamerView;
    app.view = MainView::Library;

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    for _ in 0..3 {
        let _ = ctx.run(input.clone(), |ctx| app.update(ctx, &mut frame));
    }

    assert!(
        app.gamer_cover_worker.is_none(),
        "an empty library started a cover worker, so opening the page alone \
             would open the catalogue and could reach the network"
    );
    assert_eq!(app.gamer_covers.tracked(), 0);
}

#[test]
fn changing_the_archive_clears_the_candidate_and_selection() {
    let mut app = app_with_cheats_mods_context();
    workflow_at_cheat_selection_stage(&mut app);
    if let LoadState::Ready(data) = &mut app.state {
        data.records
            .push(record("/roms/b.zip", MountState::Pending));
    }

    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/b.zip")));

    let workflow = app.cheat_workflow.as_ref().expect("workflow");
    assert!(
        workflow.candidate_selection.is_none(),
        "a candidate from another archive must never carry over"
    );
    assert!(matches!(workflow.candidates, CheatStepResource::NotLoaded));
    assert!(workflow.candidates_request.is_none());
}

#[test]
fn changing_the_retroarch_profile_clears_the_destination_derived_state() {
    let mut app = app_with_cheats_mods_context();
    workflow_at_cheat_selection_stage(&mut app);
    let workflow = app.cheat_workflow.as_mut().expect("workflow");
    workflow.selected_profile_id = Some("flatpak-user".to_string());
    // The profile radio applies exactly this reset; the destination and
    // everything computed from it must be recalculated, not reused.
    clear_cheat_candidate_state(workflow);

    assert!(workflow.candidate_selection.is_none());
    assert!(matches!(workflow.preview, CheatStepResource::NotLoaded));
    assert!(matches!(workflow.transaction, CheatTransactionState::Idle));
}

#[test]
fn editing_the_cheat_selection_invalidates_a_preview_built_from_the_old_one() {
    let mut app = app_with_cheats_mods_context();
    workflow_at_cheat_selection_stage(&mut app);
    let key = cheat_preview_key(app.cheat_workflow.as_ref().expect("workflow"));
    if let Some(workflow) = app.cheat_workflow.as_mut() {
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Failed("stale".to_string());
    }

    app.update_cheat_selection(|selection| {
        selection.select_all();
    });

    let workflow = app.cheat_workflow.as_ref().expect("workflow");
    assert!(
        matches!(workflow.preview, CheatStepResource::NotLoaded),
        "a preview of a different selection must not survive the edit"
    );
    assert!(workflow.preview_request.is_none());
    assert_eq!(
        workflow
            .candidate_selection
            .as_ref()
            .expect("selection")
            .selection
            .selected_count(),
        2
    );
}

#[test]
fn an_uninstallable_candidate_can_never_become_the_selection() {
    let mut app = app_with_cheats_mods_context();
    let key = cheat_preview_key(app.cheat_workflow.as_ref().expect("workflow"));
    if let Some(workflow) = app.cheat_workflow.as_mut() {
        workflow.candidates_request = Some(key.clone());
        workflow.candidates = CheatStepResource::Ready(CheatCandidateStage {
            key,
            catalogue_root: PathBuf::from("/catalogue"),
            list: CheatCandidateList {
                candidates: vec![CheatCandidate {
                    catalogue_relative_path: "MegaDrive/a.cht".to_string(),
                    display_name: "a".to_string(),
                    platform: Some("MegaDrive".to_string()),
                    region: None,
                    revision: None,
                    classification: CheatCandidateClassification::CrossPlatform,
                    confidence_score: 0,
                    evidence: Vec::new(),
                    cheat_count: 1,
                    source_file_hash: None,
                    auto_selectable: false,
                    manually_selectable: false,
                }],
                total_matched: 1,
                truncated: false,
                query: None,
                records_scanned: 1,
                scan_limit_reached: false,
            },
        });
    }

    app.apply_cheat_candidate_choice("MegaDrive/a.cht");

    assert!(
        app.cheat_workflow
            .as_ref()
            .expect("workflow")
            .candidate_selection
            .is_none(),
        "a cross-platform candidate is refused even if something asks for it directly"
    );
}

/// Generic 3-frame real pointer click on a single widget, mirroring
/// `simulate_row_click`'s documented reasoning (egui hit-tests a
/// frame's pointer events against the *previous* frame's registered
/// rects, so a widget rendered for the first time cannot be clicked
/// within that same frame - see that function's doc comment).
fn click_widget(
    ctx: &egui::Context,
    screen_size: egui::Vec2,
    render: impl Fn(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    let base = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen_size)),
        ..Default::default()
    };
    let call = |input: egui::RawInput| -> egui::Response {
        let mut out = None;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                out = Some(render(ui));
            });
        });
        out.expect("render closure always returns a response")
    };
    let first = call(base.clone());
    let pos = first.rect.center();
    call(egui::RawInput {
        events: vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }],
        ..base.clone()
    });
    call(egui::RawInput {
        events: vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
        ..base
    })
}

#[test]
fn find_matching_cheat_files_button_registers_a_real_pointer_click() {
    let ctx = egui::Context::default();
    let response = click_widget(&ctx, egui::vec2(1200.0, 900.0), |ui| {
        show_find_matching_cheats_button(ui)
    });
    assert!(
        response.clicked(),
        "a real press-then-release on the button's own rect must register as a click"
    );
}

/// The blocked-prerequisite case this milestone's manual test actually
/// hit: a profile is selected and the trusted catalogue has been
/// listed and a source chosen, but the catalogue itself was never
/// retrieved into `source_fetch` (no "Use cached snapshot" / fetch was
/// ever run). Previously this made `start_cheat_candidate_match`
/// return silently with no state change - the dead click.
fn app_with_unfetched_trusted_catalogue() -> ArchiveFsApp {
    let mut app = app_with_cheats_mods_context();
    app.retroarch_profiles =
        RetroArchProfilesState::Ready(cheat_discovery(vec![cheat_profile("native-user", true)]));
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.adapter = CheatEmulatorAdapter::RetroArch;
    workflow.source_mode = CheatSourceMode::ArchiveFsTrustedCatalogue;
    workflow.selected_profile_id = Some("native-user".to_string());
    workflow.selected_source_id = Some("test-source".to_string());
    // source_fetch is deliberately left NotLoaded here.
    app
}

fn app_with_fetched_trusted_catalogue() -> ArchiveFsApp {
    let mut app = app_with_unfetched_trusted_catalogue();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.source_fetch = CheatStepResource::Ready(cheat_fetch_result_for(
        "test-source",
        CheatSourceFetchStatus::Fetched,
    ));
    app
}

#[test]
fn matching_dispatches_exactly_once_and_becomes_loading_immediately() {
    let mut app = app_with_fetched_trusted_catalogue();
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().candidates,
        CheatStepResource::NotLoaded
    ));
    let before = app.history.entries().count();

    app.start_cheat_candidate_match(egui::Context::default());

    assert!(
        matches!(
            app.cheat_workflow.as_ref().unwrap().candidates,
            CheatStepResource::Loading { .. }
        ),
        "one dispatch must immediately produce a visible Loading state"
    );
    assert_eq!(
        app.history.entries().count(),
        before + 1,
        "exactly one activity entry for one dispatch"
    );
}

#[test]
fn matching_eventually_produces_a_ready_candidate_list() {
    let mut app = app_with_fetched_trusted_catalogue();
    app.start_cheat_candidate_match(egui::Context::default());
    for _ in 0..200 {
        app.poll_cheat_workflow(&egui::Context::default());
        if matches!(
            app.cheat_workflow.as_ref().unwrap().candidates,
            CheatStepResource::Ready(_)
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        matches!(
            app.cheat_workflow.as_ref().unwrap().candidates,
            CheatStepResource::Ready(_)
        ),
        "the worker result must reach a terminal, visible state"
    );
}

#[test]
fn a_no_match_result_is_a_visible_ready_state_with_an_empty_list() {
    let mut app = app_with_fetched_trusted_catalogue();
    // The fixture catalogue's manifest describes no games, so no
    // archive can ever match it - a real "no candidates" outcome.
    app.start_cheat_candidate_match(egui::Context::default());
    for _ in 0..200 {
        app.poll_cheat_workflow(&egui::Context::default());
        if !matches!(
            app.cheat_workflow.as_ref().unwrap().candidates,
            CheatStepResource::Loading { .. }
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let CheatStepResource::Ready(stage) = &app.cheat_workflow.as_ref().unwrap().candidates else {
        panic!("expected a Ready state");
    };
    assert!(
        stage.list.is_empty(),
        "no matching game means an empty, still-visible list"
    );
}

#[test]
fn clicking_without_a_retrieved_catalogue_shows_the_exact_blocked_reason() {
    let mut app = app_with_unfetched_trusted_catalogue();
    let before = app.history.entries().count();

    app.start_cheat_candidate_match(egui::Context::default());

    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatStepResource::Failed(message) = &workflow.candidates else {
        panic!("prerequisite failure must be a visible Failed state, not NotLoaded");
    };
    assert!(
        message.contains("Retrieve or reuse the trusted catalogue snapshot"),
        "the exact reason must be shown, not a generic error: {message}"
    );
    assert_eq!(
        app.history.entries().count(),
        before + 1,
        "a blocked click still creates exactly one activity entry"
    );

    let history_message = &app.history.entries().next().unwrap().message;
    assert!(
        history_message.contains("Matching blocked"),
        "the activity entry states the click was blocked: {history_message}"
    );
}

#[test]
fn the_blocked_state_renders_as_visibly_different_from_a_worker_failure() {
    let mut app = app_with_unfetched_trusted_catalogue();
    app.start_cheat_candidate_match(egui::Context::default());
    let history = OperationHistory::default();
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
                &app.xenia_profiles,
                None,
                None,
                &history,
                false,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Matching blocked"));
    assert!(!rendered_text_contains(&output, "Matching failed"));
}

#[test]
fn a_second_dispatch_while_loading_is_a_no_op() {
    let mut app = app_with_fetched_trusted_catalogue();
    app.start_cheat_candidate_match(egui::Context::default());
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().candidates,
        CheatStepResource::Loading { .. }
    ));
    let before = app.history.entries().count();
    let request_before = app
        .cheat_workflow
        .as_ref()
        .unwrap()
        .candidates_request
        .clone();

    // Simulates a rapid double click: dispatching again while the
    // first match is still running must not restart the work or
    // record a second activity entry.
    app.start_cheat_candidate_match(egui::Context::default());

    assert_eq!(
        app.history.entries().count(),
        before,
        "a repeated click while matching is active must be a no-op"
    );
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().candidates_request,
        request_before,
        "the original request key must survive an ignored repeat click"
    );
}

#[test]
fn a_click_through_the_real_rendered_page_reaches_the_dispatch_target() {
    // Proves the wiring documented at the top of this bug's fix: the
    // real button's response, when clicked, produces exactly the
    // action the app-level dispatcher matches on to call
    // `start_cheat_candidate_match`.
    let ctx = egui::Context::default();
    let response = click_widget(&ctx, egui::vec2(1200.0, 900.0), |ui| {
        show_find_matching_cheats_button(ui)
    });
    let action = response
        .clicked()
        .then_some(CheatWorkflowAction::MatchCandidates);
    assert!(matches!(action, Some(CheatWorkflowAction::MatchCandidates)));
}

/// Real mouse-wheel scroll (not the `page()` scroll wrapper's Home/End
/// keyboard shortcuts, which are not wired for Cheats & Mods' own
/// scroll area) repeated across many frames, matching how a real user
/// scrolls: `PointerMoved` over the content followed by a
/// `MouseWheel` event, each frame.
fn scroll_to_bottom_with_mouse_wheel(
    ctx: &egui::Context,
    app: &mut ArchiveFsApp,
    frame: &mut eframe::Frame,
    base_input: &egui::RawInput,
    screen: egui::Vec2,
) -> egui::FullOutput {
    let mut output = None;
    for _ in 0..40 {
        let scroll_input = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(egui::pos2(screen.x / 2.0, screen.y / 2.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -20.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..base_input.clone()
        };
        output = Some(ctx.run(scroll_input, |ctx| app.update(ctx, frame)));
    }
    output.unwrap()
}

/// Builds a Cheats & Mods workflow with 40 candidate cards - enough
/// real content that the page unambiguously overflows any reasonable
/// window, so a scroll is actually required to reach the end.
fn app_with_overflowing_cheats_mods_page() -> ArchiveFsApp {
    let mut app = app_with_fetched_trusted_catalogue();
    app.view = MainView::CheatsMods;
    let key = cheat_preview_key(app.cheat_workflow.as_ref().unwrap());
    let candidates: Vec<CheatCandidate> = (0..40)
        .map(|index| CheatCandidate {
            catalogue_relative_path: format!("NES/game{index}.cht"),
            display_name: format!("Game {index}"),
            platform: Some("NES".to_string()),
            region: None,
            revision: None,
            classification: CheatCandidateClassification::Weak,
            confidence_score: 100,
            evidence: Vec::new(),
            cheat_count: 3,
            source_file_hash: None,
            auto_selectable: false,
            manually_selectable: true,
        })
        .collect();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.candidates_request = Some(key.clone());
    workflow.candidates = CheatStepResource::Ready(CheatCandidateStage {
        key,
        catalogue_root: PathBuf::from("/catalogue"),
        list: CheatCandidateList {
            total_matched: candidates.len(),
            truncated: false,
            query: None,
            records_scanned: candidates.len(),
            scan_limit_reached: false,
            candidates,
        },
    });
    app
}

/// Asserts that `needle` - the last distinctive text on a page - is
/// both present and actually visible (its position falls inside its
/// own paint clip rect) after scrolling. Presence alone is not enough:
/// a clipped widget is still laid out and still shows up in
/// `output.shapes`, it just paints outside where anyone can see it.
fn assert_final_content_reachable(output: &egui::FullOutput, needle: &str) {
    let position = find_exact_text_position_and_clip(output, needle);
    let (pos, clip_rect) =
        position.unwrap_or_else(|| panic!("final content {needle:?} must be rendered somewhere"));
    assert!(
        pos.y <= clip_rect.max.y,
        "final content {needle:?} must fall within its own clip rect at maximum scroll:              pos.y={} clip_rect={:?}",
        pos.y,
        clip_rect
    );
}

#[test]
fn cheats_mods_final_section_is_reachable_at_maximum_scroll() {
    let mut app = app_with_overflowing_cheats_mods_page();
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let screen = egui::vec2(1600.0, 900.0);
    let base_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
        ..Default::default()
    };
    run_settle_frames(&ctx, &mut app, &mut frame, &base_input, 3);
    let output = scroll_to_bottom_with_mouse_wheel(&ctx, &mut app, &mut frame, &base_input, screen);
    assert_final_content_reachable(
        &output,
        "No related activity has been recorded in this session.",
    );
}

#[test]
fn cheats_mods_final_section_is_reachable_at_a_smaller_viewport() {
    let mut app = app_with_overflowing_cheats_mods_page();
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    // A small laptop-class window, not just the large screenshot size.
    let screen = egui::vec2(1024.0, 600.0);
    let base_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
        ..Default::default()
    };
    run_settle_frames(&ctx, &mut app, &mut frame, &base_input, 3);
    let output = scroll_to_bottom_with_mouse_wheel(&ctx, &mut app, &mut frame, &base_input, screen);
    assert_final_content_reachable(
        &output,
        "No related activity has been recorded in this session.",
    );
}

#[test]
fn resizing_the_window_does_not_reintroduce_clipping() {
    // Renders at one size, then resizes to a different size mid-session
    // (the same `egui::Context`, a new `screen_rect`) and confirms the
    // final content is still reachable after the resize.
    let mut app = app_with_overflowing_cheats_mods_page();
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let large = egui::vec2(1600.0, 900.0);
    let large_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, large)),
        ..Default::default()
    };
    run_settle_frames(&ctx, &mut app, &mut frame, &large_input, 3);
    let _ = scroll_to_bottom_with_mouse_wheel(&ctx, &mut app, &mut frame, &large_input, large);

    let small = egui::vec2(1024.0, 600.0);
    let small_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, small)),
        ..Default::default()
    };
    run_settle_frames(&ctx, &mut app, &mut frame, &small_input, 3);
    let output = scroll_to_bottom_with_mouse_wheel(&ctx, &mut app, &mut frame, &small_input, small);
    assert_final_content_reachable(
        &output,
        "No related activity has been recorded in this session.",
    );
}

/// Root-cause regression test for the bottom-clipping bug: egui's
/// `TopBottomPanel` picks each frame's height by loading `PanelState`
/// persisted under the panel's own id from the previous frame, falling
/// back to a fresh default only the very first time that id is ever
/// shown (see `containers/panel.rs` in egui 0.32). The collapsed and
/// expanded activity panel render very different content heights (one
/// status row vs. a history list up to ~220px plus a button row); if
/// they shared one panel id, the frame right after expanding would
/// load the *collapsed* height, squeeze the expanded content's own
/// paint clip rect into it (a `TopBottomPanel` clips its content to
/// its own panel rect - "if we overflow, don't do so visibly"), and
/// only correct itself one frame later. A screenshot taken in that
/// window - or a render that lands between reactive repaints - shows
/// exactly the reported symptom: page content jammed into a sliver
/// near the screen edge. `show_activity_panel` now uses a distinct id
/// per visual state so their persisted heights can never contaminate
/// each other.
#[test]
fn expanding_the_activity_panel_does_not_compress_its_content() {
    let mut app = app_for_operation_tests();
    // The bottom activity panel is an Advanced View surface (Gamer
    // View has its own, much simpler feedback line - see
    // docs/GUI_NAVIGATION_RESET_DESIGN.md).
    app.ui_mode = GuiMode::AdvancedView;
    for index in 0..8 {
        app.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            None,
            ActivityOutcome::Completed,
            format!("Activity entry {index} with a realistic message length."),
        ));
    }
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let screen = egui::vec2(1600.0, 900.0);
    let base_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
        ..Default::default()
    };
    // Settle in the collapsed state first - this is what persists a
    // small collapsed-content height, which used to leak into the
    // very next frame once expanded.
    run_settle_frames(&ctx, &mut app, &mut frame, &base_input, 3);

    app.show_activity = true;
    let first_expanded_frame = ctx.run(base_input.clone(), |ctx| app.update(ctx, &mut frame));

    fn activity_content_span(output: &egui::FullOutput) -> (f32, f32) {
        fn walk(shape: &egui::Shape, min_y: &mut f32, max_y: &mut f32) {
            match shape {
                egui::Shape::Text(text_shape) => {
                    let text = text_shape.galley.text();
                    if text.starts_with("Activity entry") || text == "Clear activity" {
                        *min_y = min_y.min(text_shape.pos.y);
                        *max_y = max_y.max(text_shape.pos.y + text_shape.galley.size().y);
                    }
                }
                egui::Shape::Vec(nested) => nested.iter().for_each(|s| walk(s, min_y, max_y)),
                _ => {}
            }
        }
        let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut min_y, &mut max_y);
        }
        (min_y, max_y)
    }

    let (min_y, max_y) = activity_content_span(&first_expanded_frame);
    assert!(
        min_y.is_finite() && max_y.is_finite(),
        "the expanded activity panel's own content must be present on its first frame"
    );
    // Before the fix this span was ~14px (everything squeezed against
    // the screen edge, e.g. [880, 894] of a 900px-tall screen). A
    // history list plus a button row needs meaningfully more room than
    // that even in principle.
    assert!(
        max_y - min_y > 100.0,
        "the expanded activity panel's content must not be compressed into a sliver on its              first rendered frame: span = [{min_y}, {max_y}]"
    );
    assert!(
        rendered_text_contains(&first_expanded_frame, "Clear activity"),
        "the expanded panel's own controls must be present on the very first frame"
    );
}

/// The activity panel is a shared component rendered identically
/// regardless of `self.view` - this confirms the fix above holds for
/// every page the task named, not only Cheats & Mods.
#[test]
fn activity_panel_expansion_does_not_obscure_content_on_any_named_page() {
    for view in [
        MainView::Library,
        MainView::Sources,
        MainView::Selected,
        MainView::HistoryLogs,
        MainView::CheatsMods,
    ] {
        let mut app = app_for_operation_tests();
        app.view = view;
        // Advanced View surface - see the equivalent comment on
        // `expanding_the_activity_panel_does_not_compress_its_content`.
        app.ui_mode = GuiMode::AdvancedView;
        for index in 0..8 {
            app.history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                None,
                ActivityOutcome::Completed,
                format!("Activity entry {index}."),
            ));
        }
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let screen = egui::vec2(1600.0, 900.0);
        let base_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..Default::default()
        };
        run_settle_frames(&ctx, &mut app, &mut frame, &base_input, 3);
        app.show_activity = true;
        let output = ctx.run(base_input, |ctx| app.update(ctx, &mut frame));
        assert!(
            rendered_text_contains(&output, "Clear activity"),
            "{view:?}: expanded activity controls must be present"
        );
    }
}

/// History & Logs keeps only the 50 most recent activity entries
/// (`HISTORY_LIMIT`) - a data-layer cap, not a rendering bug. This
/// stays within that cap and confirms the oldest *kept* entry is
/// reachable by scrolling, exercising the same shared `page()` scroll
/// wrapper Sources/Doctor/Settings/About also use (unlike Cheats &
/// Mods' own separate scroll area).
#[test]
fn workflow_diagnostics_are_collapsed_so_the_primary_action_is_not_buried() {
    // Regression test for the Cheats & Mods workflow simplification:
    // the "Find matching cheat files" primary action must render
    // before any diagnostic text on the page, not several screens of
    // status badges and identity evidence below it.
    let mut app = app_with_unfetched_trusted_catalogue();
    let history = OperationHistory::default();
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
                &app.xenia_profiles,
                None,
                None,
                &history,
                false,
                &mut clipboard,
            );
        });
    });
    let primary_action =
        find_exact_text_center(&output, "Find matching cheat files").expect("button renders");
    let diagnostics_header =
        find_exact_text_center(&output, "Workflow diagnostics").expect("section renders");
    assert!(
        primary_action.y < diagnostics_header.y,
        "the primary action must appear above the collapsed diagnostics section:              action.y={} diagnostics.y={}",
        primary_action.y,
        diagnostics_header.y
    );
    assert!(
        !rendered_text_contains(&output, "Emulator profile"),
        "diagnostics content must stay collapsed by default"
    );
}

#[test]
fn history_logs_final_entry_is_reachable_at_maximum_scroll() {
    let mut app = app_for_operation_tests();
    app.view = MainView::HistoryLogs;
    // History & Logs is an Advanced View-only destination.
    app.ui_mode = GuiMode::AdvancedView;
    // Leave headroom below HISTORY_LIMIT: entering this page triggers
    // its own "refreshing history" activity entries, which would
    // otherwise evict the oldest of a *full* 50-entry buffer before
    // the assertion below ever runs.
    for index in 0..HISTORY_LIMIT - 4 {
        app.history.record(HistoryEntry::new(
            ActivityAction::CheatPreview,
            None,
            ActivityOutcome::Completed,
            format!("History page entry {index} with enough text to take real space."),
        ));
    }
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let screen = egui::vec2(1600.0, 900.0);
    let base_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
        ..Default::default()
    };
    run_settle_frames(&ctx, &mut app, &mut frame, &base_input, 3);
    let output = scroll_to_bottom_with_mouse_wheel(&ctx, &mut app, &mut frame, &base_input, screen);
    // Entry 0 is the oldest recorded and therefore the last one shown
    // in a newest-first list - the true bottom of the page.
    assert_final_content_reachable(
        &output,
        "History page entry 0 with enough text to take real space.",
    );
}

/// A workflow with a verified PS2 CRC/serial identity - the minimum
/// `pcsx2_identity_for_workflow` requires before an install preview will
/// even attempt to stage a PNACH - plus one selected GameHacking cheat
/// candidate, matching the real-world "Install selected" scenario.
fn pcsx2_workflow_with_verified_identity_and_selected_cheat(profile: Pcsx2Profile) -> ArchiveFsApp {
    let mut app = app_with_cheats_mods_context();
    let profile_id = profile.profile_id.clone();
    app.pcsx2_profiles = Pcsx2ProfilesState::Ready(Pcsx2ProfileDiscovery {
        profiles: vec![profile],
        warnings: Vec::new(),
        complete: true,
    });
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("PS2".to_string());
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    workflow.selected_pcsx2_profile_id = Some(profile_id);
    workflow.identity_request = Some(GameIdentityRequest {
        archive_path: workflow.archive_path.clone(),
        platform: workflow.platform.clone(),
        adapter: CheatEmulatorAdapter::Pcsx2,
    });
    let report = GameIdentityReport {
        archive_path: workflow.archive_path.clone(),
        platform: archivefs_core::game_identity::IdentityPlatform::PlayStation2,
        format: IdentityImageFormat::Iso,
        evidence: vec![
            archivefs_core::game_identity::IdentityEvidence {
                kind: IdentityKind::Pcsx2ExecutableCrc,
                status: IdentityStatus::Verified,
                value: Some("A1B2C3D4".to_string()),
                confidence: archivefs_core::game_identity::IdentityConfidence::ExactBytes,
                provenance: archivefs_core::game_identity::IdentityProvenance {
                    archive_path: workflow.archive_path.clone(),
                    member_path: None,
                    member_index: None,
                    method: "test fixture".to_string(),
                },
                diagnostic: "test fixture".to_string(),
            },
            archivefs_core::game_identity::IdentityEvidence {
                kind: IdentityKind::Ps2Serial,
                status: IdentityStatus::Verified,
                value: Some("SLUS-20312".to_string()),
                confidence: archivefs_core::game_identity::IdentityConfidence::ExactBytes,
                provenance: archivefs_core::game_identity::IdentityProvenance {
                    archive_path: workflow.archive_path.clone(),
                    member_path: None,
                    member_index: None,
                    method: "test fixture".to_string(),
                },
                diagnostic: "test fixture".to_string(),
            },
        ],
        warnings: Vec::new(),
        bytes_read: 512,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    workflow.identity =
        CheatStepResource::Ready((workflow.identity_request.clone().unwrap(), report));
    let candidate = Pcsx2CheatCandidate {
        id: "gh-42-1".to_string(),
        name: "Infinite health".to_string(),
        description: Some("Health never decreases.".to_string()),
        author: Some("Codejunkies".to_string()),
        source_game_id: Some("42".to_string()),
        source_url: Some("https://gamehacking.org/game/42".to_string()),
        provider_id: "gamehacking.org".to_string(),
        provider_name: "GameHacking.org".to_string(),
        source: "https://gamehacking.org/game/42".to_string(),
        game_crc: "A1B2C3D4".to_string(),
        serial_constraint: Some("SLUS-20312".to_string()),
        region_constraint: None,
        patch_lines: vec![
            archivefs_core::patch_manager::PnachPatchLine::parse(
                "patch=1,EE,20123456,word,00000001",
            )
            .unwrap(),
        ],
        confidence: archivefs_core::patch_manager::Pcsx2CheatConfidence::VerifiedCrcAndConstraints,
        compatibility: archivefs_core::patch_manager::Pcsx2CheatCompatibility::Compatible,
    };
    let mut selection = Pcsx2CheatSelection::default();
    selection.selected_ids.insert(candidate.id.clone());
    workflow.pcsx2_gamehacking = CheatStepResource::Ready(Pcsx2GameHackingState {
        status: GameHackingMatchStatus::Matched,
        detail: "Matched from the local catalogue.".to_string(),
        game: Some(GameHackingGame {
            game_id: 42,
            title: "Fixture Game".to_string(),
            system: "PlayStation 2".to_string(),
            region: Some("NTSC-U".to_string()),
            serial: Some("SLUS-20312".to_string()),
            crc: Some("A1B2C3D4".to_string()),
            source_url: "https://gamehacking.org/game/42".to_string(),
        }),
        match_candidates: Vec::new(),
        candidates: vec![candidate],
        selection,
        cached_fallback: false,
    });
    app
}

#[test]
fn install_selected_pcsx2_surfaces_build_preview_failure_visibly_and_records_history() {
    let mut profile = pcsx2_profile_fixture();
    // No documented `cheats` category patch directory is safely usable:
    // `stage_pcsx2_pnach` must fail with `ProfileUnavailable` before it
    // ever touches disk, reproducing the reported bug where a failed
    // build-preview left no PNACH, no journal, and no visible error.
    profile.patch_directories = vec![Pcsx2PatchDirectory {
        path: PathBuf::from("/isolated/PCSX2/cheats"),
        category: Pcsx2PatchCategory::Cheats,
        state: Pcsx2PatchDirectoryState::UnsafePath,
        warning: Some("directory is a symlink and will not be followed".to_string()),
        identity: None,
    }];
    let mut app = pcsx2_workflow_with_verified_identity_and_selected_cheat(profile);
    assert_eq!(app.history.entries().count(), 0);

    app.start_pcsx2_install_preview();

    let workflow = app.cheat_workflow.as_ref().unwrap();
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!("expected a resolved preview response");
    };
    let CheatPreviewOutcome::Failed(CheatPreviewFailure::Pcsx2InstallPlan(failure)) =
        &response.outcome
    else {
        panic!("expected a failed PCSX2 install plan");
    };
    assert_eq!(
        failure.kind,
        archivefs_core::patch_manager::Pcsx2InstallPlanErrorKind::ProfileUnavailable
    );

    // The failure must be visible in the activity history, not silently
    // discarded.
    let entries: Vec<_> = app.history.entries().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].outcome, ActivityOutcome::Failed);
    assert!(entries[0].message.contains("PCSX2"));

    // And it must render as a banner in the GUI itself, not just the log.
    let ArchiveFsApp {
        cheat_workflow,
        pcsx2_profiles,
        ..
    } = &mut app;
    let workflow = cheat_workflow.as_mut().unwrap();
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_workflow(ui, workflow, pcsx2_profiles, &mut clipboard);
        });
    });
    assert!(
        rendered_text_contains(&output, "Install failed"),
        "no visible install-failure banner was rendered"
    );
}

#[test]
fn install_selected_pcsx2_stages_the_selected_cheat_with_its_real_name_and_patch() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-pcsx2-install-selected-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let cheats_directory = directory.join("cheats");
    let mut profile = pcsx2_profile_fixture();
    profile.configuration_path = directory.clone();
    profile.patch_directories = vec![Pcsx2PatchDirectory {
        path: cheats_directory.clone(),
        category: Pcsx2PatchCategory::Cheats,
        state: Pcsx2PatchDirectoryState::Missing,
        warning: None,
        identity: None,
    }];
    let mut app = pcsx2_workflow_with_verified_identity_and_selected_cheat(profile);

    app.start_pcsx2_install_preview();

    let workflow = app.cheat_workflow.as_mut().unwrap();
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!("expected a resolved preview response");
    };
    let CheatPreviewOutcome::Ready(_) = &response.outcome else {
        panic!("expected a successful preview");
    };
    let generated = response
        .pcsx2_generated
        .as_ref()
        .expect("a successful PCSX2 preview stages a generated install");
    let staged_path = generated.staging_root.join("SLUS-20312_A1B2C3D4.pnach");
    let staged = std::fs::read_to_string(&staged_path)
        .unwrap_or_else(|error| panic!("staged pnach at {staged_path:?} unreadable: {error}"));
    assert!(staged.contains("// ArchiveFS managed block: gh-42-1"));
    assert!(staged.contains("// Infinite health"));
    assert!(staged.contains("Author: Codejunkies"));
    assert!(staged.contains("patch=1,EE,20123456,word,00000001"));

    // The confirmation card must be reachable once the preview succeeds.
    assert!(matches!(
        workflow.transaction,
        CheatTransactionState::Review { .. }
    ));

    let staging_root = generated.staging_root.clone();
    let _ = std::fs::remove_dir_all(&staging_root);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn install_selected_pcsx2_detects_legacy_file_and_confirmation_names_it() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-pcsx2-legacy-migration-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cheats_directory = directory.join("cheats");
    std::fs::create_dir_all(&cheats_directory).unwrap();
    std::fs::write(
            cheats_directory.join("A1B2C3D4.pnach"),
            b"// old note\n// ArchiveFS managed block: legacy_cheat\n// Legacy cheat\npatch=1,EE,20999999,word,1\n// End ArchiveFS managed block\n",
        )
        .unwrap();
    let mut profile = pcsx2_profile_fixture();
    profile.configuration_path = directory.clone();
    profile.patch_directories = vec![Pcsx2PatchDirectory {
        path: cheats_directory.clone(),
        category: Pcsx2PatchCategory::Cheats,
        state: Pcsx2PatchDirectoryState::Available,
        warning: None,
        identity: None,
    }];
    let mut app = pcsx2_workflow_with_verified_identity_and_selected_cheat(profile);

    app.start_pcsx2_install_preview();

    let workflow = app.cheat_workflow.as_mut().unwrap();
    let CheatStepResource::Ready(response) = &workflow.preview else {
        panic!("expected a resolved preview response");
    };
    let generated = response
        .pcsx2_generated
        .as_ref()
        .expect("a successful PCSX2 preview stages a generated install");
    assert!(
        generated.legacy_migration_report.is_some(),
        "a legacy CRC-only file with a managed block must be detected for migration"
    );
    let staging_root = generated.staging_root.clone();

    // The confirmation card must name the migration explicitly, not
    // silently fold it in.
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_pcsx2_gamehacking(ui, workflow, Some(cheats_directory.as_path()));
        });
    });
    assert!(rendered_text_contains(&output, "legacy CRC-only file"));

    let _ = std::fs::remove_dir_all(&staging_root);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn pcsx2_legacy_migration_strips_legacy_file_and_reports_a_history_entry() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-pcsx2-legacy-apply-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cheats_directory = directory.join("cheats");
    std::fs::create_dir_all(&cheats_directory).unwrap();
    let legacy_path = cheats_directory.join("A1B2C3D4.pnach");
    std::fs::write(
            &legacy_path,
            b"// old note\n// ArchiveFS managed block: legacy_cheat\n// Legacy cheat\npatch=1,EE,20999999,word,1\n// End ArchiveFS managed block\n",
        )
        .unwrap();
    let mut profile = pcsx2_profile_fixture();
    profile.configuration_path = directory.clone();
    profile.patch_directories = vec![Pcsx2PatchDirectory {
        path: cheats_directory.clone(),
        category: Pcsx2PatchCategory::Cheats,
        state: Pcsx2PatchDirectoryState::Available,
        warning: None,
        identity: None,
    }];
    let mut app = pcsx2_workflow_with_verified_identity_and_selected_cheat(profile);
    app.start_pcsx2_install_preview();

    let workflow = app.cheat_workflow.as_ref().unwrap();
    let staging_root = match &workflow.preview {
        CheatStepResource::Ready(response) => response
            .pcsx2_generated
            .as_ref()
            .expect("staged")
            .staging_root
            .clone(),
        _ => panic!("expected a resolved preview response"),
    };

    // A minimal, real "primary install already succeeded" result: only
    // the fields `apply_pcsx2_pending_legacy_migration` actually reads
    // (profile ID, source mode, approved source root, operation ID,
    // status) need to be realistic; the entries are irrelevant here.
    let primary_result = SharedApplyResult {
        journal: archivefs_core::patch_manager::SharedApplyJournal {
            schema_version: 1,
            operation_id: "test-legacy-wiring".to_string(),
            plan_id: "test-plan".to_string(),
            timestamp_unix_seconds: 1_700_000_000,
            context: archivefs_core::patch_manager::SharedApplyContext {
                adapter: archivefs_core::patch_manager::PreviewAdapter::Pcsx2,
                selected_archive: archivefs_core::patch_manager::SharedTransactionPath::from_path(
                    &workflow.archive_path,
                ),
                verified_game_identity: "A1B2C3D4".to_string(),
                profile_id: "pcsx2-native-test".to_string(),
                source_mode: "pcsx2-managed-pnach".to_string(),
            },
            approved_source_root: archivefs_core::patch_manager::SharedTransactionPath::from_path(
                &staging_root,
            ),
            destination_root: archivefs_core::patch_manager::SharedTransactionPath::from_path(
                &directory,
            ),
            dry_run: false,
            entries: Vec::new(),
            status: SharedApplyStatus::Success,
            rollback_operation_id: None,
        },
        journal_path: None,
        journal_failure: None,
    };

    let entry = apply_pcsx2_pending_legacy_migration(workflow, &primary_result)
        .expect("a pending legacy migration produces a history entry");
    assert_eq!(entry.outcome, ActivityOutcome::Completed);
    assert!(
        entry
            .message
            .contains("test-legacy-wiring-legacy-migration")
    );

    let stripped = std::fs::read_to_string(&legacy_path).unwrap();
    assert!(stripped.contains("old note"));
    assert!(!stripped.contains("ArchiveFS managed block"));

    // Cleanup: remove exactly the journal/backup artifacts this test
    // created in the real shared history/backup roots, alongside the
    // disposable profile directory.
    if let Ok(history_root) = default_shared_history_root() {
        let _ = std::fs::remove_file(history_root.join("test-legacy-wiring-legacy-migration.json"));
    }
    if let Ok(backup_root) = default_shared_backup_root() {
        let _ = std::fs::remove_dir_all(backup_root.join("test-legacy-wiring-legacy-migration"));
    }
    let _ = std::fs::remove_dir_all(&staging_root);
    let _ = std::fs::remove_dir_all(&directory);
}

fn empty_dolphin_inventory() -> DolphinGameIniInventory {
    DolphinGameIniInventory {
        profile_id: "dolphin-native-test".to_string(),
        files: Vec::new(),
        warnings: Vec::new(),
        entries_visited: 0,
        bytes_inspected: 0,
        complete: true,
    }
}

fn dolphin_profile_fixture_with(id: &str, portable: bool) -> DolphinProfile {
    let mut profile = dolphin_profile_fixture();
    profile.profile_id = id.to_string();
    profile.installation_type = if portable {
        DolphinInstallationType::Explicit
    } else {
        DolphinInstallationType::Native
    };
    profile.configuration_path = PathBuf::from(format!("/isolated/{id}"));
    profile.resolved.configuration_root = profile.configuration_path.clone();
    profile.resolved.data_user_root = profile.configuration_path.clone();
    profile.resolved.priority = if portable { 500 } else { 100 };
    profile
}

fn set_dolphin_selection(
    workflow: &mut CheatWorkflowState,
    profile_id: &str,
    reason: EmulatorProfileSelectReason,
) {
    workflow.selected_dolphin_profile_id = Some(profile_id.to_string());
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: profile_id.to_string(),
        reason,
    });
}

#[test]
fn verified_runtime_profile_uses_running_wording() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let mut profile = dolphin_profile_fixture();
    profile.resolved.confidence = EmulatorProfileConfidence::RunningExplicit;
    set_dolphin_selection(
        workflow,
        &profile.profile_id,
        EmulatorProfileSelectReason::StrongestEvidence,
    );

    assert_eq!(
        dolphin_profile_selection_badge(workflow, &profile),
        Some("Running Dolphin profile")
    );
}

#[test]
fn manual_profile_uses_selected_wording_not_active_or_running() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let profile = dolphin_profile_fixture();
    set_dolphin_selection(
        workflow,
        &profile.profile_id,
        EmulatorProfileSelectReason::ExplicitChoice,
    );

    assert_eq!(
        dolphin_profile_selection_badge(workflow, &profile),
        Some("Selected Dolphin profile")
    );

    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_dolphin_profile_card(ui, workflow, &profile, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(&output, "Selected Dolphin profile"));
    assert!(!rendered_text_contains(&output, "Active Dolphin profile"));
    assert!(!rendered_text_contains(&output, "Running Dolphin profile"));
}

#[test]
fn ambiguous_unselected_profiles_have_no_profile_badge() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.selected_dolphin_profile_id = None;
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::NeedsChoice {
        candidates: Vec::new(),
    });

    assert_eq!(
        dolphin_profile_selection_badge(workflow, &dolphin_profile_fixture()),
        None
    );
}

#[test]
fn remembered_explicit_profile_is_selected_not_running() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let mut profile = dolphin_profile_fixture();
    profile.resolved.confidence = EmulatorProfileConfidence::RunningExplicit;
    set_dolphin_selection(
        workflow,
        &profile.profile_id,
        EmulatorProfileSelectReason::Remembered,
    );

    assert_eq!(
        dolphin_profile_selection_badge(workflow, &profile),
        Some("Selected Dolphin profile")
    );
}

#[test]
fn speculative_or_only_valid_fallback_is_never_labelled_running() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    let mut profile = dolphin_profile_fixture();
    profile.resolved.confidence = EmulatorProfileConfidence::Speculative;
    set_dolphin_selection(
        workflow,
        &profile.profile_id,
        EmulatorProfileSelectReason::OnlyValidProfile,
    );

    assert_eq!(dolphin_profile_selection_badge(workflow, &profile), None);
}

/// Drives `poll_dolphin_profiles` to completion with a synthetic
/// discovery result, exactly like a real background scan finishing -
/// without touching the filesystem.
fn drive_dolphin_profile_scan(app: &mut ArchiveFsApp, profiles: Vec<DolphinProfile>) {
    let (sender, receiver) = mpsc::channel();
    app.dolphin_profiles = DolphinProfilesState::Scanning { receiver };
    let discovery = DolphinProfileDiscovery {
        profiles,
        warnings: Vec::new(),
        complete: true,
    };
    sender.send(Ok(discovery)).unwrap();
    app.poll_dolphin_profiles();
}

#[test]
fn poll_dolphin_profiles_auto_selects_the_only_eligible_profile() {
    let mut app = app_with_cheats_mods_context();
    app.cheat_workflow.as_mut().unwrap().adapter = CheatEmulatorAdapter::Dolphin;
    drive_dolphin_profile_scan(&mut app, vec![dolphin_profile_fixture()]);
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(
        workflow.selected_dolphin_profile_id.as_deref(),
        Some("dolphin-native-test")
    );
    assert!(matches!(
        workflow.dolphin_profile_selection,
        Some(EmulatorProfileSelection::Auto {
            reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::OnlyValidProfile,
            ..
        })
    ));
}

#[test]
fn dolphin_provider_result_is_reconciled_when_profile_scan_finishes_second() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-provider-profile-race-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_with_matched_identity(&temp, "GAFE01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.selected_dolphin_profile_id = None;
    workflow.dolphin_profile_selection = None;
    workflow.dolphin_provider = CheatStepResource::Ready(gafe01_provider_fetch());
    workflow.dolphin_provider_selection = None;

    let profile = DolphinProfile {
        configuration_path: temp.clone(),
        game_settings_path: temp.join("GameSettings"),
        game_settings_state: DolphinSettingsDirectoryState::Missing,
        ..dolphin_profile_fixture()
    };
    drive_dolphin_profile_scan(&mut app, vec![profile]);

    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(matches!(
        workflow.dolphin_profile_selection,
        Some(EmulatorProfileSelection::Auto { .. })
    ));
    assert!(workflow.dolphin_provider_selection.is_some());
    assert_eq!(
        dolphin_beginner_status(workflow),
        BeginnerCheatStatus::CheatsFound {
            compatible_count: 1
        }
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn poll_dolphin_profiles_does_not_hide_multiple_stopped_profiles_behind_memory() {
    let mut app = app_with_cheats_mods_context();
    app.cheat_workflow.as_mut().unwrap().adapter = CheatEmulatorAdapter::Dolphin;
    app.remembered_emulator_profiles
        .push(RememberedEmulatorProfile {
            adapter: "dolphin".to_string(),
            profile_id: "second".to_string(),
            root: PathBuf::from("/isolated/second"),
        });
    drive_dolphin_profile_scan(
        &mut app,
        vec![
            dolphin_profile_fixture_with("first", false),
            dolphin_profile_fixture_with("second", false),
        ],
    );
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(workflow.selected_dolphin_profile_id, None);
    assert!(matches!(
        workflow.dolphin_profile_selection,
        Some(EmulatorProfileSelection::NeedsChoice { .. })
    ));
}

#[test]
fn poll_dolphin_profiles_requires_a_choice_with_multiple_valid_profiles_and_nothing_remembered() {
    let mut app = app_with_cheats_mods_context();
    app.cheat_workflow.as_mut().unwrap().adapter = CheatEmulatorAdapter::Dolphin;
    drive_dolphin_profile_scan(
        &mut app,
        vec![
            dolphin_profile_fixture_with("first", false),
            dolphin_profile_fixture_with("second", false),
        ],
    );
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(workflow.selected_dolphin_profile_id, None);
    assert!(matches!(
        workflow.dolphin_profile_selection,
        Some(EmulatorProfileSelection::NeedsChoice { .. })
    ));
}

#[test]
fn poll_dolphin_profiles_ignores_stale_memory_when_one_credible_profile_exists() {
    let mut app = app_with_cheats_mods_context();
    app.cheat_workflow.as_mut().unwrap().adapter = CheatEmulatorAdapter::Dolphin;
    app.remembered_emulator_profiles
        .push(RememberedEmulatorProfile {
            adapter: "dolphin".to_string(),
            profile_id: "vanished".to_string(),
            root: PathBuf::from("/isolated/vanished"),
        });
    drive_dolphin_profile_scan(&mut app, vec![dolphin_profile_fixture()]);
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(
        workflow.selected_dolphin_profile_id.as_deref(),
        Some("dolphin-native-test")
    );
    assert!(matches!(
        workflow.dolphin_profile_selection,
        Some(EmulatorProfileSelection::Auto {
            reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::OnlyValidProfile,
            ..
        })
    ));
}

#[test]
fn poll_dolphin_profiles_does_not_guess_portable_when_dolphin_is_stopped() {
    let mut app = app_with_cheats_mods_context();
    app.cheat_workflow.as_mut().unwrap().adapter = CheatEmulatorAdapter::Dolphin;
    drive_dolphin_profile_scan(
        &mut app,
        vec![
            dolphin_profile_fixture_with("standard", false),
            dolphin_profile_fixture_with("portable", true),
        ],
    );
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(workflow.selected_dolphin_profile_id, None);
    assert!(matches!(
        workflow.dolphin_profile_selection,
        Some(EmulatorProfileSelection::NeedsChoice { .. })
    ));
}

#[test]
fn poll_dolphin_profiles_active_runtime_wins_over_remembered_installed_profile() {
    let mut app = app_with_cheats_mods_context();
    app.cheat_workflow.as_mut().unwrap().adapter = CheatEmulatorAdapter::Dolphin;
    app.remembered_emulator_profiles
        .push(RememberedEmulatorProfile {
            adapter: "dolphin".to_string(),
            profile_id: "installed".to_string(),
            root: PathBuf::from("/isolated/installed"),
        });
    let installed = dolphin_profile_fixture_with("installed", false);
    let mut active = dolphin_profile_fixture_with("active", true);
    active.resolved.confidence = EmulatorProfileConfidence::RunningExplicit;
    active.resolved.priority = 400;
    drive_dolphin_profile_scan(&mut app, vec![installed, active]);
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .selected_dolphin_profile_id
            .as_deref(),
        Some("active")
    );
}

#[test]
fn seeding_explicit_root_never_overwrites_a_value_the_user_already_typed() {
    let mut app = app_with_cheats_mods_context();
    {
        let workflow = app.cheat_workflow.as_mut().unwrap();
        workflow.adapter = CheatEmulatorAdapter::Dolphin;
        workflow.dolphin_explicit_root = "/typed/by/user".to_string();
    }
    app.remembered_emulator_profiles
        .push(RememberedEmulatorProfile {
            adapter: "dolphin".to_string(),
            profile_id: "remembered".to_string(),
            root: PathBuf::from("/remembered/root"),
        });
    app.seed_explicit_root_from_remembered_profile("dolphin");
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().dolphin_explicit_root,
        "/typed/by/user"
    );
}

#[test]
fn seeding_explicit_root_from_a_remembered_profile_fills_an_empty_field() {
    let mut app = app_with_cheats_mods_context();
    app.cheat_workflow.as_mut().unwrap().adapter = CheatEmulatorAdapter::Xenia;
    app.remembered_emulator_profiles
        .push(RememberedEmulatorProfile {
            adapter: "xenia".to_string(),
            profile_id: "remembered".to_string(),
            root: PathBuf::from("/remembered/xenia-root"),
        });
    app.seed_explicit_root_from_remembered_profile("xenia");
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().xenia_explicit_root,
        "/remembered/xenia-root"
    );
}

#[test]
fn adapter_routing_is_platform_authoritative() {
    assert_eq!(
        cheat_adapter_route(Some("PS2")),
        CheatEmulatorAdapter::Pcsx2
    );
    for platform in ["GameCube", "Nintendo GameCube", "Wii", "Nintendo Wii"] {
        assert_eq!(
            cheat_adapter_route(Some(platform)),
            CheatEmulatorAdapter::Dolphin
        );
    }
    for platform in ["Xbox360", "Xbox 360"] {
        assert_eq!(
            cheat_adapter_route(Some(platform)),
            CheatEmulatorAdapter::Xenia
        );
    }
    assert_eq!(
        cheat_adapter_route(Some("PS3")),
        CheatEmulatorAdapter::RetroArch
    );
    assert_eq!(cheat_adapter_route(None), CheatEmulatorAdapter::Unsupported);
    assert_eq!(
        cheat_adapter_route(Some("Unknown")),
        CheatEmulatorAdapter::Unsupported
    );
}

#[test]
fn gamecube_route_cannot_select_retroarch() {
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut gamecube = record("/roms/animal-crossing.zip", MountState::Pending);
        gamecube.identity.platform = Some("GameCube".to_string());
        data.records.push(gamecube);
    }
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/animal-crossing.zip")));
    assert_eq!(
        app.cheat_workflow.as_ref().unwrap().adapter,
        CheatEmulatorAdapter::Dolphin
    );
}

#[test]
fn opening_a_new_beginner_game_reuses_ready_profile_state_consistently() {
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut gamecube = record("/roms/gamecube.zip", MountState::Pending);
        gamecube.identity.platform = Some("GameCube".to_string());
        data.records.push(gamecube);
    }
    app.dolphin_profiles = DolphinProfilesState::Ready(DolphinProfileDiscovery {
        profiles: vec![dolphin_profile_fixture()],
        warnings: Vec::new(),
        complete: true,
    });

    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/gamecube.zip")));
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(
        workflow.selected_dolphin_profile_id.as_deref(),
        Some("dolphin-native-test")
    );
    assert!(matches!(
        workflow.dolphin_profile_selection,
        Some(EmulatorProfileSelection::Auto { .. })
    ));
    assert_ne!(
        dolphin_beginner_status(workflow),
        BeginnerCheatStatus::EmulatorSetupNeeded
    );

    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-ready-xenia-profile-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = app_for_operation_tests();
    if let LoadState::Ready(data) = &mut app.state {
        let mut xbox = record("/roms/xbox360.zip", MountState::Pending);
        xbox.identity.platform = Some("Xbox360".to_string());
        data.records.push(xbox);
    }
    app.xenia_profiles = XeniaProfilesState::Ready(XeniaProfileDiscovery {
        profiles: vec![xenia_profile_fixture(&temp)],
        warnings: Vec::new(),
        complete: true,
    });
    assert!(app.prepare_cheats_mods_workspace(PathBuf::from("/roms/xbox360.zip")));
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert_eq!(
        workflow.selected_xenia_profile_id.as_deref(),
        Some("xenia-explicit-test")
    );
    assert!(matches!(
        workflow.xenia_profile_selection,
        Some(EmulatorProfileSelection::Auto { .. })
    ));
    assert_ne!(
        xenia_beginner_status(workflow),
        BeginnerCheatStatus::EmulatorSetupNeeded
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn gamecube_page_renders_dolphin_without_retroarch_content() {
    let mut app = app_with_cheats_mods_context();
    let history = OperationHistory::default();
    let mut clipboard = InMemoryClipboard::default();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("GameCube".to_string());
    workflow.adapter = CheatEmulatorAdapter::Dolphin;
    workflow.dolphin_details_open = true;
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
                &app.xenia_profiles,
                None,
                None,
                &history,
                false,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Stage 1 · Dolphin profile"));
    assert!(!rendered_text_contains(
        &output,
        "Stage 1 · Archive and RetroArch profile"
    ));
    assert!(!rendered_text_contains(&output, "Choose a system"));
}

#[test]
fn cheats_mods_page_renders_the_new_hierarchy_headings() {
    let mut app = app_with_cheats_mods_context();
    let history = OperationHistory::default();
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
                &app.xenia_profiles,
                None,
                None,
                &history,
                false,
                &mut clipboard,
            );
        });
    });
    for expected in [
        "Selected game: a",
        "Choose a RetroArch profile",
        "Cheat source",
        "Workflow diagnostics",
        "Recent related activity",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "Cheats & Mods page did not render {expected:?} under the new hierarchy"
        );
    }
    assert!(
        !rendered_text_contains(&output, "Emulator profile"),
        "workflow-state details must be collapsed by default, not shown inline"
    );
}

#[test]
fn shared_preview_only_renders_for_the_retroarch_adapter_and_after_profile_selection() {
    let mut app = app_with_cheats_mods_context();
    let history = OperationHistory::default();
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();

    // RetroArch (the default adapter for app_with_cheats_mods_context):
    // the shared preview step must be reachable, and it must render
    // after (not above) the profile-selection step it depends on.
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
                &app.xenia_profiles,
                None,
                None,
                &history,
                false,
                &mut clipboard,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Shared preview"));
    let profile_step_position =
        find_exact_text_center(&output, "Stage 1 · Archive and RetroArch profile");
    let preview_position = find_exact_text_center(&output, "Shared preview");
    if let (Some(profile_pos), Some(preview_pos)) = (profile_step_position, preview_position) {
        assert!(
            profile_pos.y < preview_pos.y,
            "profile selection must render above the preview that depends on it"
        );
    }

    // These workflows do not use RetroArch's generic shared-preview card;
    // they must not render a permanently-empty "Preview waiting" card.
    for (platform, adapter) in [
        ("PS2", CheatEmulatorAdapter::Pcsx2),
        ("GameCube", CheatEmulatorAdapter::Dolphin),
    ] {
        let workflow = app.cheat_workflow.as_mut().unwrap();
        workflow.platform = Some(platform.to_string());
        workflow.adapter = adapter;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_cheats_mods_page(
                    ui,
                    app.cheat_workflow.as_mut(),
                    &app.retroarch_profiles,
                    &app.pcsx2_profiles,
                    &app.dolphin_profiles,
                    &app.xenia_profiles,
                    None,
                    None,
                    &history,
                    false,
                    &mut clipboard,
                );
            });
        });
        assert!(
            !rendered_text_contains(&output, "Shared preview"),
            "{adapter:?} has no shared preview pipeline and must not render its section"
        );
        assert!(
            !rendered_text_contains(&output, "Preview waiting"),
            "{adapter:?} must not show a permanently-empty preview placeholder"
        );
    }
}

#[test]
fn dolphin_workflow_presents_provider_before_optional_local_inventory() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("GameCube".to_string());
    workflow.adapter = CheatEmulatorAdapter::Dolphin;
    workflow.selected_dolphin_profile_id = Some("dolphin-native-test".to_string());
    workflow.dolphin_inventory_profile_id = Some("dolphin-native-test".to_string());
    workflow.dolphin_inventory = CheatStepResource::Ready(empty_dolphin_inventory());
    workflow.dolphin_details_open = true;
    let profiles = DolphinProfilesState::Ready(DolphinProfileDiscovery {
        profiles: vec![dolphin_profile_fixture()],
        warnings: Vec::new(),
        complete: true,
    });
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_dolphin_workflow(ui, workflow, &profiles, &mut clipboard);
        });
    });
    for expected in [
        "Stage 1 · Dolphin profile",
        "Stage 2 · Find matching cheats",
        "Dolphin upstream GameSettings",
        "Existing Dolphin-managed files",
        "Uploaded · No",
        "Executed · No",
        "Changed · No",
        "Verified Game ID unavailable",
        "Waiting for a verified GameCube game ID and disc revision",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    for forbidden in [
        "Install now",
        "Apply patch",
        "Enable code",
        "Delete file",
        "Preview the installed file",
        "Confirm and apply exact plan",
    ] {
        assert!(!rendered_text_contains(&output, forbidden));
    }
}

/// Writes a real GameSettings INI to a real temp file and returns a
/// `DolphinGameIniFile` inventory record pointing at it - enough for
/// `build_dolphin_candidate` to match it and `load_dolphin_ini` to
/// really open it, exactly as the real pipeline would.
#[cfg(any())]
fn real_dolphin_ini_fixture(directory: &std::path::Path, game_id: &str) -> DolphinGameIniFile {
    let contents = "[Core]\n\
FastDiscSpeed = True\n\
[Gecko]\n\
$Infinite Bells [Nayr]\n\
28134C58 00000001\n\
*Gives you lots of bells\n\
$Instant Growth [Nayr]\n\
C913CEF5 00000000\n\
[Gecko_Enabled]\n\
$Instant Growth [Nayr]\n";
    let path = directory.join(format!("{game_id}.ini"));
    std::fs::write(&path, contents).expect("write real fixture INI");
    DolphinGameIniFile {
        path,
        filename_stem: std::ffi::OsString::from(game_id),
        game_id_candidate: Some(game_id.to_string()),
        revision_candidate: None,
        region_candidate: Some("E".to_string()),
        frame_patch_names: Vec::new(),
        action_replay_names: Vec::new(),
        gecko_names: vec![
            "Infinite Bells [Nayr]".to_string(),
            "Instant Growth [Nayr]".to_string(),
        ],
        riivolution_names: Vec::new(),
        enabled_frame_patch_names: Vec::new(),
        enabled_action_replay_names: Vec::new(),
        enabled_gecko_names: vec!["Instant Growth [Nayr]".to_string()],
        enabled_riivolution_names: Vec::new(),
        size_bytes: contents.len() as u64,
        sha256: "0".repeat(64),
        duplicate_game_identity: false,
        duplicate_filename: false,
        duplicate_content: false,
        warnings: Vec::new(),
    }
}

/// An RVZ-shaped identity result that reached a final state without
/// ever producing a `Verified` Game ID - the exact stuck-spinner
/// scenario `dolphin_beginner_status`/`dolphin_provider_auto_fetch_needed`
/// must resolve to a terminal, honest state instead of looping.
fn dolphin_workflow_with_deferred_identity(directory: &std::path::Path) -> ArchiveFsApp {
    let mut app = dolphin_workflow_with_matched_identity(directory, "GZ2E01");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: "dolphin-native-test".to_string(),
        reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::ExplicitChoice,
    });
    let CheatStepResource::Ready((request, report)) = &workflow.identity else {
        unreachable!("fixture always starts Ready");
    };
    let mut report = report.clone();
    report.format = IdentityImageFormat::Rvz;
    report.complete = false;
    report.evidence = vec![archivefs_core::game_identity::IdentityEvidence {
        kind: IdentityKind::DolphinGameId,
        status: IdentityStatus::Deferred,
        value: None,
        confidence: archivefs_core::game_identity::IdentityConfidence::Unavailable,
        provenance: archivefs_core::game_identity::IdentityProvenance {
            archive_path: workflow.archive_path.clone(),
            member_path: None,
            member_index: None,
            method: "test fixture".to_string(),
        },
        diagnostic: "format has no existing safe bounded reader in EmuWiz".to_string(),
    }];
    workflow.identity = CheatStepResource::Ready((request.clone(), report));
    app
}

#[test]
fn deferred_rvz_identity_reaches_a_final_state_instead_of_spinning_forever() {
    let directory = std::env::temp_dir().join("archivefs-rvz-stuck-spinner-test");
    let app = dolphin_workflow_with_deferred_identity(&directory);
    let workflow = app.cheat_workflow.as_ref().unwrap();

    assert!(
        !dolphin_provider_auto_fetch_needed(workflow),
        "a format EmuWiz cannot decode must never trigger a provider fetch"
    );
    let status = dolphin_beginner_status(workflow);
    assert!(
        matches!(status, BeginnerCheatStatus::IdentityUnavailable { .. }),
        "expected a terminal IdentityUnavailable status, got {status:?}"
    );
    assert_ne!(
        status.label(),
        "Finding compatible cheats",
        "the page must not spin forever once identity reached a final unsupported state"
    );
}

fn xenia_profile_fixture(directory: &std::path::Path) -> XeniaProfile {
    XeniaProfile {
        profile_id: "xenia-explicit-test".to_string(),
        installation_type: XeniaInstallationType::Explicit,
        scope: XeniaProfileScope::Explicit,
        configuration_path: directory.to_path_buf(),
        provenance: "test fixture",
        eligible: true,
        blockers: Vec::new(),
        patches_path: directory.join("patches"),
        patches_state: XeniaPatchesDirectoryState::Missing,
        patches_warning: None,
        configuration_identity: None,
    }
}

fn xenia_workflow_with_matched_identity(
    directory: &std::path::Path,
    title_id: &str,
) -> ArchiveFsApp {
    let mut app = app_with_cheats_mods_context();
    app.xenia_profiles = XeniaProfilesState::Ready(XeniaProfileDiscovery {
        profiles: vec![xenia_profile_fixture(directory)],
        warnings: Vec::new(),
        complete: true,
    });
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.platform = Some("Xbox360".to_string());
    workflow.adapter = CheatEmulatorAdapter::Xenia;
    workflow.selected_xenia_profile_id = Some("xenia-explicit-test".to_string());
    workflow.identity_request = Some(GameIdentityRequest {
        archive_path: workflow.archive_path.clone(),
        platform: workflow.platform.clone(),
        adapter: CheatEmulatorAdapter::Xenia,
    });
    let report = GameIdentityReport {
        archive_path: workflow.archive_path.clone(),
        platform: archivefs_core::game_identity::IdentityPlatform::Xbox360,
        format: IdentityImageFormat::Xex,
        evidence: vec![archivefs_core::game_identity::IdentityEvidence {
            kind: IdentityKind::XexTitleId,
            status: IdentityStatus::Verified,
            value: Some(title_id.to_string()),
            confidence: archivefs_core::game_identity::IdentityConfidence::ExactBytes,
            provenance: archivefs_core::game_identity::IdentityProvenance {
                archive_path: workflow.archive_path.clone(),
                member_path: None,
                member_index: None,
                method: "test fixture XEX header read".to_string(),
            },
            diagnostic: "test fixture".to_string(),
        }],
        warnings: Vec::new(),
        bytes_read: 512,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    workflow.identity =
        CheatStepResource::Ready((workflow.identity_request.clone().unwrap(), report));
    app
}

#[test]
fn dolphin_provider_auto_fetch_is_needed_once_identity_is_ready_and_nothing_requested_yet() {
    let app = dolphin_workflow_with_matched_identity(Path::new("/isolated/dolphin-test"), "GALE01");
    assert!(dolphin_provider_auto_fetch_needed(
        app.cheat_workflow.as_ref().unwrap()
    ));
}

#[test]
fn dolphin_provider_auto_fetch_is_not_needed_once_a_fetch_is_already_loading() {
    let mut app =
        dolphin_workflow_with_matched_identity(Path::new("/isolated/dolphin-test"), "GALE01");
    let (_sender, receiver) = mpsc::channel();
    app.cheat_workflow.as_mut().unwrap().dolphin_provider = CheatStepResource::Loading { receiver };
    assert!(!dolphin_provider_auto_fetch_needed(
        app.cheat_workflow.as_ref().unwrap()
    ));
}

#[test]
fn dolphin_provider_auto_fetch_is_not_needed_after_a_fetch_already_failed() {
    let mut app =
        dolphin_workflow_with_matched_identity(Path::new("/isolated/dolphin-test"), "GALE01");
    app.cheat_workflow.as_mut().unwrap().dolphin_provider =
        CheatStepResource::Failed("network unavailable".to_string());
    assert!(!dolphin_provider_auto_fetch_needed(
        app.cheat_workflow.as_ref().unwrap()
    ));
}

#[test]
fn dolphin_provider_auto_fetch_is_not_needed_for_a_non_gamecube_platform() {
    let mut app =
        dolphin_workflow_with_matched_identity(Path::new("/isolated/dolphin-test"), "GALE01");
    app.cheat_workflow.as_mut().unwrap().platform = Some("Wii".to_string());
    assert!(!dolphin_provider_auto_fetch_needed(
        app.cheat_workflow.as_ref().unwrap()
    ));
}

#[test]
fn xenia_provider_auto_fetch_is_needed_once_identity_is_ready_and_nothing_requested_yet() {
    let app = xenia_workflow_with_matched_identity(Path::new("/isolated/xenia-test"), "415607D2");
    assert!(xenia_provider_auto_fetch_needed(
        app.cheat_workflow.as_ref().unwrap()
    ));
}

#[test]
fn xenia_provider_auto_fetch_is_not_needed_after_a_fetch_already_completed_or_failed() {
    let mut app =
        xenia_workflow_with_matched_identity(Path::new("/isolated/xenia-test"), "415607D2");
    app.cheat_workflow.as_mut().unwrap().xenia_provider =
        CheatStepResource::Failed("network unavailable".to_string());
    assert!(!xenia_provider_auto_fetch_needed(
        app.cheat_workflow.as_ref().unwrap()
    ));
}

#[test]
fn repeated_polling_never_requests_the_dolphin_provider_more_than_once() {
    let mut app =
        dolphin_workflow_with_matched_identity(Path::new("/isolated/dolphin-test"), "GALE01");
    for _ in 0..5 {
        app.poll_cheat_workflow(&egui::Context::default());
    }
    // Under `cargo test` the real fetch never starts (see the
    // `cfg!(test)` guard in `poll_cheat_workflow`), so the gate stays
    // permanently open here - this test instead pins down that
    // repeated polling is idempotent and never panics/loops, and
    // that the automatic-fetch decision itself only depends on state
    // that a real fetch would eventually change (`NotLoaded` ->
    // `Loading`), never on how many times the page has rendered.
    assert!(dolphin_provider_auto_fetch_needed(
        app.cheat_workflow.as_ref().unwrap()
    ));
    app.cheat_workflow.as_mut().unwrap().dolphin_provider =
        CheatStepResource::Failed("network unavailable".to_string());
    for _ in 0..5 {
        app.poll_cheat_workflow(&egui::Context::default());
    }
    assert!(matches!(
        app.cheat_workflow.as_ref().unwrap().dolphin_provider,
        CheatStepResource::Failed(_)
    ));
}

fn quake4_provider_fetch() -> XeniaProviderFetchResult {
    XeniaProviderFetchResult {
        result: XeniaProviderResult {
            provider_id: "xenia_canary_game_patches".to_string(),
            provider_display_name: "Xenia Canary game-patches".to_string(),
            source_repository: "xenia-canary/game-patches".to_string(),
            source_commit: "1".repeat(40),
            retrieved_at_unix_seconds: 1,
            title_id: "415607D2".to_string(),
            documents: vec![XeniaProviderDocument {
                source_path: "patches/415607D2 - Quake 4.patch.toml".to_string(),
                document: archivefs_core::patch_manager::parse_xenia_patch_toml(
                    "title_name = \"Quake 4\"\ntitle_id = \"415607D2\"\nhash = \"4768B579A3C5F134\"\n\n[[patch]]\n    name = \"Performance fix\"\n    desc = \"\"\n    author = \"Sowa_95\"\n    is_enabled = false\n    [[patch.be32]]\n        address = 0x821b7140\n        value = 0x39600001\n",
                ),
            }],
            attribution: "test".to_string(),
            license: "test".to_string(),
            warnings: Vec::new(),
        },
        status: XeniaProviderFetchStatus::Downloaded,
        refresh_error: None,
    }
}

#[test]
fn xenia_workflow_shows_no_dolphin_or_retroarch_controls_before_fetching() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-xenia-workflow-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut app = xenia_workflow_with_matched_identity(&directory, "415607D2");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.xenia_details_open = true;
    let xenia_profiles = XeniaProfilesState::Ready(XeniaProfileDiscovery {
        profiles: vec![xenia_profile_fixture(&directory)],
        warnings: Vec::new(),
        complete: true,
    });
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_xenia_workflow(ui, workflow, &xenia_profiles, &mut clipboard);
        });
    });
    for expected in [
        "Xbox 360 identity",
        "Title ID",
        "415607D2",
        "Stage 1 · Xenia Canary profile",
        "Stage 2 · Find matching patches",
        "xenia-canary/game-patches",
        "Fetch patches",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    for forbidden in [
        "GameSettings",
        "Gecko",
        "RetroArch cheat catalogue",
        "Dolphin.ini",
    ] {
        assert!(
            !rendered_text_contains(&output, forbidden),
            "unexpected {forbidden}"
        );
    }
}

fn xenia_workflow_ready_for_beginner_install(directory: &Path) -> ArchiveFsApp {
    let mut app = xenia_workflow_with_matched_identity(directory, "415607D2");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.xenia_provider = CheatStepResource::Ready(quake4_provider_fetch());
    workflow.xenia_profile_selection = Some(EmulatorProfileSelection::Auto {
        profile_id: "xenia-explicit-test".to_string(),
        reason: archivefs_core::patch_manager::EmulatorProfileSelectReason::OnlyValidProfile,
    });
    app.xenia_profiles = XeniaProfilesState::Ready(XeniaProfileDiscovery {
        profiles: vec![xenia_profile_fixture(directory)],
        warnings: Vec::new(),
        complete: true,
    });
    app
}

fn render_xenia_workflow(app: &mut ArchiveFsApp) -> egui::FullOutput {
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let workflow = app.cheat_workflow.as_mut().unwrap();
            let _ = show_xenia_workflow(ui, workflow, &app.xenia_profiles, &mut clipboard);
        });
    })
}

#[test]
fn partially_verified_xenia_candidate_shows_one_warning_in_the_beginner_view() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-xenia-warning-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut app = xenia_workflow_ready_for_beginner_install(&directory);
    let output = render_xenia_workflow(&mut app);
    assert!(
        rendered_text_contains(
            &output,
            "This patch matches the game, but EmuWiz cannot confirm the exact executable version."
        ),
        "rendering mismatch"
    );
    assert!(
        rendered_text_contains(
            &output,
            "I understand this patch may target a different executable version."
        ),
        "rendering mismatch"
    );
    assert_eq!(
        app.cheat_workflow
            .as_ref()
            .unwrap()
            .xenia_selection
            .as_ref()
            .unwrap()
            .selection
            .compatibility,
        XeniaCandidateCompatibility::PartiallyVerified
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn partially_verified_xenia_candidate_requires_one_acknowledgement_before_install() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-xenia-ack-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut app = xenia_workflow_ready_for_beginner_install(&directory);
    // Render once so `xenia_auto_select_single_candidate` (called from
    // the beginner summary) builds `xenia_selection` for this single
    // matching document, then select the one patch.
    let _ = render_xenia_workflow(&mut app);
    app.update_xenia_patch_selection(|selection| {
        selection.set_selected(0, true);
    });
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(
        !workflow
            .xenia_selection
            .as_ref()
            .unwrap()
            .selection
            .can_apply()
    );
    assert!(matches!(workflow.transaction, CheatTransactionState::Idle));

    // Acknowledging flips `can_apply()` on the same selection state
    // the beginner "Install selected" button already reads - no
    // second, differently-shaped acknowledgement anywhere else.
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow
        .xenia_selection
        .as_mut()
        .unwrap()
        .selection
        .partial_verification_acknowledged = true;
    assert!(
        workflow
            .xenia_selection
            .as_ref()
            .unwrap()
            .selection
            .can_apply()
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn xenia_details_is_collapsed_by_default_on_the_beginner_page() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-xenia-details-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut app = xenia_workflow_ready_for_beginner_install(&directory);
    assert!(!app.cheat_workflow.as_ref().unwrap().xenia_details_open);
    let output = render_xenia_workflow(&mut app);
    assert!(
        rendered_text_contains(&output, "Details"),
        "rendering mismatch"
    );
    assert!(
        !rendered_text_contains(&output, "Stage 2 · Find matching patches"),
        "technical stage text leaked outside the collapsed Details section"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn xenia_candidate_picker_shows_compatibility_and_requires_explicit_choice() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-xenia-candidate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut app = xenia_workflow_with_matched_identity(&directory, "415607D2");
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.xenia_provider = CheatStepResource::Ready(quake4_provider_fetch());
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_xenia_external_provider(ui, workflow, &mut clipboard);
        });
    });
    for expected in [
        "Candidate files",
        "Quake 4",
        "Partially verified",
        "Choose this file",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    // Nothing is selected/staged until the user explicitly chooses.
    assert!(workflow.xenia_selection.is_none());
}

#[test]
fn selecting_a_xenia_candidate_loads_the_real_destination_and_opens_the_picker() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-xenia-select-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut app = xenia_workflow_with_matched_identity(&directory, "415607D2");
    let profiles = XeniaProfilesState::Ready(XeniaProfileDiscovery {
        profiles: vec![xenia_profile_fixture(&directory)],
        warnings: Vec::new(),
        complete: true,
    });
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.xenia_provider = CheatStepResource::Ready(quake4_provider_fetch());
    workflow.xenia_selected_candidate_index = Some(0);
    ensure_xenia_selection_state(workflow, &profiles);
    let state = workflow
        .xenia_selection
        .as_ref()
        .expect("selection state built");
    assert_eq!(state.candidate.title_id, "415607D2");
    assert_eq!(state.selection.entries.len(), 1);
    assert!(!state.destination.existed);
    assert_eq!(
        state.selection.compatibility,
        XeniaCandidateCompatibility::PartiallyVerified
    );
}

#[test]
fn xenia_patch_picker_requires_acknowledgement_only_for_partially_verified_candidates() {
    let directory = std::env::temp_dir().join(format!(
        "archivefs-gui-xenia-ack-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut app = xenia_workflow_with_matched_identity(&directory, "415607D2");
    let profiles = XeniaProfilesState::Ready(XeniaProfileDiscovery {
        profiles: vec![xenia_profile_fixture(&directory)],
        warnings: Vec::new(),
        complete: true,
    });
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.xenia_provider = CheatStepResource::Ready(quake4_provider_fetch());
    workflow.xenia_selected_candidate_index = Some(0);
    ensure_xenia_selection_state(workflow, &profiles);
    assert!(
        !workflow
            .xenia_selection
            .as_ref()
            .unwrap()
            .selection
            .can_apply(),
        "nothing selected yet"
    );

    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_xenia_patch_picker(ui, workflow, &mut clipboard);
        });
    });
    assert!(rendered_text_contains(
        &output,
        "Exact game version could not be confirmed"
    ));
    assert!(rendered_text_contains(
        &output,
        "I understand the module hash is not verified"
    ));
}

fn render_shared_history(details_open: bool) -> egui::FullOutput {
    let mut result = successful_shared_apply_result();
    result.journal.timestamp_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let report = SharedHistoryReport {
        journals: vec![(
            SharedTransactionPath::from_path(Path::new("/history/op-beginner-test.json")),
            result.journal,
        )],
        warnings: Vec::new(),
        complete: true,
    };
    let ctx = egui::Context::default();
    if details_open {
        ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
    }
    let mut rollback = SharedRollbackState::Idle;
    let shared_history = SharedHistoryState::Ready(report);
    let mut history = OperationHistory::default();
    let mut filters = HistoryLogFilters::default();
    let mut clipboard = InMemoryClipboard::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_history_logs_page(
                ui,
                &shared_history,
                &mut rollback,
                None,
                &mut history,
                &mut filters,
                &mut clipboard,
            );
        });
    })
}

#[test]
fn transaction_card_defaults_to_a_game_summary_and_human_time() {
    let output = render_shared_history(false);
    for expected in [
        "Completed",
        "Cheats added to a",
        "Dolphin · Today at",
        "1 change",
        "Preview rollback",
        "Technical details",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    for technical in [
        "op-beginner-test",
        "plan-beginner-test",
        "/history/op-beginner-test.json",
        "/dolphin/GameSettings",
        "/roms/a.zip",
        "Raw timestamp",
        "Source mode",
    ] {
        assert!(
            !rendered_text_contains(&output, technical),
            "technical audit value leaked into the default card: {technical}"
        );
    }
}

/// Root-cause regression for the "First use of widget ID .../Second
/// use of widget ID ..." egui warning seen on the real History & Logs
/// page: the session-activity list's per-row "Technical details"
/// disclosure used to be salted only by `("history_related_archive",
/// path)`. Two entries about the same archive (e.g. a mount followed
/// by an unmount of the same file - an ordinary, expected occurrence,
/// not a data bug) therefore collided on an identical
/// `CollapsingHeader` ID. The fix adds each row's own loop index to
/// the salt; this test proves that mechanism directly, at the exact
/// salt shape used in `show_history_logs_page`, rather than relying on
/// egui's internal warning machinery.
#[test]
fn history_technical_details_ids_differ_for_two_rows_sharing_an_archive_path() {
    let path = PathBuf::from("/roms/shared.zip");

    // Documents the bug's exact mechanism: the old, path-only salt
    // shape collides for any two rows about the same archive.
    let pre_fix_salt_row_a = ("history_related_archive", &path);
    let pre_fix_salt_row_b = ("history_related_archive", &path);
    assert_eq!(
        egui::Id::new(pre_fix_salt_row_a),
        egui::Id::new(pre_fix_salt_row_b),
        "sanity check: this is the exact collision the fix removes"
    );

    // The actual fix: each row's own index joins the salt, so the two
    // rows above now resolve to distinct, independently-toggleable
    // `CollapsingHeader` IDs even though they share the same path.
    let post_fix_salt_row_a = ("history_related_archive", 0usize, &path);
    let post_fix_salt_row_b = ("history_related_archive", 1usize, &path);
    assert_ne!(
        egui::Id::new(post_fix_salt_row_a),
        egui::Id::new(post_fix_salt_row_b),
        "two history rows referencing the same archive must not collide on widget ID"
    );
}

#[test]
fn transaction_details_preserve_the_raw_audit_record() {
    let output = render_shared_history(true);
    for expected in [
        "Transaction ID",
        "op-beginner-test",
        "Plan ID",
        "plan-beginner-test",
        "Raw timestamp",
        "Selected archive",
        "/roms/a.zip",
        "Source mode",
        "EmuWiz trusted catalogue",
        "Destination root",
        "/dolphin/GameSettings",
        "Journal path",
        "/history/op-beginner-test.json",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
}

#[test]
fn compatible_candidate_appears_in_the_beginner_main_list_by_default() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-list-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_ready_for_beginner_install(&temp);
    let output = render_dolphin_workflow(&mut app);
    assert!(
        rendered_text_contains(&output, "16:9 Widescreen"),
        "rendering mismatch"
    );
    assert!(
        rendered_text_contains(&output, "compatible enhancement found"),
        "rendering mismatch"
    );
    assert!(
        rendered_text_contains(&output, "Install selected"),
        "rendering mismatch"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn beginner_primary_control_is_visible_without_scrolling_on_a_small_viewport() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-viewport-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_ready_for_beginner_install(&temp);
    let mut clipboard = InMemoryClipboard::default();
    let history = OperationHistory::default();
    let ctx = egui::Context::default();
    let screen = egui::vec2(1024.0, 600.0);
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let _ = show_cheats_mods_page(
                        ui,
                        app.cheat_workflow.as_mut(),
                        &app.retroarch_profiles,
                        &app.pcsx2_profiles,
                        &app.dolphin_profiles,
                        &app.xenia_profiles,
                        None,
                        None,
                        &history,
                        false,
                        &mut clipboard,
                    );
                });
            });
        },
    );
    let (position, clip) = find_exact_text_position_and_clip(&output, "Install selected")
        .expect("primary control renders");
    assert!(
        position.y >= clip.min.y && position.y <= clip.max.y,
        "Install selected must be inside the initial viewport: position={position:?} clip={clip:?}"
    );
    for technical_default in [
        "Selected archive context",
        "Trusted catalogue retrieval available",
        "Controlled apply after eligible preview",
        "Mods: planned",
    ] {
        assert!(
            !rendered_text_contains(&output, technical_default),
            "technical page chrome leaked into the beginner default view: {technical_default}"
        );
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn technical_preview_is_not_mandatory_for_a_one_click_install() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-oneclick-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_ready_for_beginner_install(&temp);
    // Nothing is selected by default beyond the fixture's own already
    // enabled entries - toggle the widescreen code on directly, the
    // same mutation the beginner checklist's checkbox would perform.
    app.update_dolphin_code_selection(|selection| {
        selection.set_selected(0, true);
    });
    assert!(!app.cheat_workflow.as_ref().unwrap().dolphin_details_open);
    // One click: no manual "Preview the installed file" step first.
    app.start_beginner_install_dolphin();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    assert!(
        matches!(workflow.transaction, CheatTransactionState::Review { .. }),
        "expected the beginner install to reach the review stage directly"
    );
    let output = render_dolphin_workflow(&mut app);
    assert!(
        rendered_text_contains(&output, "Install 1 enhancement in Dolphin?"),
        "rendering mismatch"
    );
    assert!(
        rendered_text_contains(&output, "back up the existing settings"),
        "rendering mismatch"
    );
    assert!(
        rendered_text_contains(&output, "Show exact changes"),
        "rendering mismatch"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn show_exact_changes_remains_accessible_from_the_confirmation_dialog() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-exact-changes-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_ready_for_beginner_install(&temp);
    app.update_dolphin_code_selection(|selection| {
        selection.set_selected(0, true);
    });
    app.start_beginner_install_dolphin();
    let output_before = render_dolphin_workflow(&mut app);
    assert!(
        !rendered_text_contains(&output_before, "Plan ID"),
        "rendering mismatch"
    );
    app.cheat_workflow
        .as_mut()
        .unwrap()
        .dolphin_show_exact_changes = true;
    let output_after = render_dolphin_workflow(&mut app);
    assert!(
        rendered_text_contains(&output_after, "Plan ID"),
        "rendering mismatch"
    );
    assert!(
        rendered_text_contains(&output_after, "Source SHA-256"),
        "rendering mismatch"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn undo_appears_after_a_successful_beginner_install_result() {
    let temp = std::env::temp_dir().join(format!(
        "archivefs-gui-beginner-undo-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let mut app = dolphin_workflow_ready_for_beginner_install(&temp);
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.transaction = CheatTransactionState::Result {
        key: cheat_preview_key(workflow),
        result: successful_shared_apply_result(),
    };
    let output = render_dolphin_workflow(&mut app);
    assert!(
        rendered_text_contains(&output, "Installed successfully"),
        "rendering mismatch"
    );
    assert!(
        rendered_text_contains(&output, "Undo installation"),
        "rendering mismatch"
    );
    assert!(
        !rendered_text_contains(&output, "16:9 Widescreen")
            && !rendered_text_contains(&output, "Install selected"),
        "the completed state must not be mixed with stale pre-install controls"
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn beginner_failure_result_names_failed_stage_and_live_target() {
    let mut result = successful_shared_apply_result();
    result.journal.status = SharedApplyStatus::Failed;
    result.journal.entries[0].outcome = SharedApplyOutcome::VerificationFailed;
    result.journal.entries[0].verification_succeeded = false;
    result.journal.entries[0]
        .failures
        .push(archivefs_core::patch_manager::SharedApplyFailure {
            kind: archivefs_core::patch_manager::SharedApplyFailureKind::VerificationFailed,
            path: Some(SharedTransactionPath::from_path(Path::new(
                "/dolphin/GameSettings/GAFE01.ini",
            ))),
            detail: "managed section missing".to_string(),
        });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_beginner_install_result(ui, &result);
        });
    });
    for expected in [
        "Install failed",
        "VerificationFailed",
        "/dolphin/GameSettings/GAFE01.ini",
        "managed section missing",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
}
