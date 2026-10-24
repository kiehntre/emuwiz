//! EmuWiz Blue Home, Library and Browse & Play (Batch 3), rendered through the
//! real `App::show`. Presentation only: every assertion is about what is
//! painted and where a click leads; nothing here may start work or launch.
use super::*;
use crate::gui_v2::home_blue::{FIRST_RUN_TITLE, HOME_TILES, WELCOME_TITLE};

const DESKTOP: [f32; 2] = [1672.0, 941.0];
const COMPACT: [f32; 2] = [1024.0, 600.0];

fn blue_app(context: &egui::Context, games: Vec<PersistedArchive>) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app.library = Arc::new(Library::new(games));
    app.loaded = true;
    app
}

fn shown(context: &egui::Context, app: &mut App, size: [f32; 2]) -> Vec<String> {
    frame(context, app, size);
    text(&frame(context, app, size))
}

fn has(strings: &[String], wanted: &str) -> bool {
    strings.iter().any(|value| value == wanted)
}

fn click(context: &egui::Context, app: &mut App, size: [f32; 2], point: egui::Pos2) {
    for pressed in [true, false] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                events: vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |context| app.show(context),
        );
    }
    frame(context, app, size);
}

fn games() -> Vec<PersistedArchive> {
    (1..=12)
        .map(|id| archive(id, &format!("Blue Fixture {id:02}"), Some("SNES")))
        .collect()
}

#[test]
fn blue_home_shows_the_welcome_tasks_and_setup_without_the_classic_layout() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, WELCOME_TITLE));
    assert!(has(&strings, "What would you like to do?"));
    for tile in &HOME_TILES {
        assert!(has(&strings, tile.title), "{} tile", tile.title);
    }
    assert!(has(&strings, "Setup status"));
    assert!(has(&strings, "Recent activity"));
    // No game was opened, so the welcome panel offers the Library.
    assert!(has(&strings, "Pick a game and it will appear here"));
    // The Classic Home's own layout is not repeated.
    assert!(!has(&strings, "Things you can do"));
    assert!(!has(&strings, "Your game library"));
    // No invented play history.
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("Continue playing"))
    );
    assert!(
        !strings
            .iter()
            .any(|value| value.contains("Recently played"))
    );
}

#[test]
fn an_empty_library_gets_the_first_run_welcome() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, Vec::new());
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, FIRST_RUN_TITLE));
    assert!(has(&strings, "Let's find your games"));
    assert!(has(&strings, "Add game folder"));
}

#[test]
fn home_tiles_only_navigate_to_their_existing_pages() {
    for tile in &HOME_TILES {
        let context = egui::Context::default();
        let mut app = blue_app(&context, games());
        let layout = {
            frame(&context, &mut app, DESKTOP);
            frame(&context, &mut app, DESKTOP)
        };
        let point = text_bounds(&layout, tile.title)
            .into_iter()
            .find(|rect| rect.min.x > 208.0)
            .unwrap_or_else(|| panic!("{} tile", tile.title))
            .center();
        click(&context, &mut app, DESKTOP, point);
        assert_eq!(
            app.router.current,
            Route::Section(tile.section),
            "{}",
            tile.title
        );
        assert!(
            app.activity
                .jobs
                .values()
                .all(|job| !job.title.to_lowercase().contains("launch")),
            "{} started a launch",
            tile.title
        );
    }
}

#[test]
fn compact_home_keeps_the_welcome_action_and_setup_in_view() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    let layout = {
        frame(&context, &mut app, COMPACT);
        frame(&context, &mut app, COMPACT)
    };
    // The primary action is visible without scrolling at 1024x600.
    let open = text_bounds(&layout, "Open Library");
    assert!(
        open.iter().any(|rect| rect.max.y <= COMPACT[1] - 36.0),
        "Open Library is below the fold: {open:?}"
    );
    // Setup status folds into one strip; Mr Wiz appears at most once.
    let strings = text(&layout);
    assert!(has(&strings, "Setup status"));
    assert_eq!(
        strings
            .iter()
            .filter(|value| *value == "Mr Wiz says…")
            .count(),
        0,
        "compact uses the strip, not the card"
    );
}

