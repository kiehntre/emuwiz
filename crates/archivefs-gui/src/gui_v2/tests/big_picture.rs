//! Big Picture journeys through real frames of the App. Controller input goes
//! in at the same `PadEvent` level the evdev reader produces.
use super::big_picture::{
    Step,
    nav::{Button, Control, Direction, PadEvent},
};
use super::*;

const SIZE: [f32; 2] = [1600.0, 900.0];

fn app_with(context: &egui::Context, count: i64) -> App {
    let mut app = fixture(context);
    app.library = Arc::new(Library::new(
        (1..=count)
            .map(|id| {
                archive(
                    id,
                    &format!("Game {id:06}"),
                    Some(if id % 2 == 0 { "SNES" } else { "Amiga" }),
                )
            })
            .collect(),
    ));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.loaded = true;
    app.router.current = Route::BrowsePlay;
    app
}

fn opened(context: &egui::Context, count: i64) -> App {
    let mut app = app_with(context, count);
    app.enter_big_picture(context);
    settle(context, &mut app);
    app
}

fn settle(context: &egui::Context, app: &mut App) -> egui::FullOutput {
    let _ = frame(context, app, SIZE);
    frame(context, app, SIZE)
}

fn tap(context: &egui::Context, app: &mut App, control: Control) -> egui::FullOutput {
    let state = app.big_picture.as_mut().expect("Big Picture is open");
    state.inject(PadEvent::Pressed { device: 1, control });
    state.inject(PadEvent::Released { device: 1, control });
    settle(context, app)
}

const SOUTH: Control = Control::Button(Button::South);
const EAST: Control = Control::Button(Button::East);
const START: Control = Control::Button(Button::Start);
const WEST: Control = Control::Button(Button::West);
const NORTH: Control = Control::Button(Button::North);
const RIGHT: Control = Control::Direction(Direction::Right);
const DOWN: Control = Control::Direction(Direction::Down);

fn key(k: egui::Key) -> Vec<egui::Event> {
    [true, false]
        .map(|pressed| egui::Event::Key {
            key: k,
            physical_key: Some(k),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        })
        .to_vec()
}

fn state(app: &App) -> &big_picture::BigPicture {
    app.big_picture.as_ref().expect("Big Picture is open")
}

fn after_launch_page_loads(context: &egui::Context, app: &mut App) -> Vec<String> {
    text(&settle(context, app))
}

#[test]
fn the_library_button_opens_big_picture_and_shows_large_tiles_only_for_the_screen() {
    let context = egui::Context::default();
    let mut app = app_with(&context, 200);
    let output = frame(&context, &mut app, SIZE);
    assert!(text(&output).iter().any(|s| s == "Big Picture"));
    app.enter_big_picture(&context);
    let strings = text(&settle(&context, &mut app));
    assert!(strings.iter().any(|s| s == "Game 000001"));
    assert!(strings.iter().any(|s| s.contains("200 games")));
    let titles = strings.iter().filter(|s| s.starts_with("Game 0")).count();
    assert!(titles <= 24, "virtualised, drew {titles}");
    assert!(
        !strings.iter().any(|s| s.contains("Ready · browsing")),
        "no desktop chrome"
    );
}

#[test]
fn no_controller_means_a_window_with_keyboard_hints_and_a_plain_statement() {
    let context = egui::Context::default();
    let mut app = opened(&context, 5);
    assert!(
        !state(&app).fullscreen(),
        "a desktop user keeps their window"
    );
    let strings = text(&frame(&context, &mut app, SIZE));
    assert!(
        strings
            .iter()
            .any(|s| s.contains("not supported") || s.contains("No controller"))
    );
    for hint in [
        "A / Enter  Select",
        "B / Esc  Back",
        "Start / L  Launch",
        "X / S  Search",
    ] {
        assert!(strings.iter().any(|s| s == hint), "{hint}");
    }
}

