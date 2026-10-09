//! Simple v1 shell, rendered through the real `App::show`: the opt-in changes
//! navigation chrome only, and every existing location stays reachable.
use super::*;
use crate::gui_v2::simple_shell::{DESTINATIONS, Destination, palette};

const DESKTOP: [f32; 2] = [1672.0, 941.0];
const SMALL: [f32; 2] = [1024.0, 640.0];

fn simple(context: &egui::Context) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app
}

fn shown(context: &egui::Context, app: &mut App, size: [f32; 2]) -> Vec<String> {
    // Two frames: the first applies the style, the second is settled.
    frame(context, app, size);
    text(&frame(context, app, size))
}

fn has(strings: &[String], wanted: &str) -> bool {
    strings.iter().any(|value| value == wanted)
}

#[test]
fn the_default_interface_is_unchanged_until_simple_is_turned_on() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let strings = shown(&context, &mut app, DESKTOP);
    // The full sidebar and the classic header are still there...
    assert!(has(&strings, "Browse & Play"));
    assert!(has(&strings, "Jump to…"));
    assert!(has(&strings, "Setup & Doctor"));
    // ...and none of the Simple chrome is.
    assert!(!has(&strings, "All Tools"));
    assert!(!has(&strings, "Play More. Manage Better."));
    assert_eq!(
        context.style().visuals.panel_fill,
        crate::ui::theme::APP_BACKGROUND
    );
}

#[test]
fn simple_shows_exactly_the_seven_destinations_and_all_tools() {
    for size in [DESKTOP, SMALL] {
        let context = egui::Context::default();
        let mut app = simple(&context);
        let strings = shown(&context, &mut app, size);
        for destination in DESTINATIONS {
            assert_eq!(
                strings
                    .iter()
                    .filter(|value| *value == destination.label())
                    .count()
                    .min(1),
                1,
                "{} missing at {size:?}",
                destination.label()
            );
        }
        assert!(has(&strings, "All Tools"));
        assert!(has(&strings, "Play More. Manage Better."));
        assert!(strings.iter().any(|value| value.starts_with("EmuWiz v")));
        // The classic chrome is not repeated beside it.
        for gone in [
            "Jump to…",
            "Back",
            "Tab: move focus\nEnter: open\nAlt+Left: back",
        ] {
            assert!(!has(&strings, gone), "{gone} still shown at {size:?}");
        }
        assert_eq!(context.style().visuals.panel_fill, palette::APP_BACKGROUND);
    }
}

#[test]
fn every_destination_opens_its_existing_route_from_the_sidebar() {
    for destination in DESTINATIONS {
        let context = egui::Context::default();
        let mut app = simple(&context);
        // Start somewhere else so the click is what moves us.
        app.router.current = if destination == Destination::Home {
            Route::Section(Section::Settings)
        } else {
            Route::Home
        };
        frame(&context, &mut app, DESKTOP);
        click_label(&context, &mut app, DESKTOP, destination.label());
        assert_eq!(
            app.router.current,
            destination.route(),
            "{}",
            destination.label()
        );
    }
}

#[test]
fn all_tools_restores_the_full_navigation_and_simple_view_returns() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    app.router.current = Route::Section(Section::Check);
    frame(&context, &mut app, DESKTOP);

    // `click_label` skips the header band, so click All Tools directly.
    let layout = frame(&context, &mut app, DESKTOP);
    let point = text_bounds(&layout, "All Tools")[0].center();
    click_at(&context, &mut app, DESKTOP, point);
    assert!(app.simple.all_tools);
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Setup & Doctor") && has(&strings, "Jump to…"));
    assert!(has(&strings, "Simple view"));
    // The location is kept across the switch, in both directions.
    assert_eq!(app.router.current, Route::Section(Section::Check));

    let layout = frame(&context, &mut app, DESKTOP);
    let point = text_bounds(&layout, "Simple view")[0].center();
    click_at(&context, &mut app, DESKTOP, point);
    assert!(!app.simple.all_tools);
    assert_eq!(app.router.current, Route::Section(Section::Check));
    assert!(has(&shown(&context, &mut app, DESKTOP), "All Tools"));
}

fn click_at(context: &egui::Context, app: &mut App, size: [f32; 2], point: egui::Pos2) {
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

#[test]
fn an_all_tools_only_location_highlights_nothing_and_says_where_you_are() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    app.router.current = Route::Section(Section::Dat);
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, "You are in All Tools: DAT Management"));
}