#[test]
fn selecting_a_game_never_launches_and_play_is_the_only_way_to_the_check() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::BrowsePlay;
    let layout = {
        frame(&context, &mut app, DESKTOP);
        frame(&context, &mut app, DESKTOP)
    };
    let card = text_bounds(&layout, "Blue Fixture 01")
        .into_iter()
        .next()
        .expect("a game card")
        .center();
    click(&context, &mut app, DESKTOP, card);
    // Selected, not launched: the route is the selection and no launch job
    // exists (reading its cover is the only work, as in Classic).
    assert_eq!(app.router.current, Route::BrowsePlayGame(1));
    assert!(
        app.activity
            .jobs
            .values()
            .all(|job| !job.title.to_lowercase().contains("launch")),
        "selecting started a launch"
    );
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, crate::gui_v2::browse_play::SELECT_VS_PLAY));
    // The selection is still settling: no game file has been read for it,
    // the card says so, and Play is not held back by the pending check.
    assert!(
        !evidence_for(&app, 1),
        "a fresh selection read the game file"
    );
    assert!(has(&strings, PLAY_RUNS_THE_CHECK));
    // Play opens the launch-readiness check, never a launch.
    let layout = frame(&context, &mut app, DESKTOP);
    let play = text_bounds(&layout, "Play")
        .into_iter()
        .find(|rect| rect.min.x > DESKTOP[0] * 0.5)
        .expect("Play")
        .center();
    click(&context, &mut app, DESKTOP, play);
    assert_eq!(
        app.router.current,
        Route::Task {
            section: Section::Launch,
            game: 1
        }
    );
}

use crate::gui_v2::browse_play::SELECTION_SETTLE_SECS;
use crate::gui_v2::launch_readiness_summary::PLAY_RUNS_THE_CHECK;

fn frame_at(context: &egui::Context, app: &mut App, size: [f32; 2], time: f64) -> Vec<String> {
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            time: Some(time),
            ..Default::default()
        },
        |context| app.show(context),
    );
    text(&output)
}

fn fixture_path(id: i64) -> std::path::PathBuf {
    format!("/fixture/Blue Fixture {id:02}.iso").into()
}

/// Whether the read-only readiness check has started for (or finished with)
/// game `id`'s file.
fn evidence_for(app: &App, id: i64) -> bool {
    app.native_workflows
        .as_ref()
        .is_some_and(|workflows| workflows.has_evidence_for(&fixture_path(id)))
}

#[test]
fn rapid_selection_reads_no_game_file_until_the_selection_settles() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    let mut time = 10.0;
    for id in 1..=4 {
        app.router.current = Route::BrowsePlayGame(id);
        let strings = frame_at(&context, &mut app, DESKTOP, time);
        // The selected game, its cover slot and Play are drawn at once.
        assert!(has(&strings, &format!("Blue Fixture {id:02}")));
        assert!(has(&strings, "Play"));
        assert!(has(&strings, PLAY_RUNS_THE_CHECK));
        time += SELECTION_SETTLE_SECS / 3.0;
        frame_at(&context, &mut app, DESKTOP, time);
        for earlier in 1..=id {
            assert!(
                !evidence_for(&app, earlier),
                "game {earlier} was read while browsing quickly"
            );
        }
    }
    // Once the selection stays put, exactly that game's check starts.
    frame_at(&context, &mut app, DESKTOP, time + SELECTION_SETTLE_SECS);
    assert!(evidence_for(&app, 4));
    for earlier in 1..=3 {
        assert!(!evidence_for(&app, earlier));
    }
    assert!(
        app.activity
            .jobs
            .values()
            .all(|job| !job.title.to_lowercase().contains("launch")),
        "selecting started a launch"
    );
}

#[test]
fn a_late_result_for_the_previous_game_is_never_shown_for_the_new_one() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::BrowsePlayGame(1);
    frame_at(&context, &mut app, DESKTOP, 1.0);
    frame_at(&context, &mut app, DESKTOP, 2.0);
    assert!(evidence_for(&app, 1));
    // Game 1's answer (whatever it is) belongs to game 1 only.
    app.router.current = Route::BrowsePlayGame(2);
    let strings = frame_at(&context, &mut app, DESKTOP, 2.05);
    assert!(has(&strings, PLAY_RUNS_THE_CHECK), "{strings:?}");
    assert!(!evidence_for(&app, 2));
    frame_at(&context, &mut app, DESKTOP, 3.0);
    assert!(evidence_for(&app, 2));
    assert!(!evidence_for(&app, 1));
    // Back to game 1: its check runs again for the current file, not from a
    // remembered answer.
    app.router.current = Route::BrowsePlayGame(1);
    frame_at(&context, &mut app, DESKTOP, 3.05);
    assert!(!evidence_for(&app, 1));
}

