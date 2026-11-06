//! Slice 2B layout matrix: a populated library rendered through the real
//! `App::show` in Blue and Classic, at every saved scale and window size. The
//! probe measures what is actually painted - clipped text, overlapping
//! controls and reachable actions - rather than asserting on private layout.
use super::launch_readiness_summary::{self, ReadinessAction};
use super::*;

const SCALES: [f32; 4] = [0.8, 1.0, 1.3, 1.6];
const WINDOWS: [(f32, f32); 3] = [(1024.0, 640.0), (1280.0, 720.0), (1920.0, 1080.0)];
const LONG_TITLE: &str = "The Extraordinarily Long Adventures of a Game With A Title That Keeps \
    Going Well Beyond Any Reasonable Width: Special Collector's Edition Remastered (Europe) (Rev 2)";

fn populated(context: &egui::Context, blue: bool, scale: f32) -> App {
    let mut app = fixture(context);
    let mut games = vec![archive(1, LONG_TITLE, Some("Sony PlayStation 2"))];
    let systems = [
        "SNES",
        "Mega Drive",
        "Dreamcast",
        "Arcade",
        "Game Boy Advance",
    ];
    for index in 2..=60_i64 {
        games.push(archive(
            index,
            &format!("Fixture Game {index:02}"),
            Some(systems[index as usize % systems.len()]),
        ));
    }
    app.library = Arc::new(Library::new(games));
    app.simple.enabled = blue;
    app.simple.set_ui_scale(scale);
    app
}

fn points(window: (f32, f32), scale: f32) -> [f32; 2] {
    [window.0 / scale, window.1 / scale]
}

