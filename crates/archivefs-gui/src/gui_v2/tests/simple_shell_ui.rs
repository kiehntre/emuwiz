//! EmuWiz Blue shell, rendered through the real `App::show`: the standard
//! view changes navigation chrome only, every existing location stays
//! reachable, and the Classic view remains an explicit fallback.
use super::*;
use crate::gui_v2::simple_shell::{DESTINATIONS, Destination, palette};

const DESKTOP: [f32; 2] = [1672.0, 941.0];
const SMALL: [f32; 2] = [1024.0, 640.0];

fn simple(context: &egui::Context) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app
}

/// Opens `destination` the way its sidebar row does: its route, or the All
/// Tools browser for Tools.
fn open(app: &mut App, destination: Destination) {
    match destination.route() {
        Some(route) => {
            app.simple.all_tools = false;
            app.router.current = route;
        }
        None => app.simple.all_tools = true,
    }
}

/// Whether `destination` is what the person is looking at.
fn is_open(app: &App, destination: Destination) -> bool {
    match destination.route() {
        Some(route) => !app.simple.all_tools_open() && app.router.current == route,
        None => app.simple.all_tools_open(),
    }
}

/// The sidebar row of `destination`, as egui last laid it out.
fn row_rect(context: &egui::Context, destination: Destination) -> egui::Rect {
    context
        .read_response(crate::gui_v2::simple_shell::nav_id(destination))
        .unwrap_or_else(|| panic!("{} has no row", destination.label()))
        .rect
}

/// The visible part of the destination list (below the brand, above the
/// footer), as the sidebar last clipped it.
fn list_clip(context: &egui::Context) -> egui::Rect {
    context
        .data(|data| {
            data.get_temp::<egui::Rect>(crate::gui_v2::simple_shell::sidebar_list_clip_id())
        })
        .expect("the sidebar records its list clip")
}

/// Whether a window of `size` points shows the icon rail.
fn rail(size: [f32; 2]) -> bool {
    crate::gui_v2::simple_shell::shell_metrics(size[0], size[1]).rail
}

/// The row of `destination` when its whole row is visible in the list.
fn visible_row(context: &egui::Context, destination: Destination) -> Option<egui::Rect> {
    let row = row_rect(context, destination);
    list_clip(context)
        .expand(0.5)
        .contains_rect(row)
        .then_some(row)
}

/// The Classic fallback view, chosen explicitly.
fn classic_view(context: &egui::Context) -> App {
    let mut app = fixture(context);
    app.simple.enabled = false;
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
fn the_default_interface_is_emuwiz_blue_and_classic_stays_available() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    // What `App::new` installs (`simple: Default::default()`).
    app.simple = crate::gui_v2::simple_shell::ShellState::default();
    let strings = shown(&context, &mut app, DESKTOP);
    // A new window opens in EmuWiz Blue, with All Tools for everything else.
    assert!(app.simple.enabled);
    assert!(has(&strings, "All Tools"));
    assert!(!has(&strings, "Jump to…"));
    assert_eq!(context.style().visuals.panel_fill, palette::APP_BACKGROUND);

    // The Classic view, chosen explicitly, is exactly the previous interface.
    let context = egui::Context::default();
    let mut app = classic_view(&context);
    let strings = shown(&context, &mut app, DESKTOP);
    // The full sidebar and the classic header are still there...
    assert!(has(&strings, "Browse & Play"));
    assert!(has(&strings, "Jump to…"));
    assert!(has(&strings, "Setup & Doctor"));
    // ...and none of the Simple chrome is.
    assert!(!has(&strings, "All Tools"));
    assert!(!has(&strings, "Local-first."));
    assert_eq!(
        context.style().visuals.panel_fill,
        crate::ui::theme::APP_BACKGROUND
    );
}

#[test]
fn blue_shows_exactly_the_eight_destinations_and_all_tools() {
    for size in [DESKTOP, SMALL] {
        let context = egui::Context::default();
        let mut app = simple(&context);
        let strings = shown(&context, &mut app, size);
        for destination in DESTINATIONS {
            // Every row exists; on the rail it is an icon with an accessible name.
            row_rect(&context, destination);
            if rail(size) {
                continue;
            }
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
        // The brand: the name beside the badge, or the badge alone on the rail.
        assert_eq!(has(&strings, "EmuWiz"), !rail(size), "brand at {size:?}");
        // The footer on the full sidebar; the rail has none.
        assert_eq!(
            has(&strings, "Local-first.\nRead-only by default."),
            !rail(size)
        );
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
        assert!(is_open(&app, destination), "{}", destination.label());
    }
}

#[test]
fn all_tools_opens_the_browser_over_the_current_location_and_every_way_closes_it() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    app.router.current = Route::Section(Section::Check);
    frame(&context, &mut app, DESKTOP);

    // `click_label` skips the header band, so click All Tools directly.
    let layout = frame(&context, &mut app, DESKTOP);
    let point = text_bounds(&layout, "All Tools")[0].center();
    click_at(&context, &mut app, DESKTOP, point);
    assert!(app.simple.all_tools_open());
    let strings = shown(&context, &mut app, DESKTOP);
    // The grouped browser, with the Blue sidebar still in place.
    for wanted in [
        "Games & collection",
        "Verification & identification",
        "Platforms",
        "Library",
    ] {
        assert!(has(&strings, wanted), "{wanted} missing from the browser");
    }
    assert!(
        !has(&strings, "Jump to…"),
        "the Classic chrome is not swapped in"
    );
    // The location underneath is kept.
    assert_eq!(app.router.current, Route::Section(Section::Check));

    // Choosing an entry opens that existing page and closes the browser.
    let layout = frame(&context, &mut app, DESKTOP);
    let sidebar = crate::gui_v2::simple_shell::shell_metrics(DESKTOP[0], DESKTOP[1]).sidebar;
    let entry = text_bounds(&layout, "Platforms")
        .into_iter()
        .find(|rect| rect.min.x > sidebar)
        .expect("Platforms entry");
    click_at(&context, &mut app, DESKTOP, entry.center());
    assert!(!app.simple.all_tools_open());
    assert_eq!(app.router.current, Route::Section(Section::Platforms));

    // Back (Escape or Alt+Left) closes the browser and keeps the page.
    app.simple.all_tools = true;
    frame(&context, &mut app, DESKTOP);
    key_press(&context, &mut app, DESKTOP, egui::Key::Escape);
    assert!(!app.simple.all_tools_open());
    assert_eq!(app.router.current, Route::Section(Section::Platforms));

    // The Tools row opens the same browser and is highlighted while it shows.
    open(&mut app, Destination::Tools);
    assert!(is_open(&app, Destination::Tools));
    let strings = shown(&context, &mut app, DESKTOP);
    assert!(has(&strings, "Games & collection"));
    // The All Tools button closes it again.
    let layout = frame(&context, &mut app, DESKTOP);
    let point = text_bounds(&layout, "All Tools")
        .into_iter()
        .find(|rect| rect.min.y < 64.0)
        .unwrap()
        .center();
    click_at(&context, &mut app, DESKTOP, point);
    assert!(!app.simple.all_tools_open());
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
fn every_location_highlights_its_approved_row_and_activity_none() {
    use crate::gui_v2::simple_shell::destination_for;
    // A selected row is painted as the primary-blue pill over its whole row.
    let selected_rows = |context: &egui::Context, output: &egui::FullOutput| -> Vec<String> {
        DESTINATIONS
            .iter()
            .filter(|destination| {
                let row = row_rect(context, **destination);
                output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Rect(rect)
                        if rect.fill == palette::PRIMARY_ACTION
                            && (rect.rect.min - row.min).length() < 1.0
                            && (rect.rect.max - row.max).length() < 1.0)
                })
            })
            .map(|destination| destination.label().to_owned())
            .collect()
    };
    for route in [
        Route::Section(Section::Dat),
        Route::Section(Section::Museum),
        Route::Section(Section::Saves),
        Route::Section(Section::History),
        Route::Section(Section::Activity),
    ] {
        let context = egui::Context::default();
        let mut app = simple(&context);
        app.router.current = route.clone();
        frame(&context, &mut app, DESKTOP);
        let output = frame(&context, &mut app, DESKTOP);
        let expected: Vec<String> = destination_for(&route)
            .map(|d| d.label().to_owned())
            .into_iter()
            .collect();
        assert_eq!(selected_rows(&context, &output), expected, "{route:?}");
    }
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
    let metrics = crate::gui_v2::simple_shell::shell_metrics(DESKTOP[0], DESKTOP[1]);
    assert!(back.min.x >= metrics.sidebar);

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
            .any(|d| is_open(&app, *d) && *d != Destination::Settings)
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
            open(&mut app, destination);
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
    // A file written before this setting existed still loads, in Blue.
    let old: Preferences = serde_json::from_str(r#"{"welcome_dismissed":true}"#).unwrap();
    assert!(!old.classic_view);
    assert!(crate::gui_v2::simple_shell::blue_enabled(&old));
    assert_eq!(old.ui_scale, 1.0);

    // Settings switches to the Classic view and back, saving each choice.
    let context = egui::Context::default();
    let mut app = simple(&context);
    app.router.current = Route::Section(Section::Settings);
    frame(&context, &mut app, DESKTOP);
    click_label(&context, &mut app, DESKTOP, "Use the Classic view");
    assert!(!app.simple.enabled);
    assert!(app.preferences_dirty.is_some());
    app.preferences_dirty = None;
    frame(&context, &mut app, DESKTOP);
    click_label(&context, &mut app, DESKTOP, "Use the Classic view");
    assert!(app.simple.enabled);
    assert!(app.preferences_dirty.is_some());
}

