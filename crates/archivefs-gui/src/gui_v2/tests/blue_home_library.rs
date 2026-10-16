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

#[test]
fn proposed_readiness_filters_are_shown_disabled_with_their_reason() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, games());
    app.router.current = Route::BrowsePlay;
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Proposed"));
    assert!(has(
        &strings,
        crate::gui_v2::browse_play::PROPOSED_FILTERS_REASON
    ));
    // Clicking a proposed filter changes nothing.
    let before = app.filter.clone();
    let layout = frame(&context, &mut app, DESKTOP);
    let ready = text_bounds(&layout, "Ready")
        .into_iter()
        .next()
        .expect("Ready chip")
        .center();
    click(&context, &mut app, DESKTOP, ready);
    assert_eq!(format!("{:?}", app.filter), format!("{before:?}"));
    // The existing Needs attention filter is a working toggle.
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