#[test]
fn breadcrumbs_sit_under_the_header_only_on_contextual_pages() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    app.library = Arc::new(Library::new(vec![archive(7, "Trail Game", Some("SNES"))]));
    app.router.current = Route::Home;
    let home = shown(&context, &mut app, DESKTOP);
    assert!(!has(&home, "‹ Back") && !has(&home, "/"));

    app.router.go(Route::Game(7));
    let layout = {
        frame(&context, &mut app, DESKTOP);
        frame(&context, &mut app, DESKTOP)
    };
    let strings = text(&layout);
    assert!(has(&strings, "‹ Back"));
    assert!(has(&strings, "Trail Game"));
    // Below the header band, inside the page column.
    let back = text_bounds(&layout, "‹ Back")[0];
    assert!(back.min.y >= crate::gui_v2::simple_shell::HEADER_HEIGHT);
    assert!(back.min.x >= crate::gui_v2::simple_shell::SIDEBAR_WIDTH);

    click_at(&context, &mut app, DESKTOP, back.center());
    assert_eq!(app.router.current, Route::Home);
}

#[test]
fn header_search_feeds_the_existing_library_filter_and_enter_opens_the_library() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    app.router.current = Route::Section(Section::Settings);
    frame(&context, &mut app, DESKTOP);
    let before = app.filter_generation;
    let layout = frame(&context, &mut app, DESKTOP);
    let hint = text_bounds(&layout, "Search your games or systems…")[0];
    // On the left of the header, as in the reference.
    assert!(hint.min.y < crate::gui_v2::simple_shell::HEADER_HEIGHT);
    assert!(hint.min.x < DESKTOP[0] / 2.0);
    click_at(&context, &mut app, DESKTOP, hint.center());
    let typed = |events: Vec<egui::Event>, app: &mut App| {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(DESKTOP[0], DESKTOP[1]),
                )),
                events,
                ..Default::default()
            },
            |context| app.show(context),
        );
    };
    typed(vec![egui::Event::Text("zelda".into())], &mut app);
    assert_eq!(app.filter.search, "zelda");
    assert!(app.filter_generation > before, "the real filter was told");
    assert_eq!(app.router.current, Route::Section(Section::Settings));
    for pressed in [true, false] {
        typed(
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut app,
        );
    }
    assert_eq!(app.router.current, Route::BrowsePlay);
}

#[test]
fn keyboard_reaches_the_sidebar_and_enter_activates_it() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    app.router.current = Route::Section(Section::Settings);
    frame(&context, &mut app, DESKTOP);
    let key = |key: egui::Key, app: &mut App| {
        for pressed in [true, false] {
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(DESKTOP[0], DESKTOP[1]),
                    )),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |context| app.show(context),
            );
        }
    };
    // Tab until Enter lands on a sidebar destination other than Settings.
    let mut reached = false;
    for _ in 0..12 {
        key(egui::Key::Tab, &mut app);
        key(egui::Key::Enter, &mut app);
        if DESTINATIONS
            .iter()
            .any(|d| d.route() == app.router.current && *d != Destination::Settings)
        {
            reached = true;
            break;
        }
    }
    assert!(reached, "no sidebar destination is keyboard reachable");
}

#[test]
fn the_shell_starts_no_work_the_classic_interface_would_not() {
    // Some existing pages start read-only background work when opened. The
    // shell must add none: the same walk yields the same jobs either way.
    let walk = |enabled: bool| {
        let context = egui::Context::default();
        let mut app = fixture(&context);
        app.simple.enabled = enabled;
        app.library = Arc::new(Library::new(vec![archive(1, "Quiet Game", Some("SNES"))]));
        for destination in DESTINATIONS {
            app.router.current = destination.route();
            frame(&context, &mut app, DESKTOP);
            frame(&context, &mut app, SMALL);
        }
        assert!(app.repair_job.is_none() && app.undo_job.is_none());
        assert!(app.verification_job.is_none() && app.duplicate_job.is_none());
        let mut titles: Vec<_> = app
            .activity
            .jobs
            .values()
            .map(|job| job.title.clone())
            .collect();
        titles.sort();
        titles
    };
    assert_eq!(walk(true), walk(false));
}