#[test]
fn manage_library_is_a_hub_of_five_cards_that_open_existing_routes() {
    use crate::gui_v2::simple_shell::MANAGE_TASKS;
    for size in [DESKTOP, SMALL] {
        let context = egui::Context::default();
        let mut app = simple(&context);
        app.router.current = Destination::OrganiseExport.route().unwrap();
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
        app.router.current = Destination::OrganiseExport.route().unwrap();
        frame(&context, &mut app, DESKTOP);
        // The card, not a sidebar row that shares its name.
        let layout = frame(&context, &mut app, DESKTOP);
        let card = text_bounds(&layout, task.title)
            .into_iter()
            .find(|rect| {
                rect.min.x
                    > crate::gui_v2::simple_shell::shell_metrics(DESKTOP[0], DESKTOP[1]).sidebar
            })
            .unwrap_or_else(|| panic!("{} card", task.title));
        click_at(&context, &mut app, DESKTOP, card.center());
        assert_eq!(
            app.router.current,
            Route::Section(task.section),
            "{}",
            task.title
        );
        // Still inside Manage Library, and Back returns to the hub.
        app.back();
        assert_eq!(
            app.router.current,
            Destination::OrganiseExport.route().unwrap()
        );
    }
}

#[test]
fn the_classic_view_still_shows_the_organisation_overview_at_that_location() {
    // With Blue off the page is exactly the existing one.
    let context = egui::Context::default();
    let mut classic = classic_view(&context);
    classic.router.current = Destination::OrganiseExport.route().unwrap();
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
        app.router.current = Destination::OrganiseExport.route().unwrap();
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
        // Only a focused card counts: a sidebar row may open the same page.
        let focused = context.memory(|memory| memory.focused());
        let card = MANAGE_TASKS
            .iter()
            .find(|task| focused == Some(crate::gui_v2::simple_shell::task_id(task.section)));
        key(egui::Key::Enter, &mut app);
        if let Some(task) = card
            && app.router.current == Route::Section(task.section)
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
        // Activity: no sidebar row opens it, so any arrival is from the keyboard.
        app.router.current = Route::Section(Section::Activity);
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
            frame(&context, &mut app, size);
            let row = visible_row(&context, destination).unwrap_or_else(|| {
                panic!("{} was focused off screen at {scale}", destination.label())
            });
            assert!(
                row.min.y >= 0.0 && row.max.y <= size[1],
                "{} focused outside the window at {scale}",
                destination.label()
            );
            key_press(&context, &mut app, size, egui::Key::Enter);
            assert!(
                is_open(&app, destination),
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
            app.router.current = Route::Section(Section::Activity);
            frame(&context, &mut app, size);
            let over_sidebar = egui::pos2(30.0, size[1] * 0.6);
            // Click the row if it is showing; otherwise wheel down over the
            // sidebar and look again, as a person would.
            for _ in 0..14 {
                frame(&context, &mut app, size);
                if let Some(row) = visible_row(&context, destination) {
                    click_at(&context, &mut app, size, row.center());
                    if is_open(&app, destination) {
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
            assert!(
                is_open(&app, destination),
                "{} not reachable by mouse at {scale}",
                destination.label()
            );
        }
    }
}

#[test]
fn sidebar_labels_are_never_cut_off_by_the_sidebar_edge() {
    for scale in [0.8, 1.0, 1.25, 1.6] {
        for window in [[1024.0, 640.0], [1672.0, 941.0]] {
            let size = [window[0] / scale, window[1] / scale];
            let context = egui::Context::default();
            let mut app = scaled(&context, scale);
            frame(&context, &mut app, size);
            let layout = frame(&context, &mut app, size);
            let metrics = crate::gui_v2::simple_shell::shell_metrics(size[0], size[1]);
            for destination in DESTINATIONS {
                for rect in text_bounds(&layout, destination.label()) {
                    // Sidebar rows only; the page may repeat a label.
                    if rect.min.x < metrics.sidebar {
                        // The rail paints no labels at all.
                        assert!(!metrics.rail, "{} label on the rail", destination.label());
                        assert!(
                            rect.max.x <= metrics.sidebar - 6.0,
                            "{} runs to {} past the {} sidebar at {scale}, {window:?}",
                            destination.label(),
                            rect.max.x,
                            metrics.sidebar
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
    visit(&mut app, Destination::OrganiseExport.route().unwrap());
    // Cheats & Mods builds the embedded workflow host, which restyles the context.
    visit(&mut app, Destination::Enhance.route().unwrap());
    visit(&mut app, Route::Home);
    for section in crate::gui_v2::routes::SECTIONS {
        visit(&mut app, Route::Section(*section));
        visit(&mut app, Route::Home);
    }
    // All Tools and back keep it too.
    app.simple.all_tools = true;
    visit(&mut app, Destination::Enhance.route().unwrap());
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
    let mut app = classic_view(&context);
    frame(&context, &mut app, DESKTOP);
    assert_classic(&context, &classic, "Classic start");

    for round in 0..4 {
        app.simple.enabled = true;
        app.simple.set_ui_scale(1.0 + 0.2 * round as f32);
        // Visit pages in both modes, including the one that restyles the context.
        for route in [
            Route::Home,
            Destination::Enhance.route().unwrap(),
            Destination::OrganiseExport.route().unwrap(),
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
            Destination::Enhance.route().unwrap(),
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
            // Classic honours the saved scale; only the style is restored.
            assert!((context.zoom_factor() - app.simple.ui_scale).abs() < 1e-6);
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
    assert!((context.zoom_factor() - 1.6).abs() < 1e-6);
}

// Manage-card focus must follow the page scroll, including reverse Tab.
fn manage_window(window: [f32; 2], scale: f32) -> (egui::Context, App, [f32; 2]) {
    let context = egui::Context::default();
    let mut app = scaled(&context, scale);
    app.router.current = Destination::OrganiseExport.route().unwrap();
    app.beginner_hints_enabled = false;
    let size = [window[0] / scale, window[1] / scale];
    frame(&context, &mut app, size);
    frame(&context, &mut app, size);
    // The preference rounds 125% to 130%. Exercise an actual 125% viewport
    // too, without changing that existing preference policy.
    context.set_zoom_factor(scale);
    frame(&context, &mut app, size);
    frame(&context, &mut app, size);
    assert!((context.zoom_factor() - scale).abs() < 1e-6);
    (context, app, size)
}

fn shift_tab(context: &egui::Context, app: &mut App, size: [f32; 2]) {
    let modifiers = egui::Modifiers {
        shift: true,
        ..Default::default()
    };
    for pressed in [true, false] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |context| app.show(context),
        );
    }
}

fn settled_card_frame(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    // Match a native repaint interval (about 350 ms). Scroll-area targets and
    // scrollbar layout can span several passes, especially with reverse Tab.
    for _ in 0..20 {
        frame(context, app, size);
    }
    frame(context, app, size)
}

fn tab_to_task(context: &egui::Context, app: &mut App, size: [f32; 2], section: Section) {
    let id = crate::gui_v2::simple_shell::task_id(section);
    for _ in 0..32 {
        key_press(context, app, size, egui::Key::Tab);
        frame(context, app, size);
        frame(context, app, size);
        if context.memory(|memory| memory.focused()) == Some(id) {
            settled_card_frame(context, app, size);
            return;
        }
    }
    panic!("Tab never reached {section:?}");
}

fn assert_task_visible(
    context: &egui::Context,
    output: &egui::FullOutput,
    task: &crate::gui_v2::simple_shell::ManageTask,
) {
    use crate::gui_v2::simple_shell::task_id;
    let response = context.read_response(task_id(task.section)).unwrap();
    assert!(response.has_focus(), "{} lacks keyboard focus", task.title);
    // Check the entire focus outline against the actual scroll-area clip,
    // not merely whether the title's coordinates fall inside the window.
    assert!(
        output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Rect(rect)
                if rect.rect.min.distance(response.rect.min) < 0.01
                    && rect.rect.max.distance(response.rect.max) < 0.01
                    && rect.stroke.width >= 2.0
                    && rect.stroke.color == palette::ACCENT
                    && shape.clip_rect.contains_rect(rect.rect))
        }),
        "{} focus outline is missing or clipped: {:?}; painted focus rectangles: {:?}",
        task.title,
        response.rect,
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.width >= 2.0 && rect.stroke.color == palette::ACCENT =>
                    Some((rect.rect, shape.clip_rect)),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
    for wanted in [task.title, task.description] {
        assert!(
            output.shapes.iter().any(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                    text.galley.text() == wanted
                        && response.rect.contains_rect(bounds)
                        && shape.clip_rect.contains_rect(bounds)
                } else {
                    false
                }
            }),
            "{} text is missing or clipped: {wanted}",
            task.title
        );
    }
}

#[test]
fn manage_cards_are_fully_visible_with_forward_and_reverse_tab_at_all_sizes() {
    use crate::gui_v2::simple_shell::{MANAGE_TASKS, task_id};
    for window in [SMALL, DESKTOP] {
        for scale in [0.8, 1.0, 1.25, 1.6] {
            let (context, mut app, size) = manage_window(window, scale);
            tab_to_task(&context, &mut app, size, MANAGE_TASKS[0].section);
            for (index, task) in MANAGE_TASKS.iter().enumerate() {
                if index > 0 {
                    key_press(&context, &mut app, size, egui::Key::Tab);
                }
                frame(&context, &mut app, size);
                let output = settled_card_frame(&context, &mut app, size);
                assert_eq!(context.memory(|m| m.focused()), Some(task_id(task.section)));
                assert_task_visible(&context, &output, task);
            }
            for task in MANAGE_TASKS[..4].iter().rev() {
                shift_tab(&context, &mut app, size);
                frame(&context, &mut app, size);
                let output = settled_card_frame(&context, &mut app, size);
                assert_eq!(context.memory(|m| m.focused()), Some(task_id(task.section)));
                assert_task_visible(&context, &output, task);
            }
        }
    }
}

#[test]
fn manage_cards_enter_opens_the_visibly_focused_destination_at_all_sizes() {
    use crate::gui_v2::simple_shell::MANAGE_TASKS;
    for window in [SMALL, DESKTOP] {
        for scale in [0.8, 1.0, 1.25, 1.6] {
            for task in MANAGE_TASKS {
                let (context, mut app, size) = manage_window(window, scale);
                tab_to_task(&context, &mut app, size, task.section);
                frame(&context, &mut app, size);
                let output = settled_card_frame(&context, &mut app, size);
                assert_task_visible(&context, &output, &task);
                key_press(&context, &mut app, size, egui::Key::Enter);
                assert_eq!(app.router.current, Route::Section(task.section));
            }
        }
    }
}

fn page_wheel(context: &egui::Context, app: &mut App, size: [f32; 2], delta: f32) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            events: vec![
                egui::Event::PointerMoved(egui::pos2(size[0] - 40.0, size[1] * 0.7)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, delta),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |context| app.show(context),
    );
    // Wheel input is smoothed by egui. Finish that movement before comparing
    // geometry or sending the next pointer event.
    for _ in 0..20 {
        frame(context, app, size);
    }
}

#[test]
fn manage_cards_remain_mouse_reachable_with_wheel_scrolling_at_all_sizes() {
    use crate::gui_v2::simple_shell::MANAGE_TASKS;
    for window in [SMALL, DESKTOP] {
        for scale in [0.8, 1.0, 1.25, 1.6] {
            for task in MANAGE_TASKS {
                let (context, mut app, size) = manage_window(window, scale);
                let mut clicked = false;
                for _ in 0..30 {
                    let output = frame(&context, &mut app, size);
                    let point = output.shapes.iter().find_map(|shape| {
                        if let egui::Shape::Text(text) = &shape.shape {
                            let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                            // The card, never a sidebar row with the same name.
                            let sidebar =
                                crate::gui_v2::simple_shell::shell_metrics(size[0], size[1])
                                    .sidebar;
                            (text.galley.text() == task.title
                                && bounds.min.x > sidebar
                                && shape.clip_rect.contains_rect(bounds))
                            .then_some(bounds.center())
                        } else {
                            None
                        }
                    });
                    if let Some(point) = point {
                        click_at(&context, &mut app, size, point);
                        assert_eq!(app.router.current, Route::Section(task.section));
                        clicked = true;
                        break;
                    }
                    page_wheel(&context, &mut app, size, -40.0);
                }
                assert!(clicked, "{} not mouse reachable at {scale}", task.title);
            }
        }
    }
}

#[test]
fn manage_card_focus_does_not_snap_back_after_explicit_wheel_scrolling() {
    use crate::gui_v2::simple_shell::{MANAGE_TASKS, task_id};
    let (context, mut app, size) = manage_window(SMALL, 1.6);
    let task = MANAGE_TASKS[4];
    tab_to_task(&context, &mut app, size, task.section);
    frame(&context, &mut app, size);
    for _ in 0..4 {
        page_wheel(&context, &mut app, size, 40.0);
    }
    let before = context.read_response(task_id(task.section)).unwrap().rect;
    frame(&context, &mut app, size);
    let after = context.read_response(task_id(task.section)).unwrap().rect;
    assert_eq!(
        before, after,
        "idle/hover snapped the page back to focused card"
    );
    assert!(
        after.max.y > size[1],
        "explicit wheel scrolling did not move the card off screen"
    );
}

#[test]
fn manage_card_pointer_press_does_not_scroll_a_partially_visible_card() {
    use crate::gui_v2::simple_shell::{MANAGE_TASKS, task_id};
    let (context, mut app, size) = manage_window(SMALL, 1.6);
    let task = MANAGE_TASKS[0];
    page_wheel(&context, &mut app, size, -40.0);
    let output = frame(&context, &mut app, size);
    let before = context.read_response(task_id(task.section)).unwrap().rect;
    let (point, clip) = output
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                (text.galley.text() == task.title)
                    .then_some((text.pos + text.galley.size() * 0.5, shape.clip_rect))
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        !clip.contains_rect(before),
        "fixture must have a clipped card"
    );
    assert!(clip.contains(point), "click target must be visible");
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
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |context| app.show(context),
    );
    frame(&context, &mut app, size);
    assert_eq!(
        context.read_response(task_id(task.section)).unwrap().rect,
        before
    );
    assert_eq!(
        app.router.current,
        Destination::OrganiseExport.route().unwrap()
    );
    frame_with(
        &context,
        &mut app,
        size,
        vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(app.router.current, Route::Section(task.section));
}

// ---- second review coverage: both window sizes, the selectable steps, reverse
// ---- Tab, the selected row, and repeated palette lifecycles

const REVIEW_WINDOWS: [[f32; 2]; 2] = [[1024.0, 640.0], [1280.0, 720.0]];
/// The steps a person can select: 80%, 100%, the 130% the preference rounds
/// 125% to, and 160%.
const REVIEW_SCALES: [f32; 4] = [0.8, 1.0, 1.3, 1.6];

/// A `window` in pixels, in points at `scale`.
fn window_at(window: [f32; 2], scale: f32) -> [f32; 2] {
    [window[0] / scale, window[1] / scale]
}

/// A Simple window at `scale` on a page no sidebar row opens.
fn review_window(window: [f32; 2], scale: f32) -> (egui::Context, App, [f32; 2]) {
    let context = egui::Context::default();
    let mut app = scaled(&context, scale);
    assert!(
        (app.simple.ui_scale - scale).abs() < 1e-6,
        "{scale} is not a step"
    );
    app.router.current = Route::Section(Section::Activity);
    let size = window_at(window, scale);
    frame(&context, &mut app, size);
    frame(&context, &mut app, size);
    (context, app, size)
}

/// Asserts the destination's whole row is inside the part of the sidebar that
/// shows rows: below the brand and above the version footer, with its label
/// painted. A row scrolled out of the list fails this.
#[track_caller]
fn assert_row_shown(
    context: &egui::Context,
    layout: &egui::FullOutput,
    destination: Destination,
    when: &str,
) {
    let clip = list_clip(context);
    let row = row_rect(context, destination);
    assert!(
        clip.expand(0.5).contains_rect(row),
        "{} row {:?} is outside the visible list {:?} {when}",
        destination.label(),
        row.y_range(),
        clip.y_range()
    );
    // On the full sidebar the label is painted in its row; on the rail the
    // icon carries it (and the row's accessible name).
    if row.width() > crate::gui_v2::simple_shell::RAIL_WIDTH {
        assert!(
            text_bounds(layout, destination.label())
                .iter()
                .any(|label| row.contains(label.center())),
            "{} label is not painted in its row {when}",
            destination.label()
        );
    }
}

/// A settled frame. The sidebar scroll, and the response egui remembers for a
/// row, each trail the input by a frame or two.
fn settled(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    for _ in 0..6 {
        frame(context, app, size);
    }
    frame(context, app, size)
}

fn focused_destination(context: &egui::Context) -> Option<Destination> {
    use crate::gui_v2::simple_shell::nav_id;
    let focused = context.memory(|memory| memory.focused());
    DESTINATIONS
        .iter()
        .copied()
        .find(|destination| focused == Some(nav_id(*destination)))
}

fn wheel_sidebar(context: &egui::Context, app: &mut App, size: [f32; 2], delta: f32) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            events: vec![
                egui::Event::PointerMoved(egui::pos2(30.0, size[1] * 0.6)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, delta),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |context| app.show(context),
    );
    frame(context, app, size);
}

#[test]
fn tab_reveals_and_opens_every_destination_at_every_window_and_step() {
    for window in REVIEW_WINDOWS {
        for scale in REVIEW_SCALES {
            let (context, mut app, size) = review_window(window, scale);
            let when = format!("with Tab at {window:?}, {scale}");
            let mut reached = Vec::new();
            for _ in 0..24 {
                key_press(&context, &mut app, size, egui::Key::Tab);
                let Some(destination) = focused_destination(&context) else {
                    continue;
                };
                if reached.contains(&destination.label()) {
                    break;
                }
                let layout = settled(&context, &mut app, size);
                assert_row_shown(&context, &layout, destination, &when);
                key_press(&context, &mut app, size, egui::Key::Enter);
                assert!(is_open(&app, destination), "Enter {when}");
                reached.push(destination.label());
            }
            let expected: Vec<_> = DESTINATIONS.iter().map(|d| d.label()).collect();
            assert_eq!(reached, expected, "{when}");
        }
    }
}

#[test]
fn shift_tab_reveals_every_destination_at_every_window_and_step() {
    for window in REVIEW_WINDOWS {
        for scale in REVIEW_SCALES {
            let (context, mut app, size) = review_window(window, scale);
            let when = format!("with Shift+Tab at {window:?}, {scale}");
            let mut reached = Vec::new();
            // Backwards from the end of the window: the page first, then the
            // header, then the sidebar from Settings up to Home.
            for _ in 0..240 {
                shift_tab(&context, &mut app, size);
                let Some(destination) = focused_destination(&context) else {
                    if reached.is_empty() {
                        continue;
                    }
                    break;
                };
                let layout = settled(&context, &mut app, size);
                assert_row_shown(&context, &layout, destination, &when);
                reached.push(destination.label());
                if reached.len() == DESTINATIONS.len() {
                    break;
                }
            }
            let expected: Vec<_> = DESTINATIONS.iter().rev().map(|d| d.label()).collect();
            assert_eq!(reached, expected, "{when}");
        }
    }
}

#[test]
fn the_wheel_reaches_every_destination_at_every_window_and_step() {
    for window in REVIEW_WINDOWS {
        for scale in REVIEW_SCALES {
            for destination in DESTINATIONS {
                let (context, mut app, size) = review_window(window, scale);
                for _ in 0..14 {
                    frame(&context, &mut app, size);
                    if let Some(row) = visible_row(&context, destination) {
                        click_at(&context, &mut app, size, row.center());
                        if is_open(&app, destination) {
                            break;
                        }
                    }
                    wheel_sidebar(&context, &mut app, size, -40.0);
                }
                assert!(
                    is_open(&app, destination),
                    "{} not reachable by wheel at {window:?}, {scale}",
                    destination.label()
                );
            }
        }
    }
}

#[test]
fn every_label_fits_its_row_at_every_window_and_step() {
    for window in REVIEW_WINDOWS {
        for scale in REVIEW_SCALES {
            for destination in DESTINATIONS {
                // Selected, so the row is shown whatever the window height.
                let (context, mut app, size) = review_window(window, scale);
                open(&mut app, destination);
                let layout = settled(&context, &mut app, size);
                let metrics = crate::gui_v2::simple_shell::shell_metrics(size[0], size[1]);
                let labels: Vec<_> = text_bounds(&layout, destination.label())
                    .into_iter()
                    .filter(|rect| rect.min.x < metrics.sidebar)
                    .collect();
                if metrics.rail {
                    // Icons only; the label is the hover text and accessible name.
                    assert!(
                        labels.is_empty(),
                        "{} label on the rail",
                        destination.label()
                    );
                    continue;
                }
                // The whole label, on one line, inside the sidebar.
                assert_eq!(
                    labels.len(),
                    1,
                    "{} is not shown whole at {window:?}, {scale}",
                    destination.label()
                );
                assert!(
                    labels[0].max.x <= metrics.sidebar - 6.0 && labels[0].height() < 30.0,
                    "{} label {:?} does not fit the {} sidebar at {window:?}, {scale}",
                    destination.label(),
                    labels[0],
                    metrics.sidebar
                );
            }
        }
    }
}

#[test]
fn the_selected_destination_is_shown_at_every_window_and_step() {
    for window in REVIEW_WINDOWS {
        for scale in REVIEW_SCALES {
            // Opened there: a restored session, or a link from a page.
            for destination in DESTINATIONS {
                let (context, mut app, size) = review_window(window, scale);
                open(&mut app, destination);
                let layout = settled(&context, &mut app, size);
                assert_row_shown(
                    &context,
                    &layout,
                    destination,
                    &format!("when opened at {window:?}, {scale}"),
                );
            }
            // Moving between the ends of the list in one session.
            let (context, mut app, size) = review_window(window, scale);
            for destination in [
                Destination::Settings,
                Destination::Home,
                Destination::HealthRecovery,
                Destination::Library,
                Destination::Settings,
            ] {
                open(&mut app, destination);
                let layout = settled(&context, &mut app, size);
                assert_row_shown(
                    &context,
                    &layout,
                    destination,
                    &format!("after moving there at {window:?}, {scale}"),
                );
            }
        }
    }
}

#[test]
fn showing_the_selected_destination_does_not_undo_wheel_scrolling() {
    // 1024x640 at 160%: the list is taller than the sidebar.
    let (context, mut app, size) = review_window([1024.0, 640.0], 1.6);
    open(&mut app, Destination::Settings);
    let layout = settled(&context, &mut app, size);
    assert_row_shown(&context, &layout, Destination::Settings, "when opened");
    // The person scrolls back to the top of the list; it stays there.
    for _ in 0..12 {
        wheel_sidebar(&context, &mut app, size, 40.0);
    }
    let layout = settled(&context, &mut app, size);
    assert_row_shown(&context, &layout, Destination::Home, "after wheeling up");
    assert!(is_open(&app, Destination::Settings));
}

#[test]
fn home_to_cheats_and_mods_and_back_keeps_the_exact_blue_style_every_time() {
    let context = egui::Context::default();
    let mut app = simple(&context);
    frame(&context, &mut app, DESKTOP);
    frame(&context, &mut app, DESKTOP);
    assert!(blue(&context));
    // Every field, not a sample of colours.
    let blue_style = (*context.style()).clone();
    for round in 0..10 {
        for route in [Destination::Enhance.route().unwrap(), Route::Home] {
            app.router.current = route.clone();
            frame(&context, &mut app, DESKTOP);
            frame(&context, &mut app, DESKTOP);
            assert_eq!(
                *context.style(),
                blue_style,
                "round {round}: the blue style changed on {route:?}"
            );
        }
    }
}

#[test]
fn twelve_blue_classic_cycles_restore_both_complete_styles_without_drift() {
    let classic = fresh_classic();
    let context = egui::Context::default();
    let mut app = classic_view(&context);
    frame(&context, &mut app, DESKTOP);
    assert_classic(&context, &classic, "Classic start");
    let mut first_blue: Option<egui::Style> = None;
    for round in 0..12 {
        let scale = REVIEW_SCALES[round % REVIEW_SCALES.len()];
        app.simple.enabled = true;
        app.simple.set_ui_scale(scale);
        // Through the page that restyles the context, and back.
        for route in [
            Route::Home,
            Destination::Enhance.route().unwrap(),
            Route::Home,
        ] {
            app.router.current = route;
            frame(&context, &mut app, DESKTOP);
            frame(&context, &mut app, DESKTOP);
            let now = (*context.style()).clone();
            match &first_blue {
                None => {
                    assert!(blue(&context));
                    first_blue = Some(now);
                }
                Some(first) => assert_eq!(&now, first, "round {round}: blue drifted"),
            }
        }
        assert!(
            (context.zoom_factor() - scale).abs() < 1e-6,
            "round {round}"
        );
        app.simple.enabled = false;
        for route in [Destination::Enhance.route().unwrap(), Route::Home] {
            app.router.current = route;
            frame(&context, &mut app, DESKTOP);
            frame(&context, &mut app, DESKTOP);
            assert_classic(
                &context,
                &classic,
                &format!("round {round}: classic drifted"),
            );
            assert!(
                (context.zoom_factor() - app.simple.ui_scale).abs() < 1e-6,
                "round {round}"
            );
        }
    }
}

// No release-only or settling frames between these key presses. A complete
// physical key gesture is delivered to App::show in one rendered frame.
fn hardening_key_frame(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    key: egui::Key,
    reverse: bool,
) -> egui::FullOutput {
    let modifiers = egui::Modifiers {
        shift: reverse,
        ..Default::default()
    };
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            modifiers,
            events: [true, false]
                .into_iter()
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .collect(),
            ..Default::default()
        },
        |context| {
            app.show(context);
            for id in DESTINATIONS
                .map(crate::gui_v2::simple_shell::nav_id)
                .into_iter()
                .chain(
                    crate::gui_v2::simple_shell::MANAGE_TASKS
                        .map(|task| crate::gui_v2::simple_shell::task_id(task.section)),
                )
            {
                if let Some(response) = context.read_response(id) {
                    context.data_mut(|data| {
                        data.insert_temp(id.with("hardening_actual_rect"), response.rect)
                    });
                }
            }
        },
    )
}