/// Text shapes painted wider than the region they are clipped to, or off the
/// screen: the "horizontal clipping" the slice is about.
fn clipped_text(output: &egui::FullOutput, size: [f32; 2]) -> Vec<String> {
    fn gather(shape: &egui::Shape, clip: egui::Rect, size: [f32; 2], found: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => {
                let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                let limit = clip.intersect(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                ));
                if rect.width() > 1.0
                    && (rect.max.x > limit.max.x + 1.0 || rect.min.x < limit.min.x - 1.0)
                {
                    found.push(format!("{:?}", text.galley.text()));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    gather(shape, clip, size, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        gather(&clipped.shape, clipped.clip_rect, size, &mut found);
    }
    found
}

fn settled(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    for _ in 0..3 {
        frame(context, app, size);
    }
    frame(context, app, size)
}

fn visible(rect: egui::Rect, size: [f32; 2]) -> bool {
    rect.min.x >= -0.5
        && rect.min.y >= -0.5
        && rect.max.x <= size[0] + 0.5
        && rect.max.y <= size[1] + 0.5
}

fn role_size(context: &egui::Context, role: crate::ui::theme::TextRole) -> f32 {
    context.style().text_styles[&role.text_style()].size
}

#[test]
fn typography_roles_are_ordered_in_blue_and_classic_keeps_its_sizes() {
    use crate::ui::theme::{self, TextRole};
    for blue in [true, false] {
        let context = egui::Context::default();
        let mut app = populated(&context, blue, 1.0);
        app.router.current = Route::Home;
        settled(&context, &mut app, [1280.0, 720.0]);
        let page = role_size(&context, TextRole::PageTitle);
        let section = role_size(&context, TextRole::SectionTitle);
        let card = role_size(&context, TextRole::CardTitle);
        let body = role_size(&context, TextRole::Body);
        assert_eq!(
            page,
            context.style().text_styles[&egui::TextStyle::Heading].size
        );
        assert!(page > section && section > body, "{page} {section} {body}");
        if blue {
            assert!(
                section > card && card > body,
                "{page} {section} {card} {body}"
            );
        } else {
            // Classic is unchanged at 100%: headings 27, section titles 19.
            assert_eq!(
                (page, section, card),
                (
                    theme::PAGE_TITLE_SIZE,
                    theme::SECTION_TITLE_SIZE,
                    theme::PAGE_TITLE_SIZE
                )
            );
        }
    }
}

#[test]
fn slice2b_matrix_has_no_clipping_overlap_or_hidden_play() {
    let pages: [(&str, Route); 5] = [
        ("home", Route::Home),
        ("platforms", Route::Section(Section::Platforms)),
        ("library", Route::BrowsePlayGame(1)),
        ("game", Route::Game(1)),
        ("settings", Route::Section(Section::Settings)),
    ];
    let mut failures = Vec::new();
    for blue in [true, false] {
        for scale in SCALES {
            for window in WINDOWS {
                let size = points(window, scale);
                for (name, route) in &pages {
                    let context = egui::Context::default();
                    let mut app = populated(&context, blue, scale);
                    app.router.current = route.clone();
                    let output = settled(&context, &mut app, size);
                    let cell = format!("blue={blue} {scale} {window:?} {name}");
                    let clipped = clipped_text(&output, size);
                    if !clipped.is_empty() {
                        failures.push(format!("{cell}: clipped {clipped:?}"));
                    }
                    if *name == "library" {
                        let search = text_bounds(&output, "Search by title or system");
                        let grid = text_bounds(&output, "Grid");
                        let list = text_bounds(&output, "List");
                        // Search shares a row with the switch only when it fits;
                        // either way their rectangles never intersect.
                        for switch in grid.iter().chain(list.iter()) {
                            for field in &search {
                                if field.intersects(*switch) {
                                    failures.push(format!("{cell}: search overlaps {switch:?}"));
                                }
                            }
                        }
                        let primary = scale == 1.0 && window == (1280.0, 720.0);
                        if primary
                            && !text_bounds(&output, "Play")
                                .iter()
                                .any(|r| visible(*r, size))
                        {
                            failures.push(format!("{cell}: Play not visible"));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// One physical key gesture (press and release) delivered in a single frame.
fn key_gesture(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    key: egui::Key,
    shift: bool,
) -> egui::FullOutput {
    let modifiers = egui::Modifiers {
        shift,
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
        |context| app.show(context),
    )
}

/// The rectangle of the keyboard-focused widget, when there is one.
fn focused_rect(context: &egui::Context) -> Option<egui::Rect> {
    let id = context.memory(|memory| memory.focused())?;
    context.read_response(id).map(|response| response.rect)
}

/// What a person sees of the page body: right of the sidebar, below the
/// header, above the status bar.
fn body_band(output: &egui::FullOutput, size: [f32; 2]) -> egui::Rect {
    let top = text_bounds(output, "Browse & Play")
        .iter()
        .map(|rect| rect.min.y)
        .fold(f32::MAX, f32::min);
    let bottom = text_bounds(output, "View Activity")
        .first()
        .map_or(size[1], |rect| rect.min.y);
    let left = text_bounds(output, "Find a game, check that it is ready, then play.")
        .first()
        .map_or(0.0, |rect| rect.min.x - 4.0);
    egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(size[0], bottom))
}

/// The real keyboard path at 1024x640 and 160% in both profiles: Tab walks to
/// Play, focus is revealed on screen, Enter opens the Launch task (nothing is
/// started), Shift+Tab leaves Play for a control that is also revealed, and
/// the wheel still scrolls the page.
#[test]
fn keyboard_focus_reveals_library_play_at_160_percent_in_both_profiles() {
    let size = points((1024.0, 640.0), 1.6);
    for blue in [true, false] {
        let context = egui::Context::default();
        let mut app = populated(&context, blue, 1.6);
        app.router.current = Route::BrowsePlayGame(1);
        let mut output = settled(&context, &mut app, size);
        // Start inside the page, as a person does after clicking the search
        // field: this keeps the test on the Library body, not the sidebar.
        let search = text_bounds(&output, "Search by title or system")
            .first()
            .copied()
            .expect("search field painted");
        let point = search.center();
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
        output = settled(&context, &mut app, size);
        let mut play_focused = false;
        let mut seen_content_focus = 0;
        for step in 0..40 {
            output = key_gesture(&context, &mut app, size, egui::Key::Tab, false);
            // A keyboard reveal may take several passes to settle.
            for _ in 0..8 {
                output = frame(&context, &mut app, size);
            }
            let band = body_band(&output, size);
            if let Some(rect) = focused_rect(&context)
                && rect.min.x >= band.min.x
                && rect.min.y > band.min.y - 1.0
            {
                seen_content_focus += 1;
                // A game card is taller than the shelf can show at this size,
                // so require its top (cover and focus ring) on screen with a
                // useful part of it, rather than all of it.
                let shown = rect.intersect(band);
                assert!(
                    rect.min.y >= band.min.y - 1.0
                        && rect.max.x <= band.max.x + 1.0
                        && (rect.max.y <= band.max.y + 1.0 || shown.height() >= 100.0),
                    "blue={blue} step {step}: focus {rect:?} is outside the visible body {band:?}"
                );
                if text_bounds(&output, "Play")
                    .iter()
                    .any(|play| rect.contains(play.center()))
                {
                    play_focused = true;
                    break;
                }
            }
        }
        assert!(
            seen_content_focus > 0 && play_focused,
            "blue={blue}: Tab never brought Play into focus on screen"
        );

        // Shift+Tab moves off Play to a control that is revealed too.
        let play_rect = focused_rect(&context).unwrap();
        let play_id = context.memory(|memory| memory.focused());
        let _ = key_gesture(&context, &mut app, size, egui::Key::Tab, true);
        output = settled(&context, &mut app, size);
        let band = body_band(&output, size);
        let back = focused_rect(&context).expect("focus after Shift+Tab");
        assert_ne!(back, play_rect, "blue={blue}: Shift+Tab did not move");
        assert!(
            back.min.y >= band.min.y - 1.0
                && (back.max.y <= band.max.y + 1.0 || back.intersect(band).height() >= 100.0),
            "blue={blue}: Shift+Tab target {back:?} is outside the visible body {band:?}"
        );

        // Tab forward again to Play and activate it from the keyboard.
        let _ = key_gesture(&context, &mut app, size, egui::Key::Tab, false);
        let output = settled(&context, &mut app, size);
        // The same Play control, still on screen. The selected-game panel may
        // have scrolled to reveal the Shift+Tab target, so its position can
        // differ from the first visit.
        assert_eq!(context.memory(|memory| memory.focused()), play_id);
        let band = body_band(&output, size);
        let again = focused_rect(&context).expect("Play focused again");
        assert_eq!(again.size(), play_rect.size());
        assert!(
            band.contains_rect(again),
            "blue={blue}: Play {again:?} is outside the visible body {band:?}"
        );
        let _ = key_gesture(&context, &mut app, size, egui::Key::Enter, false);
        settled(&context, &mut app, size);
        assert_eq!(
            app.router.current,
            Route::Task {
                section: Section::Launch,
                game: 1
            },
            "blue={blue}: Enter on Play did not open the Launch task"
        );
    }
}

fn wheel_at(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    point: egui::Pos2,
    delta_y: f32,
) -> egui::FullOutput {
    // Settle on the pointer first, as a real pointer move does, then turn the
    // wheel in a separate frame and let egui finish its scroll.
    for events in [
        vec![egui::Event::PointerMoved(point)],
        vec![egui::Event::MouseWheel {
            phase: egui::TouchPhase::Move,
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta_y),
            modifiers: egui::Modifiers::NONE,
        }],
    ] {
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                events,
                ..Default::default()
            },
            |context| app.show(context),
        );
    }
    settled(context, app, size)
}

/// Wheel input at 160%: over the page it reaches Play; over the shelf it
/// scrolls the shelf, and turning back never leaves the person stuck.
#[test]
fn wheel_scrolling_still_reaches_library_play_at_160_percent() {
    let size = points((1024.0, 640.0), 1.6);
    for blue in [true, false] {
        let context = egui::Context::default();
        let mut app = populated(&context, blue, 1.6);
        app.router.current = Route::BrowsePlayGame(1);
        settled(&context, &mut app, size);
        // Beside the toolbar: inside the scrolling body, off the shelf.
        let page_point = egui::pos2(size[0] * 0.55, size[1] * 0.69);
        let mut reached = false;
        let mut output = settled(&context, &mut app, size);
        for _ in 0..6 {
            output = wheel_at(&context, &mut app, size, page_point, -100.0);
            reached |= text_bounds(&output, "Play")
                .iter()
                .any(|rect| visible(*rect, size));
        }
        assert!(
            reached,
            "blue={blue}: wheel over the page never exposed Play"
        );

        // Over the shelf: wheel down, then back up all the way. The page must
        // follow once the shelf is at its top, so Play is exposed again.
        let shelf_point = text_bounds(&output, "Fixture Gba Game 01")
            .first()
            .map_or(page_point, |rect| rect.center());
        for _ in 0..4 {
            wheel_at(&context, &mut app, size, shelf_point, -100.0);
        }
        let mut back = false;
        for _ in 0..30 {
            output = wheel_at(&context, &mut app, size, shelf_point, 100.0);
            back |= text_bounds(&output, "Play")
                .iter()
                .any(|rect| visible(*rect, size));
        }
        assert!(
            back,
            "blue={blue}: wheel up over the shelf left the page stuck"
        );
    }
}

/// Starting at the very top (no click), every Tab stop is on screen - sidebar
/// entries included - until Play is reached. Blue and Classic, 1024x640 at 160%.
#[test]
fn tab_from_the_top_never_loses_focus_off_screen_before_play() {
    let size = points((1024.0, 640.0), 1.6);
    for blue in [true, false] {
        let context = egui::Context::default();
        let mut app = populated(&context, blue, 1.6);
        app.router.current = Route::BrowsePlayGame(1);
        let mut output = settled(&context, &mut app, size);
        let mut play_focused = false;
        for step in 0..140 {
            let _ = key_gesture(&context, &mut app, size, egui::Key::Tab, false);
            for _ in 0..8 {
                output = frame(&context, &mut app, size);
            }
            let status_top = text_bounds(&output, "View Activity")
                .first()
                .map_or(size[1], |rect| rect.min.y - 4.0);
            let Some(rect) = focused_rect(&context) else {
                continue;
            };
            assert!(
                rect.min.y >= -1.0 && rect.min.y < status_top && rect.min.x < size[0],
                "blue={blue} step {step}: focus {rect:?} is off screen (status bar at {status_top})"
            );
            if text_bounds(&output, "Play")
                .iter()
                .any(|play| rect.contains(play.center()))
            {
                play_focused = true;
                break;
            }
        }
        assert!(
            play_focused,
            "blue={blue}: Tab from the top never reached Play"
        );
    }
}

/// Cold reverse traversal: with no earlier focus, the first Shift+Tab lands on
/// the last rendered Library card, below the page, inside the nested shelf. It
/// must be brought into the visible page body in both profiles.
#[test]
fn cold_shift_tab_reveals_the_last_library_card_at_160_percent_in_both_profiles() {
    let size = points((1024.0, 640.0), 1.6);
    let mut failures = Vec::new();
    for blue in [true, false] {
        let context = egui::Context::default();
        let mut app = populated(&context, blue, 1.6);
        app.router.current = Route::BrowsePlayGame(1);
        let _ = settled(&context, &mut app, size);
        assert_eq!(context.memory(|memory| memory.focused()), None);
        let _ = key_gesture(&context, &mut app, size, egui::Key::Tab, true);
        let mut output = settled(&context, &mut app, size);
        for _ in 0..12 {
            output = settled(&context, &mut app, size);
        }
        let rect = focused_rect(&context).expect("a card has keyboard focus");
        let top = text_bounds(&output, "Find a game, check that it is ready, then play.")
            .first()
            .map_or(0.0, |text| text.max.y + 4.0);
        let bottom = text_bounds(&output, "View Activity")
            .first()
            .map_or(size[1], |text| text.min.y);
        let body = egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(size[0], bottom));
        let shown = rect.intersect(body);
        if shown.height() < 20.0 {
            failures.push(format!(
                "blue={blue}: cold Shift+Tab focus {rect:?} is not in the page body {body:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Game Details, side-by-side columns: every right-hand control sits inside
/// the window at 1280x820 in physical pixels (scale 0.8 gives the room for the
/// two-column layout; the fixture's left column is narrower than a real
/// library's, so this guards the placement, not the real-library content).
#[test]
fn game_details_right_column_stays_inside_the_window_at_1280_by_820() {
    for scale in [0.8_f32, 1.0] {
        let size = points((1280.0, 820.0), scale);
        let context = egui::Context::default();
        let mut app = populated(&context, true, scale);
        app.router.current = Route::Game(1);
        let output = settled(&context, &mut app, size);
        let heading = text_bounds(&output, "Can I play it?");
        if scale == 0.8 {
            assert!(!heading.is_empty(), "two-column layout expected at 0.8");
        }
        for rect in heading {
            assert!(
                visible(rect, size),
                "scale {scale}: {rect:?} outside {size:?}"
            );
        }
        assert!(clipped_text(&output, size).is_empty());
    }
}

/// Library: selecting a game adds the breadcrumb trail; the controls under it
/// must not move (the row's height is reserved when there is no selection).
#[test]
fn library_controls_do_not_shift_when_a_game_is_selected() {
    let size = [1280.0, 820.0];
    let top_of_search = |route: Route| {
        let context = egui::Context::default();
        let mut app = populated(&context, true, 1.0);
        app.router.current = route;
        let output = settled(&context, &mut app, size);
        text_bounds(&output, "Search by title or system")
            .first()
            .map(|r| r.min.y)
    };
    let none = top_of_search(Route::BrowsePlay).expect("search shown");
    let selected = top_of_search(Route::BrowsePlayGame(1)).expect("search shown");
    assert!((none - selected).abs() < 1.0, "{none} vs {selected}");
}

fn compact_view(
    fix: Option<ReadinessAction>,
    pending: bool,
    spinner: bool,
    play_enabled: bool,
    reason: &str,
) -> launch_readiness_summary::CompactReadiness {
    launch_readiness_summary::CompactReadiness {
        badge: "Status",
        tone: launch_readiness_summary::ReadinessPresentationState::Ready,
        spinner,
        reason: reason.into(),
        headline: "headline".into(),
        play_enabled,
        fix,
        pending,
        more_findings: 0,
    }
}

/// The Library side panel's Play / Game details positions are identical in
/// Checking, Ready and a blocked answer with an extra fix button.
#[test]
fn side_panel_buttons_do_not_move_when_readiness_resolves() {
    let long = "Identity could not be confirmed. The file's own identity evidence is missing, \
        so EmuWiz cannot tell which game this is; review it before playing this game.";
    for (window, panel) in [
        ([1280.0_f32, 820.0], 400.0_f32),
        ([1024.0, 768.0], 320.0),
        ([900.0, 700.0], 260.0),
    ] {
        let states = [
            (
                "checking",
                compact_view(None, true, true, true, "Checking."),
            ),
            (
                "ready",
                compact_view(None, false, false, true, "Ready to play."),
            ),
            (
                "blocked",
                compact_view(
                    Some(ReadinessAction::ReviewIdentity),
                    false,
                    false,
                    false,
                    long,
                ),
            ),
        ];
        let mut ys = Vec::new();
        for (name, view) in &states {
            let context = egui::Context::default();
            let mut found = None;
            let mut output = None;
            for _ in 0..3 {
                output = Some(context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(window[0], window[1]),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            launch_readiness_summary::show_compact(
                                ui,
                                view,
                                panel,
                                3,
                                Some("Game details"),
                            );
                        });
                    },
                ));
            }
            let output = output.unwrap();
            let play = text_bounds(&output, "Play")[0].min.y;
            let details = text_bounds(&output, "Game details")[0].min.y;
            found.replace((play, details));
            ys.push((*name, found.unwrap()));
        }
        for pair in ys.windows(2) {
            assert!(
                (pair[0].1.0 - pair[1].1.0).abs() < 1.0 && (pair[0].1.1 - pair[1].1.1).abs() < 1.0,
                "{window:?}: {:?} vs {:?}",
                pair[0],
                pair[1]
            );
        }
    }
}

/// Review identity: Change system / Back to game keep their place while the
/// recorded evidence loads.
#[test]
fn review_identity_actions_do_not_move_when_evidence_loads() {
    for window in [[1280.0_f32, 820.0], [1024.0, 768.0], [900.0, 700.0]] {
        let mut ys = Vec::new();
        for loaded in [false, true] {
            let context = egui::Context::default();
            let mut app = populated(&context, true, 1.0);
            app.router.current = Route::ReviewIdentity(1);
            if loaded {
                app.review.knowledge_for = Some(1);
            }
            let output = settled(&context, &mut app, window);
            let back = text_bounds(&output, "Back to game");
            assert!(!back.is_empty(), "{window:?} loaded={loaded}");
            ys.push(back[0].min.y);
        }
        assert!((ys[0] - ys[1]).abs() < 1.0, "{window:?}: {ys:?}");
    }
}

/// Game Details at the real nav width: stacked at 1280x820, and nothing is
/// painted beyond the window's right edge at 1280, 1100 or 1500 wide.
#[test]
fn game_details_paints_nothing_beyond_the_window_edge() {
    fn max_right(shape: &egui::Shape, max: &mut f32) {
        match shape {
            egui::Shape::Text(t) => {
                *max = max.max(t.pos.x + t.galley.size().x);
            }
            egui::Shape::Rect(r) => *max = max.max(r.rect.max.x),
            egui::Shape::Vec(v) => v.iter().for_each(|s| max_right(s, max)),
            _ => {}
        }
    }
    for (width, height) in [(1280.0_f32, 820.0_f32), (1100.0, 800.0), (1500.0, 900.0)] {
        let size = [width, height];
        let context = egui::Context::default();
        let mut app = populated(&context, true, 1.0);
        app.router.current = Route::Game(1);
        let output = settled(&context, &mut app, size);
        let mut max = 0.0_f32;
        for clipped in &output.shapes {
            // Painted beyond the clip is invisible; measure what is shown.
            let mut right = 0.0_f32;
            max_right(&clipped.shape, &mut right);
            max = max.max(right.min(clipped.clip_rect.right()));
        }
        assert!(max <= width + 0.5, "{size:?}: painted to {max}");
        assert!(clipped_text(&output, size).is_empty(), "{size:?}");
        if width <= 1280.0 {
            assert!(
                text_bounds(&output, "Can I play it?").is_empty(),
                "{size:?} must use the stacked layout"
            );
        }
    }
}