/// A game file that never answers (a FIFO with no writer stands in for a hung
/// remote mount) must not freeze Blue, disable Play, or be retried each frame.
#[cfg(unix)]
#[test]
fn a_game_on_storage_that_never_answers_does_not_freeze_the_library_or_block_play() {
    let dir = tempfile::tempdir().unwrap();
    let hung = dir.path().join("Hung Disc.iso");
    let status = std::process::Command::new("mkfifo")
        .arg(&hung)
        .status()
        .expect("mkfifo");
    assert!(status.success());
    let mut game = archive(1, "Hung Disc", Some("PlayStation"));
    game.absolute_path = hung.clone();
    let context = egui::Context::default();
    let mut app = blue_app(
        &context,
        vec![game, archive(2, "Blue Fixture 02", Some("SNES"))],
    );
    app.router.current = Route::BrowsePlayGame(1);
    let started = std::time::Instant::now();
    let mut time = 1.0;
    let mut strings = Vec::new();
    for _ in 0..20 {
        strings = frame_at(&context, &mut app, DESKTOP, time);
        time += 0.1;
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "Blue waited on the file: {:?}",
        started.elapsed()
    );
    let workflows = app.native_workflows.as_ref().unwrap();
    assert!(
        workflows.has_evidence_for(&hung),
        "one check for the hung file"
    );
    let generation = workflows
        .app
        .selected_evidence_ui
        .selected_evidence_generation;
    // Still pending: Play stays usable and opens the Launch preflight.
    assert!(has(&strings, "Play"));
    let pending = matches!(
        workflows.app.selected_evidence_ui.selected_evidence,
        crate::selected_evidence_page::SelectedEvidenceState::Loading { .. }
    );
    assert!(pending, "the hung file's check should still be waiting");
    assert!(has(&strings, PLAY_RUNS_THE_CHECK), "{strings:?}");
    let layout = frame(&context, &mut app, DESKTOP);
    let play = text_bounds(&layout, "Play")
        .into_iter()
        .find(|rect| rect.min.x > DESKTOP[0] * 0.5)
        .expect("Play")
        .center();
    click(&context, &mut app, DESKTOP, play);
    assert_eq!(
        app.router.current,
        Route::Task {
            section: Section::Launch,
            game: 1
        }
    );
    app.router.current = Route::BrowsePlayGame(1);
    // The clicks above advanced egui's own clock.
    time += 1.0;
    // Further frames never start another check for the same file.
    for _ in 0..10 {
        frame_at(&context, &mut app, DESKTOP, time);
        time += 0.1;
    }
    let workflows = app.native_workflows.as_ref().unwrap();
    assert_eq!(
        workflows
            .app
            .selected_evidence_ui
            .selected_evidence_generation,
        generation,
        "the hung file was checked again"
    );
    // Choosing another game stops waiting on the hung one straight away.
    app.router.current = Route::BrowsePlayGame(2);
    frame_at(&context, &mut app, DESKTOP, time);
    assert!(
        !app.native_workflows
            .as_ref()
            .unwrap()
            .has_evidence_for(&hung)
    );
    // Release the parked worker thread.
    let _ = std::fs::OpenOptions::new().write(true).open(&hung);
}

#[test]
fn compact_library_keeps_readiness_and_play_above_the_fold_before_and_after_the_check() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::BrowsePlayGame(1);
    for time in [1.0, 1.05, 3.0, 3.05, 4.0] {
        frame_at(&context, &mut app, COMPACT, time);
        let layout = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(COMPACT[0], COMPACT[1]),
                )),
                time: Some(time + 0.01),
                ..Default::default()
            },
            |context| app.show(context),
        );
        let play = text_bounds(&layout, "Play");
        assert!(
            play.iter().any(|rect| rect.max.y <= COMPACT[1]),
            "Play is below the fold at {time}: {play:?}"
        );
        let details = text_bounds(&layout, "Game details");
        assert!(
            details.iter().any(|rect| rect.max.y <= COMPACT[1]),
            "Game details is below the fold at {time}: {details:?}"
        );
    }
    // Let the read-only check for the (missing) fixture file finish, as the
    // app's own frame loop would.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let workflows = app.native_workflows.as_mut().unwrap();
        workflows.poll(&context, &mut app.activity);
        let loading = matches!(
            workflows.app.selected_evidence_ui.selected_evidence,
            crate::selected_evidence_page::SelectedEvidenceState::Loading { .. }
        );
        if !loading || std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    frame_at(&context, &mut app, COMPACT, 6.0);
    let layout = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(COMPACT[0], COMPACT[1]),
            )),
            time: Some(6.01),
            ..Default::default()
        },
        |context| app.show(context),
    );
    let strings = text(&layout);
    // The card now names the check's real answer, never the old promise.
    assert!(
        !strings
            .iter()
            .any(|s| s.contains("Play will show what is missing"))
    );
    assert!(!has(&strings, "! Checking"), "{strings:?}");
    let play = text_bounds(&layout, "Play");
    assert!(
        play.iter().any(|rect| rect.max.y <= COMPACT[1]),
        "Play is below the fold after the check: {play:?} {strings:?}"
    );
}