#[track_caller]
fn hardening_row_visible(
    context: &egui::Context,
    output: &egui::FullOutput,
    destination: Destination,
    keyboard: bool,
) {
    let response = context
        .read_response(crate::gui_v2::simple_shell::nav_id(destination))
        .unwrap();
    let rect = if keyboard {
        context
            .data(|data| data.get_temp::<egui::Rect>(response.id.with("hardening_actual_rect")))
            .unwrap()
    } else {
        response.rect
    };
    let clip = list_clip(context);
    let labelled = output.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Text(text)
            if text.galley.text() == destination.label()
                && rect.contains(text.pos)
                && shape.clip_rect.contains_rect(rect))
    });
    let on_rail = rect.width() <= crate::gui_v2::simple_shell::RAIL_WIDTH;
    assert!(
        clip.expand(0.5).contains_rect(rect) && (labelled || on_rail),
        "{} row {:?} is outside its actual sidebar clip {:?}",
        destination.label(),
        rect,
        clip
    );
}

#[test]
fn hardening_sidebar_direct_route_reveals_once_matrix() {
    use crate::gui_v2::simple_shell::nav_id;
    for window in [[1024.0, 640.0], [1280.0, 720.0]] {
        for scale in [0.8, 1.0, 1.3, 1.6] {
            let (context, mut app, size) = manage_window(window, scale);
            // Exercise every route, including changes between the two ends.
            for destination in DESTINATIONS.into_iter().rev().chain(DESTINATIONS) {
                open(&mut app, destination);
                frame(&context, &mut app, size);
                let output = frame(&context, &mut app, size);
                hardening_row_visible(&context, &output, destination, false);
            }
            // Wheel up while Settings stays selected. Idle repaint must leave
            // that deliberate position alone, including at sizes without overflow.
            open(&mut app, Destination::Settings);
            frame(&context, &mut app, size);
            frame(&context, &mut app, size);
            for _ in 0..12 {
                wheel_sidebar(&context, &mut app, size, 40.0);
            }
            for _ in 0..20 {
                frame(&context, &mut app, size);
            }
            let before = context
                .read_response(nav_id(Destination::Home))
                .unwrap()
                .rect;
            let output = frame(&context, &mut app, size);
            assert_eq!(
                context
                    .read_response(nav_id(Destination::Home))
                    .unwrap()
                    .rect,
                before
            );
            hardening_row_visible(&context, &output, Destination::Home, false);
            assert!(is_open(&app, Destination::Settings));
        }
    }
}