#[test]
fn d_pad_moves_focus_and_select_opens_the_focused_game() {
    let context = egui::Context::default();
    let mut app = opened(&context, 100);
    let columns = state(&app).columns();
    assert!(columns > 1);
    tap(&context, &mut app, RIGHT);
    assert_eq!(state(&app).focus(), 1);
    tap(&context, &mut app, DOWN);
    assert_eq!(state(&app).focus(), 1 + columns);
    let id = state(&app).focused_game().unwrap();
    assert_eq!(id, 2 + columns as i64);
    tap(&context, &mut app, SOUTH);
    assert_eq!(state(&app).step(), Step::Game(id));
}

#[test]
fn keyboard_has_the_same_moves_as_the_controller() {
    let context = egui::Context::default();
    let mut app = opened(&context, 100);
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::ArrowRight));
    assert_eq!(state(&app).focus(), 1);
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::End));
    assert_eq!(state(&app).focus(), 99);
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::Home));
    assert_eq!(state(&app).focus(), 0);
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::Enter));
    assert!(matches!(state(&app).step(), Step::Game(_)));
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::Escape));
    assert_eq!(state(&app).step(), Step::Browse);
}

#[test]
fn back_walks_out_one_step_at_a_time_and_leaving_needs_a_second_press() {
    let context = egui::Context::default();
    let mut app = opened(&context, 10);
    tap(&context, &mut app, SOUTH);
    tap(&context, &mut app, START);
    assert!(matches!(state(&app).step(), Step::Launch(_)));
    tap(&context, &mut app, EAST);
    assert!(matches!(state(&app).step(), Step::Game(_)));
    tap(&context, &mut app, EAST);
    assert_eq!(state(&app).step(), Step::Browse);
    tap(&context, &mut app, EAST);
    assert!(app.big_picture.is_some(), "one stray press does not leave");
    assert!(state(&app).notice_text().unwrap().contains("again"));
    tap(&context, &mut app, RIGHT);
    assert_eq!(state(&app).notice_text(), None, "moving disarms it");
    tap(&context, &mut app, EAST);
    tap(&context, &mut app, EAST);
    assert!(app.big_picture.is_none());
    assert_eq!(
        app.router.current,
        Route::BrowsePlay,
        "the desktop page is unchanged"
    );
}

#[test]
fn launch_only_opens_the_existing_launch_page_and_starts_nothing() {
    let context = egui::Context::default();
    let mut app = opened(&context, 10);
    tap(&context, &mut app, START);
    let strings = after_launch_page_loads(&context, &mut app);
    assert!(matches!(state(&app).step(), Step::Launch(1)), "{strings:?}");
    assert!(strings.iter().any(|s| s == "Play / Launch readiness"));
    let workflows = app.native_workflows.as_ref().unwrap();
    assert!(!workflows.launch_in_progress());
    // Pressing Start again on that page is not a confirmation either.
    tap(&context, &mut app, START);
    tap(&context, &mut app, START);
    assert!(!app.native_workflows.as_ref().unwrap().launch_in_progress());
}

#[test]
fn controller_walks_launch_controls_keeps_them_on_screen_and_clicks_only_the_focused_one() {
    use super::big_picture::{nav::Command, page_command};
    let context = egui::Context::default();
    let ids = std::cell::RefCell::new(Vec::new());
    let clicked = std::cell::RefCell::new(Vec::new());
    let clip = std::cell::Cell::new(egui::Rect::NOTHING);
    let run = |command: Option<Command>| {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE.into())),
                ..Default::default()
            },
            |context| {
                if let Some(command) = command {
                    page_command(context, command);
                }
                ids.borrow_mut().clear();
                egui::CentralPanel::default().show(context, |ui| {
                    super::super::native_workflows::launch_scroll(ui, 7, |ui| {
                        clip.set(ui.clip_rect());
                        for label in ["Check profile", "Launch RetroArch", "Technical details"] {
                            ui.allocate_space(egui::vec2(100.0, 700.0));
                            let response = ui.button(label);
                            if response.clicked() {
                                clicked.borrow_mut().push(label);
                            }
                            ids.borrow_mut().push((response.id, response.rect));
                        }
                    });
                });
            },
        );
    };
    let down = Command::Move(Direction::Down);
    let up = Command::Move(Direction::Up);
    let settled = |command| {
        run(Some(command));
        for _ in 0..5 {
            run(None);
        }
        let focused = context
            .memory(|m| m.focused())
            .expect("a control has focus");
        let (id, rect) = ids
            .borrow()
            .iter()
            .copied()
            .find(|(id, _)| *id == focused)
            .unwrap();
        assert!(
            clip.get().contains_rect(rect.shrink(1.0)),
            "{rect:?} off {:?}",
            clip.get()
        );
        ids.borrow()
            .iter()
            .position(|(other, _)| *other == id)
            .unwrap()
    };
    run(None);
    assert_eq!(context.memory(|m| m.focused()), None);
    assert_eq!(settled(down), 0);
    assert_eq!(settled(down), 1);
    assert_eq!(settled(down), 2);
    assert_eq!(settled(up), 1);
    assert!(clicked.borrow().is_empty(), "moving clicks nothing");
    run(Some(Command::Select));
    run(None);
    assert_eq!(*clicked.borrow(), ["Launch RetroArch"]);
}