#[test]
fn browse_play_shows_no_developer_placeholder_filters() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::BrowsePlay;
    let strings = shown(&context, &mut app, DESKTOP);
    // Unimplemented readiness filters are not advertised to players.
    assert!(!strings.iter().any(|s| s.contains("Proposed")));
    assert!(!strings.iter().any(|s| s.contains("does not keep yet")));
    assert!(!has(&strings, "Not checked"));
    // The existing Needs attention filter is still a working toggle.
    let layout = frame(&context, &mut app, DESKTOP);
    let attention = text_bounds(&layout, "Needs attention")
        .into_iter()
        .next()
        .expect("Needs attention")
        .center();
    click(&context, &mut app, DESKTOP, attention);
    assert!(app.filter.attention_only);
}

#[test]
fn the_classic_home_is_unchanged() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.simple = crate::gui_v2::simple_shell::ShellState::classic_view();
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Your game library"));
    assert!(!has(&strings, WELCOME_TITLE));
}

// ---- Batch 4: Game Details ----

fn with_saved_record(
    games: Vec<PersistedArchive>,
    id: i64,
    values: archivefs_core::screenscraper_enrichment::AcceptedScreenScraperMetadata,
) -> Library {
    let mut library = Library::new(games);
    let index = library
        .games
        .iter()
        .position(|game| game.archive.id == id)
        .unwrap();
    library.games[index].screenscraper = Some(
        archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment {
            archive_id: id,
            values,
            receipt: archivefs_core::screenscraper_enrichment::ScreenScraperEnrichmentReceipt {
                provider: "ScreenScraper".into(),
                provider_record_id: "ss-7".into(),
                retrieved_at_unix_seconds: 1,
                match_basis: "verified hash".into(),
                before: Default::default(),
                accepted: Default::default(),
                media_reference_count: 0,
            },
        },
    );
    library
}

#[test]
fn game_information_rows_show_only_saved_values() {
    use crate::gui_v2::game_detail_blue::information_rows;
    let library = with_saved_record(
        games(),
        2,
        archivefs_core::screenscraper_enrichment::AcceptedScreenScraperMetadata {
            developer: Some("Fixture Developer".into()),
            release_year: Some(1994),
            genre: Some("   ".into()),
            languages: Some(vec!["en".into(), "fr".into()]),
            ..Default::default()
        },
    );
    let rows = information_rows(library.game(2).unwrap());
    assert_eq!(
        rows,
        vec![
            ("Developer", "Fixture Developer".to_string()),
            ("Year", "1994".to_string()),
            ("Languages", "en, fr".to_string()),
        ]
    );
    // No saved record: no rows, never guessed ones.
    assert!(information_rows(library.game(1).unwrap()).is_empty());
}

#[test]
fn blue_readiness_keeps_red_for_real_failures_only() {
    use crate::gui_v2::launch_readiness_summary::ReadinessPresentationState as State;
    use crate::gui_v2::launch_readiness_summary::blue_status_tone;
    use crate::ui::components::StatusTone;
    for setup in [
        State::NeedsEmulator,
        State::NeedsFirmware,
        State::NeedsIdentityReview,
    ] {
        assert_eq!(blue_status_tone(setup), StatusTone::Warning, "{setup:?}");
    }
    for failure in [
        State::SourceUnavailable,
        State::MediaUnreadable,
        State::Blocked,
    ] {
        assert_eq!(
            blue_status_tone(failure),
            StatusTone::Blocked,
            "{failure:?}"
        );
    }
    assert_eq!(blue_status_tone(State::Ready), StatusTone::Success);
}