#[test]
fn hardening_sidebar_tabs_every_frame_forward_reverse_matrix() {
    use crate::gui_v2::simple_shell::nav_id;
    for window in [[1024.0, 640.0], [1280.0, 720.0]] {
        for scale in [0.8, 1.0, 1.3, 1.6] {
            for reverse in [false, true] {
                let (context, mut app, size) = manage_window(window, scale);
                let mut reached = Vec::new();
                for _ in 0..80 {
                    let output =
                        hardening_key_frame(&context, &mut app, size, egui::Key::Tab, reverse);
                    let focused = context.memory(|memory| memory.focused());
                    if let Some(destination) = DESTINATIONS
                        .into_iter()
                        .find(|d| focused == Some(nav_id(*d)))
                    {
                        hardening_row_visible(&context, &output, destination, true);
                        if reached.last() != Some(&destination) {
                            reached.push(destination);
                        }
                        if reached.len() == DESTINATIONS.len() {
                            break;
                        }
                    }
                }
                let expected: Vec<_> = if reverse {
                    DESTINATIONS.into_iter().rev().collect()
                } else {
                    DESTINATIONS.to_vec()
                };
                assert_eq!(reached, expected, "{window:?} {scale} reverse={reverse}");
            }
        }
    }
}

#[track_caller]
fn hardening_card_visible(
    context: &egui::Context,
    output: &egui::FullOutput,
    task: &crate::gui_v2::simple_shell::ManageTask,
) {
    let id = crate::gui_v2::simple_shell::task_id(task.section);
    assert_eq!(context.memory(|memory| memory.focused()), Some(id));
    let rect = context
        .data(|data| data.get_temp::<egui::Rect>(id.with("hardening_actual_rect")))
        .unwrap();
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(painted)
        if painted.rect == rect && painted.stroke.width >= 2.0 && painted.stroke.color == palette::ACCENT
            && shape.clip_rect.contains_rect(rect))), "{} focused rect {rect:?} is not fully painted inside the content clip", task.title);
    for wanted in [task.title, task.description] {
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text)
            if text.galley.text() == wanted && rect.contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size()))
                && shape.clip_rect.contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size())))), "{wanted} is missing/clipped");
    }
}

