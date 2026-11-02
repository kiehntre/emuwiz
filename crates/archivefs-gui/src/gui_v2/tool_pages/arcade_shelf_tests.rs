//! Virtual Arcade Shelf: navigation, virtualisation and page journeys.
use super::*;
use crate::gui_v2::{
    all_tools::{self, ToolSafety},
    library::Library,
    library_failure,
    routes::ToolPage,
    tests::{self as gui, page_reachability::visible_text},
};
use std::sync::Arc;

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn frame(context: &egui::Context, app: &mut App, events: Vec<egui::Event>) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |context| app.show(context),
    )
}

fn shelf_app(context: &egui::Context, games: Vec<(i64, &str, &str)>) -> App {
    let mut app = gui::fixture(context);
    let archives = games
        .into_iter()
        .map(|(id, title, platform)| gui::archive(id, title, Some(platform)))
        .collect();
    app.library = Arc::new(Library::new(archives));
    app.loaded = true;
    app.go(Route::Tool(ToolPage::ArcadeShelf));
    app
}

fn cursor_game(app: &App) -> Option<i64> {
    app.tools.arcade_shelf.cursor.map(|cursor| cursor.game)
}

#[test]
fn arrow_keys_stay_inside_the_shelf_and_keep_the_column_where_they_can() {
    let lengths = [3, 1, 5];
    assert_eq!(step(&[], None, Move::Right), None);
    assert_eq!(step(&[2, 0], Some((0, 0)), Move::Right), None);
    for to in [Move::Left, Move::Right, Move::Up, Move::Down] {
        assert_eq!(step(&lengths, None, to), Some((0, 0)));
    }
    assert_eq!(step(&lengths, Some((0, 0)), Move::Left), Some((0, 0)));
    assert_eq!(step(&lengths, Some((0, 2)), Move::Right), Some((0, 2)));
    assert_eq!(step(&lengths, Some((0, 0)), Move::Up), Some((0, 0)));
    // Down from column 2 onto a one-game shelf lands on its only cover; the
    // next Down keeps that column rather than jumping back to column 2.
    assert_eq!(step(&lengths, Some((0, 2)), Move::Down), Some((1, 0)));
    assert_eq!(step(&lengths, Some((1, 0)), Move::Down), Some((2, 0)));
    assert_eq!(step(&lengths, Some((2, 4)), Move::Down), Some((2, 4)));
    assert_eq!(step(&lengths, Some((2, 4)), Move::Up), Some((1, 0)));
    assert_eq!(step(&lengths, Some((2, 1)), Move::RowEnd), Some((2, 4)));
    assert_eq!(step(&lengths, Some((2, 3)), Move::RowStart), Some((2, 0)));
    // A stale cursor beyond the current rows is clamped, never out of range.
    assert_eq!(step(&lengths, Some((9, 9)), Move::Left), Some((2, 3)));
}

#[test]
fn only_covers_in_view_are_laid_out_however_long_the_shelf_is() {
    assert_eq!(visible_columns(0.0, 3.0 * STRIDE, 1000), (0, 4));
    assert_eq!(visible_columns(0.0, 3.0 * STRIDE, 2), (0, 2));
    assert_eq!(visible_columns(0.0, 1000.0, 0), (0, 0));
    let (first, last) = visible_columns(50_000.0 * STRIDE, 50_000.0 * STRIDE + 1200.0, 132_000);
    assert_eq!(first, 50_000);
    assert!(last - first <= 12, "{first}..{last}");
    // Scrolled past the end (a shrinking library) yields nothing, not a panic.
    assert_eq!(
        visible_columns(9_000.0 * STRIDE, 9_010.0 * STRIDE, 10),
        (10, 10)
    );
}

#[test]
fn the_shelf_is_a_read_only_games_tool_that_honours_library_failures() {
    let page = ToolPage::ArcadeShelf;
    assert!(ToolPage::ALL.contains(&page));
    assert_eq!(page.home(), Section::Games);
    let listed: Vec<_> = all_tools::groups()
        .into_iter()
        .flat_map(|group| group.tools)
        .filter(|tool| tool.route == Route::Tool(page))
        .collect();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].safety, ToolSafety::LookingOnly);
    // A failed load is shown as a failure, never as an empty shelf.
    assert!(library_failure::needs_library(&Route::Tool(page)));
}

#[test]
fn an_empty_collection_explains_itself_while_loading_and_once_loaded() {
    let context = egui::Context::default();
    let mut app = shelf_app(&context, Vec::new());
    app.loaded = false;
    let output = frame(&context, &mut app, Vec::new());
    assert!(visible_text(&output, "Loading your game list"));
    assert!(!visible_text(&output, "Your shelf is empty"));

    app.loaded = true;
    let output = frame(&context, &mut app, Vec::new());
    assert!(visible_text(&output, "Your shelf is empty"));
    assert!(visible_text(&output, "Open Sources"));
    // Keys on an empty shelf select nothing and do not panic.
    frame(&context, &mut app, vec![key(egui::Key::ArrowRight)]);
    assert_eq!(cursor_game(&app), None);
}