#[test]
fn blue_game_details_put_readiness_information_and_advanced_beside_the_hero() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::Game(2);
    frame(&context, &mut app, DESKTOP);
    let first = frame(&context, &mut app, DESKTOP);
    for wanted in ["Can I play it?", "More for this game", "Blue Fixture 02"] {
        assert!(
            visible_text(&first, wanted),
            "{wanted} missing: {:?}",
            text(&first)
        );
    }
    let readiness = text_bounds(&first, "Can I play it?")[0];
    let hero = text_bounds(&first, "More for this game")[0];
    assert!(
        readiness.min.x > hero.max.x,
        "readiness must be beside the hero: {readiness:?} {hero:?}"
    );
    // The readiness card now includes firmware and save-state guidance. The
    // following cards may be below the fold, but must stay in the right column
    // and be reachable in the original viewport.
    for label in ["Game information", "Advanced details"] {
        let output = scroll_to_text(&context, &mut app, DESKTOP, label);
        let card = text_bounds(&output, label)[0];
        assert!(
            card.min.x >= readiness.min.x,
            "{label} left the right column: {card:?} {readiness:?}"
        );
    }
    // Opening a game never starts a launch.
    assert!(
        app.activity
            .jobs
            .values()
            .all(|job| !job.title.to_lowercase().contains("launch"))
    );
    // The Classic page keeps its single column without the new cards.
    app.simple.enabled = false;
    let output = scroll_to_text(&context, &mut app, DESKTOP, "More for this game");
    assert!(!has(&text(&output), "Can I play it?"));
}

#[test]
fn arcade_game_details_explain_missing_machine_and_controller_evidence_at_compact_size() {
    let context = egui::Context::default();
    let mut arcade_game = archive(41, "Pac-Man", Some("Arcade"));
    arcade_game.archive_kind = "arcade_set_directory".into();
    let mut app = blue_app(&context, vec![arcade_game]);
    app.router.current = Route::Game(41);
    shown(&context, &mut app, COMPACT);
    let output = page_reachability::scroll_to_text(
        &context,
        &mut app,
        COMPACT,
        "Connected controller mapping and compatibility: Unknown.",
    );
    let strings = text(&output);
    for expected in [
        "Arcade controls",
        "A single verified MAME machine identity is not available for this game.",
        "Original controls are unknown until machine identity is established.",
        "Connected controller mapping and compatibility: Unknown.",
    ] {
        assert!(has(&strings, expected), "{expected} missing: {strings:?}");
    }
    assert!(!has(&strings, "controller works"));
}

// ---- Batch 5: Setup & Doctor ----

fn snapshot_with_sources() -> crate::gui_v2::environment::EnvironmentSnapshot {
    crate::gui_v2::environment::EnvironmentSnapshot {
        config_present: true,
        database_present: true,
        source_count: 1,
        available_sources: 1,
        ..Default::default()
    }
}

#[test]
fn setup_steps_say_the_same_thing_in_both_views_and_next_step_prefers_attention() {
    use crate::gui_v2::onboarding::{StepState, next_step, setup_steps};
    let library = Library::new(games());
    let snapshot = snapshot_with_sources();
    let steps = setup_steps(&snapshot, &library);
    assert_eq!(steps.len(), 5);
    assert_eq!(steps[0].label, "Folder available");
    // Games but no emulator: emulators need attention and lead.
    assert_eq!(steps[1].state, StepState::Attention);
    let (index, step) = next_step(&steps).unwrap();
    assert_eq!(index, 1);
    assert_eq!(step.action.unwrap().0, "Open Emulator Setup");
    // Nothing needing attention: the first optional step with an action.
    let empty = Library::new(Vec::new());
    let steps = setup_steps(&snapshot, &empty);
    assert_eq!(next_step(&steps).unwrap().0, 1);
}

#[test]
fn blue_setup_shows_step_cards_and_the_next_step_while_classic_keeps_its_grid() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.environment = Some(snapshot_with_sources());
    app.router.current = Route::Section(Section::Setup);
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, "1. Games folder"));
    assert!(has(&strings, "NEXT STEP"));
    assert!(!has(&strings, "System setup"));
    // Switch the way Settings does, so the remembered Classic style returns.
    app.simple.enabled = false;
    let strings = shown(&context, &mut app, DESKTOP);
    // Classic keeps its own System setup card and grid.
    assert!(has(&strings, "System setup"));
    assert!(!has(&strings, "NEXT STEP"));
}