fn hardening_card_walk(reverse: bool, slow: bool, activate: Option<usize>) -> Vec<Section> {
    use crate::gui_v2::simple_shell::{MANAGE_TASKS, task_id};
    let (context, mut app, size) = manage_window(SMALL, 1.6);
    let mut reached = Vec::new();
    for step in 0..80 {
        let mut output = hardening_key_frame(&context, &mut app, size, egui::Key::Tab, reverse);
        if slow {
            output = settled_card_frame(&context, &mut app, size);
        }
        let focused = context.memory(|memory| memory.focused());
        let Some(task) = MANAGE_TASKS
            .iter()
            .find(|task| focused == Some(task_id(task.section)))
        else {
            continue;
        };
        if slow {
            assert_task_visible(&context, &output, task);
        } else {
            hardening_card_visible(&context, &output, task);
        }
        if reached.last() != Some(&task.section) {
            reached.push(task.section);
            if activate == Some(reached.len() - 1) {
                // The very next event activates the visibly outlined card.
                hardening_key_frame(&context, &mut app, size, egui::Key::Enter, false);
                assert_eq!(
                    app.router.current,
                    Route::Section(task.section),
                    "Enter at step {step}"
                );
                return reached;
            }
        }
        if reached.len() == MANAGE_TASKS.len() {
            return reached;
        }
    }
    panic!("did not reach every card: {reached:?}, reverse={reverse}");
}