#[test]
fn input_held_or_pressed_while_a_game_runs_never_reaches_the_browser() {
    let context = egui::Context::default();
    let mut app = opened(&context, 10);
    tap(&context, &mut app, START);
    let id = match state(&app).step() {
        Step::Launch(id) => id,
        other => panic!("{other:?}"),
    };
    app.big_picture.as_mut().unwrap().set_connected();
    app.big_picture.as_mut().unwrap().set_launch_active(true);
    let strings = text(&settle(&context, &mut app));
    assert!(strings.iter().any(|s| s.contains("ignores the controller")));
    let focused = context.memory(|m| m.focused());
    tap(&context, &mut app, DOWN);
    tap(&context, &mut app, EAST);
    tap(&context, &mut app, SOUTH);
    assert_eq!(
        state(&app).step(),
        Step::Launch(id),
        "the game's input moved EmuWiz"
    );
    assert_eq!(context.memory(|m| m.focused()), focused);

    // The player holds A as the emulator closes.
    app.big_picture.as_mut().unwrap().inject(PadEvent::Pressed {
        device: 1,
        control: SOUTH,
    });
    settle(&context, &mut app);
    app.big_picture.as_mut().unwrap().set_launch_active(false);
    settle(&context, &mut app);
    assert_eq!(state(&app).step(), Step::Browse, "back at the browser");
    assert_eq!(state(&app).focused_game(), Some(id), "on the same game");
    assert!(state(&app).notice_text().unwrap().contains("has closed"));
    settle(&context, &mut app);
    assert_eq!(
        state(&app).step(),
        Step::Browse,
        "the held button did not reselect"
    );
}

#[test]
fn input_is_ignored_during_the_grace_period_after_an_emulator_exits() {
    let context = egui::Context::default();
    let mut app = opened(&context, 10);
    tap(&context, &mut app, START);
    app.big_picture.as_mut().unwrap().set_launch_active(true);
    settle(&context, &mut app);
    app.big_picture.as_mut().unwrap().set_launch_active(false);
    settle(&context, &mut app);
    tap(&context, &mut app, SOUTH);
    assert_eq!(state(&app).step(), Step::Browse);
}

#[test]
fn an_unfocused_window_ignores_the_controller() {
    let context = egui::Context::default();
    let mut app = opened(&context, 10);
    let state_mut = app.big_picture.as_mut().unwrap();
    state_mut.inject(PadEvent::Pressed {
        device: 1,
        control: RIGHT,
    });
    let _ = context.run(
        egui::RawInput {
            focused: false,
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE.into())),
            ..Default::default()
        },
        |context| app.show(context),
    );
    assert_eq!(state(&app).focus(), 0);
}