#[test]
fn the_setting_turns_the_shell_on_and_is_saved_with_the_scale() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("gui-v2.json");
    backend::save_preferences(
        &path,
        &Preferences {
            simple_shell: true,
            ui_scale: 1.2,
            ..Default::default()
        },
    )
    .unwrap();
    let saved: Preferences =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(saved.simple_shell);
    assert!((saved.ui_scale - 1.2).abs() < 1e-6);
    // A file written before this setting existed still loads, with Simple off.
    let old: Preferences = serde_json::from_str(r#"{"welcome_dismissed":true}"#).unwrap();
    assert!(!old.simple_shell);
    assert_eq!(old.ui_scale, 1.0);

    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Settings);
    frame(&context, &mut app, DESKTOP);
    click_label(&context, &mut app, DESKTOP, "Use the Simple view");
    assert!(app.simple.enabled);
    assert!(app.preferences_dirty.is_some());
}

#[test]
fn manage_library_is_a_hub_of_five_cards_that_open_existing_routes() {
    use crate::gui_v2::simple_shell::MANAGE_TASKS;
    for size in [DESKTOP, SMALL] {
        let context = egui::Context::default();
        let mut app = simple(&context);
        app.router.current = Destination::ManageLibrary.route();
        let strings = shown(&context, &mut app, size);
        assert!(has(&strings, "Manage Library"));
        for task in MANAGE_TASKS {
            assert!(has(&strings, task.title), "{} at {size:?}", task.title);
        }
        // The Organisation overview is not what Simple shows here.
        assert!(!has(&strings, "Organisation"));
    }
    for task in MANAGE_TASKS {
        let context = egui::Context::default();
        let mut app = simple(&context);
        app.router.current = Destination::ManageLibrary.route();
        frame(&context, &mut app, DESKTOP);
        click_label(&context, &mut app, DESKTOP, task.title);
        assert_eq!(
            app.router.current,
            Route::Section(task.section),
            "{}",
            task.title
        );
        // Still inside Manage Library, and Back returns to the hub.
        app.back();
        assert_eq!(app.router.current, Destination::ManageLibrary.route());
    }
}

#[test]
fn all_tools_still_shows_the_organisation_overview_at_that_location() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    app.simple.all_tools = true;
    app.router.current = Destination::ManageLibrary.route();
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Organisation"));
    assert!(!has(&strings, "Check & Identify"));

    // And with Simple off entirely the page is exactly the existing one.
    let context = egui::Context::default();
    let mut classic = fixture(&context);
    classic.router.current = Destination::ManageLibrary.route();
    let strings = shown(&context, &mut classic, DESKTOP);
    assert!(has(&strings, "Organisation") && !has(&strings, "Manage Library"));
}

#[test]
fn manage_library_cards_are_keyboard_reachable() {
    use crate::gui_v2::simple_shell::MANAGE_TASKS;
    // Tab k times from a fresh window, then press Enter once. Every card must
    // be the target for some k, in order.
    let mut reached = Vec::new();
    for tabs in 1..=30 {
        // (Focus order: activity bar, header, sidebar, then the cards.)
        let context = egui::Context::default();
        let mut app = simple(&context);
        app.router.current = Destination::ManageLibrary.route();
        frame(&context, &mut app, DESKTOP);
        let key = |key: egui::Key, app: &mut App| {
            for pressed in [true, false] {
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(DESKTOP[0], DESKTOP[1]),
                        )),
                        events: vec![egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                        ..Default::default()
                    },
                    |context| app.show(context),
                );
            }
        };
        for _ in 0..tabs {
            key(egui::Key::Tab, &mut app);
        }
        key(egui::Key::Enter, &mut app);
        if let Some(task) = MANAGE_TASKS
            .iter()
            .find(|task| app.router.current == Route::Section(task.section))
        {
            reached.push(task.title);
        }
    }
    let expected: Vec<_> = MANAGE_TASKS.iter().map(|task| task.title).collect();
    // Thirty tabs can wrap the focus ring, so the first pass is what counts.
    assert_eq!(
        reached.get(..expected.len()),
        Some(expected.as_slice()),
        "cards reached by Tab then Enter"
    );
}

// ---- review corrections: zoomed navigation, palette lifecycle, exact classic ----

/// The window size in points for a 1024x640 pixel window at `scale`.
fn small_window_at(scale: f32) -> [f32; 2] {
    [1024.0 / scale, 640.0 / scale]
}

fn scaled(context: &egui::Context, scale: f32) -> App {
    let mut app = simple(context);
    app.simple.set_ui_scale(scale);
    app
}