#[test]
fn hardening_manage_tabs_every_frame_forward_reverse() {
    use crate::gui_v2::simple_shell::MANAGE_TASKS;
    let expected: Vec<_> = MANAGE_TASKS.iter().map(|task| task.section).collect();
    assert_eq!(hardening_card_walk(false, false, None), expected);
    assert_eq!(
        hardening_card_walk(true, false, None),
        expected.into_iter().rev().collect::<Vec<_>>()
    );
}

#[test]
fn hardening_manage_enter_after_every_rapid_focus_step() {
    for reverse in [false, true] {
        for index in 0..5 {
            assert_eq!(
                hardening_card_walk(reverse, false, Some(index)).len(),
                index + 1
            );
        }
    }
}

#[test]
fn hardening_manage_fast_and_slow_focus_orders_match() {
    for reverse in [false, true] {
        assert_eq!(
            hardening_card_walk(reverse, false, None),
            hardening_card_walk(reverse, true, None)
        );
    }
}

// ------------------------------------------------------------ Slice 2A scale

const WINDOWS_2A: [[f32; 2]; 3] = [[1024.0, 640.0], [1280.0, 720.0], [1920.0, 1080.0]];
const SCALES_2A: [f32; 4] = [0.8, 1.0, 1.3, 1.6];