#[test]
fn shelves_group_covers_by_system_and_keys_open_existing_game_details() {
    let context = egui::Context::default();
    let mut app = shelf_app(
        &context,
        vec![
            (1, "Alpha Racer", "Zeta Console"),
            (2, "Beta Blaster", "Zeta Console"),
            (3, "Gamma Quest", "Zeta Console"),
            (4, "Delta Dash", "Alpha Console"),
        ],
    );
    let output = frame(&context, &mut app, Vec::new());
    for wanted in [
        "4 games on 2 system shelves",
        "Alpha Console",
        "Zeta Console",
        "Delta Dash",
        "Alpha Racer",
    ] {
        assert!(
            visible_text(&output, wanted),
            "{wanted:?} not shown in {:?}",
            gui::text(&output)
        );
    }
    assert!(!visible_text(&output, "Open Game Details"));
    let library = app.library.clone();
    let rows: Vec<&str> = shelf_rows(&library).iter().map(|(name, _)| *name).collect();
    assert_eq!(rows, ["Alpha Console", "Zeta Console"]);

    // The first key picks the first cover; nothing is opened yet.
    frame(&context, &mut app, vec![key(egui::Key::ArrowRight)]);
    assert_eq!(cursor_game(&app), Some(4));
    let output = frame(&context, &mut app, Vec::new());
    assert!(visible_text(&output, "Open Game Details"));
    assert_eq!(app.router.current, Route::Tool(ToolPage::ArcadeShelf));

    frame(&context, &mut app, vec![key(egui::Key::ArrowDown)]);
    assert_eq!(cursor_game(&app), Some(1));
    frame(&context, &mut app, vec![key(egui::Key::End)]);
    assert_eq!(cursor_game(&app), Some(3));
    frame(&context, &mut app, vec![key(egui::Key::ArrowLeft)]);
    assert_eq!(cursor_game(&app), Some(2));
    // Back up onto the one-game shelf, then down again keeps column 0.
    frame(&context, &mut app, vec![key(egui::Key::ArrowUp)]);
    assert_eq!(cursor_game(&app), Some(4));
    frame(&context, &mut app, vec![key(egui::Key::ArrowDown)]);
    assert_eq!(cursor_game(&app), Some(1));

    frame(&context, &mut app, vec![key(egui::Key::Enter)]);
    assert_eq!(app.router.current, Route::Game(1));
    assert!(app.router.can_back());
}

#[test]
fn the_selection_follows_its_game_across_a_reload_and_clears_when_it_is_gone() {
    let context = egui::Context::default();
    let mut app = shelf_app(
        &context,
        vec![
            (1, "Bravo", "Saturn Console"),
            (2, "Charlie", "Saturn Console"),
        ],
    );
    frame(&context, &mut app, vec![key(egui::Key::ArrowRight)]);
    frame(&context, &mut app, vec![key(egui::Key::ArrowRight)]);
    assert_eq!(cursor_game(&app), Some(2));

    // A new game sorts before it: the cursor follows game 2 to its new column.
    app.library = Arc::new(Library::new(vec![
        gui::archive(1, "Bravo", Some("Saturn Console")),
        gui::archive(2, "Charlie", Some("Saturn Console")),
        gui::archive(3, "Alpha", Some("Saturn Console")),
    ]));
    let output = frame(&context, &mut app, Vec::new());
    assert_eq!(cursor_game(&app), Some(2));
    assert_eq!(app.tools.arcade_shelf.cursor.map(|c| c.col), Some(2));
    assert!(visible_text(&output, "Charlie"));

    app.library = Arc::new(Library::new(vec![gui::archive(
        1,
        "Bravo",
        Some("Saturn Console"),
    )]));
    let output = frame(&context, &mut app, Vec::new());
    assert_eq!(cursor_game(&app), None);
    assert!(!visible_text(&output, "Open Game Details"));
}

#[test]
fn a_very_large_library_lays_out_only_visible_covers_and_reaches_the_last() {
    const GAMES: i64 = 20_000;
    let titles: Vec<String> = (0..GAMES).map(|n| format!("Game {n:05}")).collect();
    let mut games: Vec<(i64, &str, &str)> = titles
        .iter()
        .enumerate()
        .map(|(n, title)| (n as i64 + 1, title.as_str(), "Zeta Computer"))
        .collect();
    games.push((GAMES + 1, "Lonely", "Alpha Computer"));
    let context = egui::Context::default();
    let mut app = shelf_app(&context, games);
    let output = frame(&context, &mut app, Vec::new());
    let laid_out = gui::text(&output)
        .iter()
        .filter(|text| text.starts_with("Game "))
        .count();
    assert!(
        (1..=40).contains(&laid_out),
        "{laid_out} cover titles laid out for a 20,000-game shelf"
    );
    assert!(visible_text(&output, "20,000 games"));

    // Down onto the long shelf, End to its last cover: it scrolls into view.
    frame(&context, &mut app, vec![key(egui::Key::ArrowDown)]);
    frame(&context, &mut app, vec![key(egui::Key::ArrowDown)]);
    frame(&context, &mut app, vec![key(egui::Key::End)]);
    assert_eq!(cursor_game(&app), Some(GAMES));
    let mut shown = false;
    for _ in 0..4 {
        let output = frame(&context, &mut app, Vec::new());
        shown |= visible_text(&output, "Game 19999");
    }
    assert!(shown, "the last cover never came into view");
}