fn key_press(context: &egui::Context, app: &mut App, size: [f32; 2], key: egui::Key) {
    for pressed in [true, false] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |context| app.show(context),
        );
    }
}

#[test]
fn every_destination_is_keyboard_reachable_and_shown_when_focused_at_every_scale() {
    use crate::gui_v2::simple_shell::nav_id;
    for scale in [0.8, 1.0, 1.25, 1.6] {
        let size = small_window_at(scale);
        let context = egui::Context::default();
        let mut app = scaled(&context, scale);
        // Somewhere no sidebar row opens, so any arrival is from the keyboard.
        app.router.current = Route::Section(Section::Tape);
        frame(&context, &mut app, size);
        let mut reached = Vec::new();
        // Tab through the window. Enter is pressed only while a sidebar row
        // holds the focus, so no control on the page behind is activated.
        for _ in 0..24 {
            key_press(&context, &mut app, size, egui::Key::Tab);
            let focused = context.memory(|memory| memory.focused());
            let Some(destination) = DESTINATIONS
                .iter()
                .copied()
                .find(|destination| focused == Some(nav_id(*destination)))
            else {
                continue;
            };
            if reached.contains(&destination.label()) {
                break;
            }
            // The focused row must be on screen, inside the sidebar.
            let layout = frame(&context, &mut app, size);
            let row = text_bounds(&layout, destination.label())
                .into_iter()
                .find(|rect| rect.min.x < 100.0)
                .unwrap_or_else(|| {
                    panic!("{} was focused off screen at {scale}", destination.label())
                });
            assert!(
                row.min.y >= 0.0 && row.max.y <= size[1],
                "{} focused outside the window at {scale}",
                destination.label()
            );
            key_press(&context, &mut app, size, egui::Key::Enter);
            assert_eq!(
                app.router.current,
                destination.route(),
                "Enter on {} at {scale}",
                destination.label()
            );
            reached.push(destination.label());
        }
        let expected: Vec<_> = DESTINATIONS.iter().map(|d| d.label()).collect();
        assert_eq!(reached, expected, "keyboard reach at scale {scale}");
    }
}

#[test]
fn every_destination_is_mouse_reachable_by_scrolling_the_sidebar_at_every_scale() {
    for scale in [0.8, 1.0, 1.25, 1.6] {
        let size = small_window_at(scale);
        for destination in DESTINATIONS {
            let context = egui::Context::default();
            let mut app = scaled(&context, scale);
            app.router.current = Route::Section(Section::Tape);
            frame(&context, &mut app, size);
            let over_sidebar = egui::pos2(60.0, size[1] * 0.6);
            // Click the row if it is showing; otherwise wheel down over the
            // sidebar and look again, as a person would.
            for _ in 0..14 {
                let bounds = text_bounds(&frame(&context, &mut app, size), destination.label());
                if let Some(row) = bounds.iter().find(|rect| rect.min.x < 100.0) {
                    click_at(&context, &mut app, size, row.center());
                    if app.router.current == destination.route() {
                        break;
                    }
                }
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size[0], size[1]),
                        )),
                        events: vec![
                            egui::Event::PointerMoved(over_sidebar),
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: egui::vec2(0.0, -40.0),
                                phase: egui::TouchPhase::Move,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |context| app.show(context),
                );
                frame(&context, &mut app, size);
            }
            assert_eq!(
                app.router.current,
                destination.route(),
                "{} not reachable by mouse at {scale}",
                destination.label()
            );
        }
    }
}

#[test]
fn sidebar_labels_are_never_cut_off_by_the_sidebar_edge() {
    use crate::gui_v2::simple_shell::{SIDEBAR_WIDTH, SIDEBAR_WIDTH_NARROW};
    for scale in [0.8, 1.0, 1.25, 1.6] {
        for window in [[1024.0, 640.0], [1672.0, 941.0]] {
            let size = [window[0] / scale, window[1] / scale];
            let context = egui::Context::default();
            let mut app = scaled(&context, scale);
            frame(&context, &mut app, size);
            let layout = frame(&context, &mut app, size);
            let edge = if size[0] < 900.0 {
                SIDEBAR_WIDTH_NARROW
            } else {
                SIDEBAR_WIDTH
            };
            for destination in DESTINATIONS {
                for rect in text_bounds(&layout, destination.label()) {
                    // Sidebar rows only; the page may repeat a label.
                    if rect.min.x < 100.0 {
                        assert!(
                            rect.max.x <= edge - 6.0,
                            "{} runs to {} past the {edge} sidebar at {scale}, {window:?}",
                            destination.label(),
                            rect.max.x
                        );
                    }
                }
            }
        }
    }
}