fn pass_2a(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size[0], size[1]),
            )),
            modifiers,
            events,
            ..Default::default()
        },
        |context| app.show(context),
    );
}

fn chord(context: &egui::Context, app: &mut App, size: [f32; 2], key: egui::Key) {
    let modifiers = egui::Modifiers::COMMAND;
    for pressed in [true, false] {
        pass_2a(
            context,
            app,
            size,
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
    }
    for _ in 0..3 {
        frame(context, app, size);
    }
}

/// A Settings page in either profile at one window and saved scale.
fn settings_2a(
    blue: bool,
    window: [f32; 2],
    scale: f32,
    hints: bool,
) -> (egui::Context, App, [f32; 2]) {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.simple.enabled = blue;
    app.simple.set_ui_scale(scale);
    app.beginner_hints_enabled = hints;
    app.router.current = Route::Section(Section::Settings);
    let size = [window[0] / scale, window[1] / scale];
    for _ in 0..4 {
        frame(&context, &mut app, size);
    }
    (context, app, size)
}

fn zoom_of(context: &egui::Context) -> f32 {
    context.zoom_factor()
}

#[test]
fn slice2a_keyboard_zoom_and_saved_scale_are_one_value_in_both_profiles() {
    for blue in [true, false] {
        let (context, mut app, size) = settings_2a(blue, SMALL, 1.0, false);
        for (key, expected) in [
            (egui::Key::Plus, 1.1),
            (egui::Key::Equals, 1.2),
            (egui::Key::Minus, 1.1),
            (egui::Key::Num0, 1.0),
        ] {
            chord(&context, &mut app, size, key);
            assert!(
                (app.simple.ui_scale - expected).abs() < 1e-6,
                "blue={blue} {key:?}: saved {}",
                app.simple.ui_scale
            );
            assert!(
                (zoom_of(&context) - expected).abs() < 1e-6,
                "blue={blue} {key:?}"
            );
        }
        assert!(app.preferences_dirty.is_some(), "the new size is saved");
    }
}

#[test]
fn slice2a_keyboard_zoom_never_leaves_the_supported_bounds() {
    for blue in [true, false] {
        let (context, mut app, size) = settings_2a(blue, SMALL, 1.6, false);
        for _ in 0..3 {
            chord(&context, &mut app, size, egui::Key::Plus);
        }
        assert!((app.simple.ui_scale - 1.6).abs() < 1e-6);
        assert!(
            (zoom_of(&context) - 1.6).abs() < 1e-6,
            "blue={blue} above max"
        );
        let (context, mut app, size) = settings_2a(blue, SMALL, 0.8, false);
        for _ in 0..3 {
            chord(&context, &mut app, size, egui::Key::Minus);
        }
        assert!((app.simple.ui_scale - 0.8).abs() < 1e-6);
        assert!(
            (zoom_of(&context) - 0.8).abs() < 1e-6,
            "blue={blue} below min"
        );
    }
}

#[test]
fn slice2a_classic_renders_the_saved_scale_and_modes_preserve_it() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.simple.set_ui_scale(1.3);
    app.router.current = Route::Section(Section::Settings);
    let size = [DESKTOP[0] / 1.3, DESKTOP[1] / 1.3];
    for mode in [false, true, false, true, false] {
        app.simple.enabled = mode;
        for _ in 0..3 {
            frame(&context, &mut app, size);
        }
        assert!((zoom_of(&context) - 1.3).abs() < 1e-6, "blue={mode}");
        assert!((app.simple.ui_scale - 1.3).abs() < 1e-6, "blue={mode}");
    }
}

/// The painted text of `label` lies fully inside both its clip and the window.
fn label_visible(output: &egui::FullOutput, label: &str, size: [f32; 2]) -> bool {
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
    output.shapes.iter().any(|shape| match &shape.shape {
        egui::Shape::Text(text) if text.galley.text() == label => {
            let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
            shape.clip_rect.contains_rect(bounds) && window.contains_rect(bounds)
        }
        _ => false,
    })
}