#[test]
fn a_missing_game_is_labelled_and_a_recording_cannot_be_launched() {
    let context = egui::Context::default();
    let mut missing = archive(1, "Lost Game", Some("SNES"));
    missing.last_verified_missing_at = Some("2026-11-01".into());
    let mut recording = archive(2, "Recording", Some("ZX Spectrum"));
    recording.archive_kind = "preservation_recording".into();
    let mut app = fixture(&context);
    app.library = Arc::new(Library::new(vec![missing, recording]));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.loaded = true;
    assert!(app.library.games[1].is_preservation_only(), "fixture kind");
    app.enter_big_picture(&context);
    let strings = text(&settle(&context, &mut app));
    assert!(
        strings.iter().any(|s| s.contains("Missing file")),
        "{strings:?}"
    );
    assert!(strings.iter().any(|s| s.contains("Not playable")));
    tap(&context, &mut app, RIGHT);
    tap(&context, &mut app, START);
    assert_eq!(state(&app).step(), Step::Browse);
    assert!(
        state(&app)
            .notice_text()
            .unwrap()
            .contains("Play is not offered")
    );
    let strings = text(&tap(&context, &mut app, SOUTH));
    assert!(matches!(state(&app).step(), Step::Game(2)));
    assert!(!strings.iter().any(|s| s == "Play"));
    assert!(strings.iter().any(|s| s == "Game details"));
    tap(&context, &mut app, START);
    assert_eq!(state(&app).step(), Step::Game(2));
}

#[test]
fn game_details_leaves_for_the_desktop_page_instead_of_stranding_the_controller() {
    let context = egui::Context::default();
    let mut app = opened(&context, 3);
    tap(&context, &mut app, SOUTH);
    tap(&context, &mut app, RIGHT);
    tap(&context, &mut app, SOUTH);
    assert!(app.big_picture.is_none());
    assert_eq!(app.router.current, Route::Game(1));
}

#[test]
fn loading_and_empty_states_are_said_plainly() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.loaded = false;
    app.enter_big_picture(&context);
    let strings = text(&settle(&context, &mut app));
    assert!(strings.iter().any(|s| s.contains("Loading your game list")));
    app.loaded = true;
    let strings = text(&settle(&context, &mut app));
    assert!(
        strings
            .iter()
            .any(|s| s.contains("No games have been added"))
    );
    tap(&context, &mut app, SOUTH);
    tap(&context, &mut app, START);
    assert_eq!(state(&app).step(), Step::Browse, "nothing to select");
}

#[test]
fn on_screen_keyboard_searches_and_keeps_keyboard_typing() {
    let context = egui::Context::default();
    let mut app = opened(&context, 30);
    tap(&context, &mut app, WEST);
    assert!(state(&app).search_open());
    // A, then Right x9, Down... : press the first key.
    tap(&context, &mut app, SOUTH);
    assert_eq!(app.filter.search, "A");
    tap(&context, &mut app, RIGHT);
    tap(&context, &mut app, SOUTH);
    assert_eq!(app.filter.search, "AB");
    tap(&context, &mut app, EAST);
    assert!(!state(&app).search_open());
    assert_eq!(app.filter.search, "AB", "Back keeps what was typed");
    app.filter.search.clear();
    tap(&context, &mut app, WEST);
    let _ = frame_with_events(
        &context,
        &mut app,
        SIZE,
        vec![egui::Event::Text("game 000012".into())],
    );
    assert_eq!(app.filter.search, "game 000012");
    let strings = text(&settle(&context, &mut app));
    assert!(strings.iter().any(|s| s == "Game 000012"), "{strings:?}");
    assert!(strings.iter().any(|s| s.contains("1 game")));
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::Enter));
    settle(&context, &mut app);
    assert!(!state(&app).search_open());
    assert_eq!(
        state(&app).focused_game(),
        Some(12),
        "focus follows the list"
    );
}