fn blue(context: &egui::Context) -> bool {
    let style = context.style();
    style.visuals.panel_fill == palette::APP_BACKGROUND
        && style.visuals.faint_bg_color == palette::CARD_SURFACE
        && style.text_styles[&egui::TextStyle::Heading].size
            == crate::gui_v2::simple_shell::PAGE_TITLE_SIZE
}

#[test]
fn the_blue_profile_survives_every_route_including_embedded_workflows() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    let mut visit = |app: &mut App, route: Route| {
        app.router.current = route.clone();
        frame(&context, app, DESKTOP);
        frame(&context, app, DESKTOP);
        assert!(blue(&context), "palette lost on {route:?}");
    };
    visit(&mut app, Destination::ManageLibrary.route());
    // Cheats & Mods builds the embedded workflow host, which restyles the context.
    visit(&mut app, Destination::CheatsMods.route());
    visit(&mut app, Route::Home);
    for section in crate::gui_v2::routes::SECTIONS {
        visit(&mut app, Route::Section(*section));
        visit(&mut app, Route::Home);
    }
    // All Tools and back keep it too.
    app.simple.all_tools = true;
    visit(&mut app, Destination::CheatsMods.route());
    app.simple.all_tools = false;
    visit(&mut app, Route::Home);
    // Something else restyling the context mid-session is corrected next frame.
    readable_style(&context);
    visit(&mut app, Route::Home);
}

/// A context styled exactly as a fresh classic GUI v2 window.
fn fresh_classic() -> egui::Style {
    let context = egui::Context::default();
    readable_style(&context);
    (*context.style()).clone()
}

/// Asserts `context` carries the complete classic style: every field of a
/// fresh classic window. egui compares a style's number formatter by pointer,
/// and each context makes its own, so that one slot is aligned first.
#[track_caller]
fn assert_classic(context: &egui::Context, classic: &egui::Style, when: &str) {
    let mut expected = classic.clone();
    expected.number_formatter = context.style().number_formatter.clone();
    assert_eq!(*context.style(), expected, "{when}");
}

#[test]
fn turning_simple_off_restores_the_complete_classic_style() {
    let classic = fresh_classic();
    let context = egui::Context::default();
    let mut app = fixture(&context);
    frame(&context, &mut app, DESKTOP);
    assert_classic(&context, &classic, "default-off start");

    for round in 0..4 {
        app.simple.enabled = true;
        app.simple.set_ui_scale(1.0 + 0.2 * round as f32);
        // Visit pages in both modes, including the one that restyles the context.
        for route in [
            Route::Home,
            Destination::CheatsMods.route(),
            Destination::ManageLibrary.route(),
            Route::Section(Section::Settings),
        ] {
            app.router.current = route;
            frame(&context, &mut app, DESKTOP);
            frame(&context, &mut app, DESKTOP);
            assert!(blue(&context), "round {round}");
        }
        app.simple.enabled = false;
        for route in [
            Route::Section(Section::Settings),
            Destination::CheatsMods.route(),
            Route::Home,
        ] {
            app.router.current = route;
            frame(&context, &mut app, DESKTOP);
            frame(&context, &mut app, DESKTOP);
            assert_classic(
                &context,
                &classic,
                &format!("round {round}: classic not restored"),
            );
            assert_eq!(context.zoom_factor(), 1.0);
        }
    }
}

#[test]
fn a_session_restored_with_simple_on_can_still_return_to_exact_classic() {
    // On restart the saved preference arrives after the window was styled.
    let classic = fresh_classic();
    let context = egui::Context::default();
    let mut app = fixture(&context);
    frame(&context, &mut app, DESKTOP);
    app.simple.enabled = true;
    app.simple.set_ui_scale(1.6);
    frame(&context, &mut app, DESKTOP);
    frame(&context, &mut app, DESKTOP);
    assert!(blue(&context));
    assert!((context.zoom_factor() - 1.6).abs() < 1e-6);
    app.simple.enabled = false;
    frame(&context, &mut app, DESKTOP);
    frame(&context, &mut app, DESKTOP);
    assert_classic(
        &context,
        &classic,
        "after a restored Simple session is turned off",
    );
    assert_eq!(context.zoom_factor(), 1.0);
}