#[test]
fn creating_a_starter_config_asks_first_and_cancel_or_escape_change_nothing() {
    use crate::gui_v2::onboarding::SetupConfirm;
    let target = std::path::PathBuf::from("/nonexistent-emuwiz-test/config.toml");
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.environment = Some(crate::gui_v2::environment::EnvironmentSnapshot {
        starter_config_path: Some(target.clone()),
        ..snapshot_with_sources()
    });
    app.router.current = Route::Section(Section::Setup);
    // A tall window: the Configuration card sits mid-page.
    const DESKTOP: [f32; 2] = [1672.0, 2400.0];
    frame(&context, &mut app, DESKTOP);
    let layout = frame(&context, &mut app, DESKTOP);
    let request = text_bounds(&layout, "Create Starter Config…")
        .into_iter()
        .next()
        .expect("the create button")
        .center();
    click(&context, &mut app, DESKTOP, request);
    // Asked, not done: the dialog names the exact file; no job started.
    assert_eq!(
        app.setup_actions.confirm,
        Some(SetupConfirm::StarterConfig(target.clone()))
    );
    assert!(app.setup_actions.job.is_none());
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(
        strings
            .iter()
            .any(|value| value.contains("/nonexistent-emuwiz-test/config.toml"))
    );
    // Cancel is focused first and closes it without a job.
    let layout = frame(&context, &mut app, DESKTOP);
    let cancel = text_bounds(&layout, "Cancel")
        .into_iter()
        .next()
        .expect("Cancel")
        .center();
    click(&context, &mut app, DESKTOP, cancel);
    assert!(app.setup_actions.confirm.is_none());
    assert!(app.setup_actions.job.is_none());
    assert!(!target.exists());
    // Escape closes it too.
    app.setup_actions.confirm = Some(SetupConfirm::StarterConfig(target.clone()));
    frame(&context, &mut app, DESKTOP);
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(DESKTOP[0], DESKTOP[1]),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |context| app.show(context),
    );
    assert!(app.setup_actions.confirm.is_none());
    assert!(app.setup_actions.job.is_none());
}

#[test]
fn a_stale_confirmation_creates_nothing() {
    // The person confirmed a path that is not the current configuration
    // location: refused before anything is written.
    let directory = tempfile::tempdir().unwrap();
    let stale = directory.path().join("elsewhere").join("config.toml");
    let error = crate::gui_v2::backend::create_starter_config_at(&stale).unwrap_err();
    assert!(error.contains("Nothing was created"), "{error}");
    assert!(!stale.exists());
    assert!(!stale.parent().unwrap().exists());
}

// ---- Batch 9: Saves ----

#[test]
fn saves_opens_native_save_vault_without_starting_a_restore() {
    use crate::gui_v2::legacy::overlay_for;
    // The explicit specialist handoff remains available; the native action
    // below must stay inside Blue instead of invoking it.
    assert_eq!(
        overlay_for(Section::Saves),
        Some(crate::navigation::ToolsOverlay::SaveVault)
    );
    // No other handoff silently opens the vault.
    for section in crate::gui_v2::routes::SECTIONS {
        if *section != Section::Saves {
            assert_eq!(overlay_for(*section), None, "{section:?}");
        }
    }
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.saves_states.inventory = Some(saves_inventory(Vec::new()));
    app.router.current = Route::Section(Section::Saves);
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Open PS2 Save Vault"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Read-only inspection"))
    );
    assert!(!app.saves_states.vault_open);
    assert!(app.native_workflows.is_none());
    assert!(!has(&strings, "Replace existing save"));

    let strings = text(&click_label(
        &context,
        &mut app,
        DESKTOP,
        "Open PS2 Save Vault",
    ));
    assert!(app.saves_states.vault_open);
    assert_eq!(app.router.current, Route::Section(Section::Saves));
    assert!(has(&strings, "Check PCSX2 Cards"));
    assert!(
        strings
            .iter()
            .any(|value| value.contains("Explicit confirmation required"))
    );
    assert!(has(&strings, "Back to save inventory"));
    assert!(!has(&strings, "Replace existing save"));
    assert!(!has(&strings, "Legacy / Advanced interface"));
    assert!(app.handoff_status.is_none());
    assert_eq!(app.activity.running(), 0);
    let native = app.native_workflows.as_ref().unwrap();
    assert!(matches!(
        native.app.emulator_readiness.pcsx2_status,
        crate::pcsx2_page::Pcsx2StatusState::Idle
    ));
    assert!(
        native
            .app
            .emulator_readiness
            .pcsx2_save_vault
            .manual_loading
            .is_none()
    );
}