#[test]
fn system_filter_uses_the_library_filter_and_cancel_changes_nothing() {
    let context = egui::Context::default();
    let mut app = opened(&context, 30);
    tap(&context, &mut app, NORTH);
    assert!(state(&app).platforms_open());
    tap(&context, &mut app, DOWN);
    tap(&context, &mut app, EAST);
    assert!(!state(&app).platforms_open());
    assert_eq!(app.filter.platform, "");
    tap(&context, &mut app, NORTH);
    tap(&context, &mut app, DOWN);
    tap(&context, &mut app, DOWN);
    let strings = text(&tap(&context, &mut app, SOUTH));
    assert_eq!(app.filter.platform, "SNES");
    assert!(
        strings.iter().any(|s| s.contains("15 games")),
        "{strings:?}"
    );
    assert!(strings.iter().all(|s| !s.contains("Game 000001")));
}

#[test]
fn a_filter_that_keeps_the_focused_game_keeps_focus_on_it() {
    let context = egui::Context::default();
    let mut app = opened(&context, 30);
    tap(&context, &mut app, RIGHT);
    assert_eq!(state(&app).focused_game(), Some(2));
    app.filter.select_platform("SNES".into());
    settle(&context, &mut app);
    assert_eq!(state(&app).focused_game(), Some(2));
    assert_eq!(state(&app).focus(), 0);
}

#[test]
fn a_game_removed_by_a_rescan_returns_to_the_browser_with_a_reason() {
    let context = egui::Context::default();
    let mut app = opened(&context, 3);
    tap(&context, &mut app, SOUTH);
    app.library = Arc::new(Library::new(vec![archive(9, "Other", Some("SNES"))]));
    settle(&context, &mut app);
    assert_eq!(state(&app).step(), Step::Browse);
    assert!(state(&app).notice_text().unwrap().contains("no longer"));
}

#[test]
fn a_133k_library_stays_fast_through_the_existing_filter_worker() {
    let context = egui::Context::default();
    let mut app = opened(&context, 133_000);
    // Large catalogues go through the debounced worker, as in the Library.
    app.indices = (0..133_000).collect();
    app.filter_dirty = None;
    app.filter_inflight = false;
    settle(&context, &mut app);
    let start = std::time::Instant::now();
    for _ in 0..200 {
        tap(&context, &mut app, DOWN);
    }
    let per_step = start.elapsed() / 400;
    assert_eq!(state(&app).focus(), 200 * state(&app).columns());
    let strings = text(&frame(&context, &mut app, SIZE));
    assert!(strings.iter().filter(|s| s.starts_with("Game ")).count() <= 24);
    println!("BIG_PICTURE_FRAME {per_step:?}");
    assert!(
        per_step < std::time::Duration::from_millis(100),
        "a frame took {per_step:?}"
    );
}

#[test]
fn closing_stops_at_a_real_exit_of_full_screen() {
    let context = egui::Context::default();
    let mut app = opened(&context, 3);
    app.big_picture.as_mut().unwrap().set_connected();
    tap(&context, &mut app, EAST);
    let pad = app.big_picture.as_mut().unwrap();
    pad.inject(PadEvent::Pressed {
        device: 1,
        control: EAST,
    });
    let output = frame(&context, &mut app, SIZE);
    assert!(app.big_picture.is_none());
    let commands = &output.viewport_output.values().next().unwrap().commands;
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::Fullscreen(false)))
    );
}

#[test]
fn the_grid_fills_a_full_hd_screen_instead_of_leaving_half_of_it_empty() {
    let context = egui::Context::default();
    let mut app = opened(&context, 100);
    let strings = text(&settle(&context, &mut app));
    let columns = state(&app).columns();
    let tiles = strings.iter().filter(|s| s.starts_with("Game 0")).count();
    assert!(tiles >= columns * 2, "{tiles} tiles, {columns} columns");
}

#[test]
fn nothing_claims_a_game_count_before_the_library_has_loaded() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.loaded = false;
    app.enter_big_picture(&context);
    let strings = text(&settle(&context, &mut app));
    assert!(
        !strings.iter().any(|s| s.contains("0 games")),
        "{strings:?}"
    );
}

fn key_event(k: egui::Key, pressed: bool, repeat: bool) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: Some(k),
        pressed,
        repeat,
        modifiers: egui::Modifiers::NONE,
    }
}