#[test]
fn slice2a_settings_scale_controls_are_visible_without_scrolling_at_every_size() {
    for blue in [true, false] {
        for window in WINDOWS_2A {
            for scale in SCALES_2A {
                for hints in [true, false] {
                    let (context, mut app, size) = settings_2a(blue, window, scale, hints);
                    let output = frame(&context, &mut app, size);
                    for label in ["Smaller", "Larger", "Reset size"] {
                        assert!(
                            label_visible(&output, label, size),
                            "{label}: blue={blue} window={window:?} scale={scale} hints={hints}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn slice2a_tab_reveals_every_settings_control_at_the_hardest_setting() {
    for (blue, hints, reverse) in [
        (true, true, false),
        (true, false, false),
        (false, true, false),
        (false, false, false),
        (true, true, true),
        (false, false, true),
    ] {
        {
            let (context, mut app, size) = settings_2a(blue, SMALL, 1.6, hints);
            let mut seen = 0;
            let mut last = None;
            for step in 0..48 {
                if reverse {
                    shift_tab(&context, &mut app, size);
                } else {
                    key_press(&context, &mut app, size, egui::Key::Tab);
                }
                for _ in 0..3 {
                    frame(&context, &mut app, size);
                }
                let output = frame(&context, &mut app, size);
                let Some(id) = context.memory(|memory| memory.focused()) else {
                    continue;
                };
                if last == Some(id) {
                    continue;
                }
                last = Some(id);
                let Some(response) = context.read_response(id) else {
                    continue;
                };
                // Only content-area controls are under test; the header and
                // sidebar have their own reveal.
                if response.rect.min.x < 200.0 / 1.6 {
                    continue;
                }
                seen += 1;
                let window =
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
                assert!(
                    window.contains_rect(response.rect),
                    "step {step}: focused control {:?} outside the window (blue={blue} hints={hints})",
                    response.rect
                );
                let painted = output.shapes.iter().any(|shape| match &shape.shape {
                    egui::Shape::Text(text) => {
                        let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                        response.rect.contains_rect(bounds) && shape.clip_rect.contains_rect(bounds)
                    }
                    _ => false,
                });
                assert!(
                    painted,
                    "step {step}: focused control {:?} not shown (blue={blue} hints={hints})",
                    response.rect
                );
            }
            assert!(seen >= 6, "only {seen} Settings controls were reached");
        }
    }
}

// ---- Slice 2A focus correction: status-bar focus never scrolls the page body

/// The size heading and all three size controls are fully painted and visible.
#[track_caller]
fn size_controls_visible(output: &egui::FullOutput, size: [f32; 2], when: &str) {
    for label in [
        "Text and interface size: 160%",
        "Smaller",
        "Larger",
        "Reset size",
    ] {
        assert!(
            label_visible(output, label, size),
            "{label:?} not visible {when}"
        );
    }
}

#[test]
fn slice2a_first_tab_to_the_status_bar_leaves_the_settings_scroll_unchanged() {
    for blue in [true, false] {
        for hints in [true, false] {
            let when = format!("blue={blue} hints={hints}");
            let (context, mut app, size) = settings_2a(blue, SMALL, 1.6, hints);
            let output = frame(&context, &mut app, size);
            size_controls_visible(&output, size, &format!("before Tab, {when}"));

            key_press(&context, &mut app, size, egui::Key::Tab);
            for _ in 0..3 {
                frame(&context, &mut app, size);
            }
            let output = frame(&context, &mut app, size);
            size_controls_visible(&output, size, &format!("after the first Tab, {when}"));
            let focused = context
                .memory(|memory| memory.focused())
                .and_then(|id| context.read_response(id))
                .expect("the first Tab focuses a control");
            assert!(
                focused.rect.min.x < 200.0 / 1.6 || focused.rect.min.y > size[1] - 60.0,
                "the first Tab reaches chrome, not the page body, {when}: {:?}",
                focused.rect
            );
        }
    }
}

/// Every painted text with its position, as a sorted, comparable snapshot of
/// where the page body is. Equal snapshots mean nothing scrolled.
fn painted_text(output: &egui::FullOutput) -> Vec<(String, i32, i32)> {
    let mut texts: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some((
                text.galley.text().to_string(),
                text.pos.x.round() as i32,
                text.pos.y.round() as i32,
            )),
            _ => None,
        })
        .collect();
    texts.sort();
    texts
}

fn settled_frame(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    for _ in 0..3 {
        frame(context, app, size);
    }
    frame(context, app, size)
}

#[test]
fn slice2a_first_tab_never_scrolls_any_page_body_in_either_profile() {
    for blue in [true, false] {
        for section in DESTINATIONS {
            let context = egui::Context::default();
            let mut app = fixture(&context);
            app.simple.enabled = blue;
            app.simple.set_ui_scale(1.6);
            open(&mut app, section);
            let size = [SMALL[0] / 1.6, SMALL[1] / 1.6];
            let before = painted_text(&settled_frame(&context, &mut app, size));
            key_press(&context, &mut app, size, egui::Key::Tab);
            let after = painted_text(&settled_frame(&context, &mut app, size));
            let moved: Vec<_> = after
                .iter()
                .filter(|text| text.1 >= (200.0 / 1.6) as i32 && !before.contains(text))
                .collect();
            let content_y = |text: &(String, i32, i32)| text.2;
            let focus_ring_only = moved.iter().all(|text| content_y(text) > 0);
            assert!(
                focus_ring_only && moved.len() <= 2,
                "first Tab moved the {section:?} body (blue={blue}): {moved:?}"
            );
        }
    }
}

#[test]
fn slice2a_wheel_scrolling_stays_where_the_user_put_it() {
    for blue in [true, false] {
        let (context, mut app, size) = settings_2a(blue, SMALL, 1.6, true);
        let start = painted_text(&settled_frame(&context, &mut app, size));
        page_wheel(&context, &mut app, size, -120.0);
        let scrolled = painted_text(&settled_frame(&context, &mut app, size));
        assert_ne!(start, scrolled, "the wheel scrolls Settings (blue={blue})");
        for _ in 0..30 {
            frame(&context, &mut app, size);
        }
        assert_eq!(
            scrolled,
            painted_text(&frame(&context, &mut app, size)),
            "an idle page does not drift after a wheel scroll (blue={blue})"
        );
    }
}

#[test]
fn slice2a_a_genuine_offscreen_page_control_is_revealed_after_the_status_bar_fix() {
    // Tab until a page-body control that began below the fold takes focus: it
    // must end fully visible, in both directions and both profiles.
    for blue in [true, false] {
        for reverse in [false, true] {
            let (context, mut app, size) = settings_2a(blue, SMALL, 1.6, true);
            let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
            let mut revealed = 0;
            for _ in 0..48 {
                if reverse {
                    shift_tab(&context, &mut app, size);
                } else {
                    key_press(&context, &mut app, size, egui::Key::Tab);
                }
                settled_frame(&context, &mut app, size);
                let Some(response) = context
                    .memory(|memory| memory.focused())
                    .and_then(|id| context.read_response(id))
                else {
                    continue;
                };
                if response.rect.min.x < 200.0 / 1.6 {
                    continue;
                }
                assert!(
                    window.contains_rect(response.rect),
                    "focused page control {:?} is outside the window (blue={blue} reverse={reverse})",
                    response.rect
                );
                revealed += 1;
            }
            assert!(revealed >= 4, "only {revealed} page controls were reached");
        }
    }
}