/// A real keyboard sends the letter as text in the same frame as the key.
#[test]
fn the_key_that_opens_search_is_not_typed_into_it() {
    let context = egui::Context::default();
    let mut app = opened(&context, 30);
    for (k, typed) in [(egui::Key::S, "s"), (egui::Key::Slash, "/")] {
        let _ = frame_with_events(
            &context,
            &mut app,
            SIZE,
            vec![
                key_event(k, true, false),
                egui::Event::Text(typed.into()),
                key_event(k, false, false),
            ],
        );
        settle(&context, &mut app);
        assert!(state(&app).search_open(), "{typed} opens search");
        assert_eq!(app.filter.search, "", "{typed} is not typed into it");
        let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::Escape));
        assert!(!state(&app).search_open());
    }
    // Typing once search is open still works.
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::S));
    let _ = frame_with_events(
        &context,
        &mut app,
        SIZE,
        vec![egui::Event::Text("s".into())],
    );
    assert_eq!(app.filter.search, "s");
}

/// The key that closed the emulator (often Esc) may still be held, and
/// autorepeating, when EmuWiz gets focus back.
#[test]
fn keyboard_input_while_a_game_runs_or_in_the_grace_period_does_nothing() {
    let context = egui::Context::default();
    let mut app = opened(&context, 10);
    tap(&context, &mut app, START);
    let launch = state(&app).step();
    assert!(matches!(launch, Step::Launch(_)));
    app.big_picture.as_mut().unwrap().set_launch_active(true);
    settle(&context, &mut app);
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::Escape));
    assert_eq!(state(&app).step(), launch, "a running game keeps the page");
    app.big_picture.as_mut().unwrap().set_launch_active(false);
    settle(&context, &mut app);
    assert_eq!(state(&app).step(), Step::Browse);
    for _ in 0..3 {
        let _ = frame_with_events(
            &context,
            &mut app,
            SIZE,
            vec![key_event(egui::Key::Escape, true, true)],
        );
    }
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::Enter));
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::ArrowRight));
    assert!(app.big_picture.is_some(), "a held Esc did not leave");
    assert_eq!(state(&app).step(), Step::Browse, "Enter did not reselect");
    assert_eq!(state(&app).focus(), 0, "arrows did not move");
    assert!(
        !state(&app)
            .notice_text()
            .is_some_and(|notice| notice.contains("again")),
        "leaving was not armed"
    );
}

/// A held key autorepeats (egui marks every press after the first as a
/// repeat). Only movement repeats, as on the controller: holding Esc or Enter
/// acts once, so it cannot leave Big Picture or walk into the Launch page.
#[test]
fn a_held_back_or_select_key_acts_once() {
    let context = egui::Context::default();
    let mut app = opened(&context, 10);
    for _ in 0..3 {
        let _ = frame_with_events(
            &context,
            &mut app,
            SIZE,
            vec![key_event(egui::Key::Escape, true, false)],
        );
    }
    assert!(app.big_picture.is_some(), "a held Esc only arms leaving");
    assert!(state(&app).notice_text().unwrap().contains("again"));
    let _ = frame_with_events(
        &context,
        &mut app,
        SIZE,
        vec![key_event(egui::Key::Escape, false, false)],
    );
    for _ in 0..3 {
        let _ = frame_with_events(
            &context,
            &mut app,
            SIZE,
            vec![key_event(egui::Key::Enter, true, false)],
        );
    }
    assert!(
        matches!(state(&app).step(), Step::Game(_)),
        "a held Enter opens the game and stops there"
    );
    let _ = frame_with_events(
        &context,
        &mut app,
        SIZE,
        vec![key_event(egui::Key::Enter, false, false)],
    );
    let _ = frame_with_events(&context, &mut app, SIZE, key(egui::Key::Escape));
    assert_eq!(state(&app).step(), Step::Browse);
    for _ in 0..3 {
        let _ = frame_with_events(
            &context,
            &mut app,
            SIZE,
            vec![key_event(egui::Key::ArrowRight, true, false)],
        );
    }
    assert_eq!(state(&app).focus(), 3, "a held arrow still moves");
}
