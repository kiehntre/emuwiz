//! "At a glance" on Game Details through the real `App::show`, synthetic data
//! only. Nothing here starts a launch or touches a real library.
use super::*;
use crate::gui_v2::library::Detail;

const DESKTOP: [f32; 2] = [1672.0, 941.0];

fn blue_app(context: &egui::Context, games: Vec<PersistedArchive>, open: i64) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    app.library = Arc::new(Library::new(games));
    app.loaded = true;
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Game(open);
    app
}

fn checked(game: i64, present: bool) -> Detail {
    Detail {
        game,
        file_present: present,
        unchanged: present,
        ..Detail::default()
    }
}

fn settled(context: &egui::Context, app: &mut App, size: [f32; 2]) -> egui::FullOutput {
    for _ in 0..3 {
        frame(context, app, size);
    }
    frame(context, app, size)
}

fn click(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    at: egui::Pos2,
) -> egui::FullOutput {
    let mut last = None;
    for pressed in [true, false] {
        last = Some(frame_with(
            context,
            app,
            size,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        ));
    }
    last.unwrap()
}

#[test]
fn blue_details_show_the_facts_in_the_hero_beside_readiness() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, vec![archive(7, "Fixture", Some("PSX"))], 7);
    app.detail = Some(checked(7, true));
    let output = settled(&context, &mut app, DESKTOP);
    let strings = text(&output);
    for wanted in [
        "At a glance",
        "System",
        "Region",
        "File format",
        "ISO · Game image",
        "Size",
        "4 B",
        "File",
        "Found, unchanged since the last scan",
        "Emulator",
        "Copy file path",
    ] {
        assert!(
            strings.iter().any(|line| line == wanted),
            "{wanted}: {strings:?}"
        );
    }
    assert!(
        strings.iter().any(|line| line == "Not recorded"),
        "{strings:?}"
    );
    let facts = text_bounds(&output, "At a glance")[0];
    let more = text_bounds(&output, "More for this game")[0];
    let readiness = text_bounds(&output, "Can I play it?")[0];
    assert!(facts.max.y <= more.min.y);
    assert!(readiness.min.x > facts.max.x);
}

#[test]
fn a_missing_file_reads_as_missing_and_is_never_offered_as_ready() {
    let context = egui::Context::default();
    let mut game = archive(7, "Fixture", Some("PSX"));
    game.last_known_health = "missing".into();
    game.last_verified_missing_at = Some("2026-11-01".into());
    let mut app = blue_app(&context, vec![game], 7);
    app.detail = Some(checked(7, false));
    let strings = text(&settled(&context, &mut app, DESKTOP));
    assert!(
        strings.iter().any(|line| line == "Not found"),
        "{strings:?}"
    );
    assert!(
        strings
            .iter()
            .any(|line| line == "The game file is unavailable"),
        "{strings:?}"
    );
    assert!(
        strings.iter().any(|line| line == "Check games folder"),
        "{strings:?}"
    );
    for wrong in ["Ready to play", "Ready", "Playable"] {
        assert!(
            !strings.iter().any(|line| line == wrong),
            "{wrong}: {strings:?}"
        );
    }
}

#[test]
fn a_missing_verdict_is_looked_at_again_on_the_next_visit() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, vec![archive(7, "Fixture", Some("PSX"))], 7);
    app.detail = Some(checked(7, true));
    app.navigation_changed();
    assert!(app.detail.is_some(), "a found file keeps its check");
    app.detail = Some(checked(7, false));
    app.navigation_changed();
    assert!(app.detail.is_none(), "a missing file must be rechecked");
}

/// The bar across the bottom of the window, which pages scroll under.
const STATUS_BAR: f32 = 56.0;

fn assert_first_screen(output: &egui::FullOutput, size: [f32; 2], wanted: &str) {
    let bounds = text_bounds(output, wanted);
    assert!(!bounds.is_empty(), "{wanted} missing: {:?}", text(output));
    assert!(
        bounds[0].max.y + 14.0 < size[1] - STATUS_BAR,
        "{wanted} is cut off at {size:?}: {:?}",
        bounds[0]
    );
}

#[test]
fn play_and_its_fix_are_on_the_first_screen_at_every_supported_size() {
    for size in [[1024.0, 600.0], [1280.0, 720.0], [1920.0, 1080.0]] {
        for blue in [true, false] {
            let context = egui::Context::default();
            let mut app = blue_app(&context, vec![archive(7, "Fixture", Some("PSX"))], 7);
            app.simple.enabled = blue;
            app.detail = Some(checked(7, true));
            let output = settled(&context, &mut app, size);
            assert_first_screen(&output, size, "Play");
        }
    }
}

#[test]
fn a_long_title_does_not_push_play_off_a_small_window() {
    let size = [1024.0, 600.0];
    let context = egui::Context::default();
    let title = "Fixture With A Very Long Name That Wraps (Europe) (En,Fr,De,Es,It) (Rev 1)";
    let mut app = blue_app(&context, vec![archive(7, title, Some("PSX"))], 7);
    app.detail = Some(checked(7, true));
    assert_first_screen(&settled(&context, &mut app, size), size, "Play");
}

#[test]
fn a_short_window_puts_quick_actions_and_facts_before_the_long_notes() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, vec![archive(7, "Fixture", Some("PSX"))], 7);
    app.detail = Some(checked(7, true));
    let short = settled(&context, &mut app, [1024.0, 600.0]);
    assert_first_screen(&short, [1024.0, 600.0], "Play");
    let tall = settled(&context, &mut app, [1024.0, 1600.0]);
    assert!(!text_bounds(&tall, "At a glance").is_empty());
    let painted = scrolled_text(&context, &mut app, [1024.0, 600.0]);
    let at = |wanted: &str| {
        painted
            .iter()
            .position(|line| line == wanted)
            .unwrap_or_else(|| panic!("{wanted}: {painted:?}"))
    };
    assert!(at("More for this game") < at("At a glance"));
    assert!(at("At a glance") < at("Save-state compatibility guidance"));
}

#[test]
fn a_short_window_drops_the_page_heading_but_a_tall_one_keeps_it() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, vec![archive(7, "Fixture", Some("PSX"))], 7);
    app.detail = Some(checked(7, true));
    let short = settled(&context, &mut app, [1280.0, 720.0]);
    assert!(text_bounds(&short, "Game Details").is_empty());
    let tall = settled(&context, &mut app, [1920.0, 1080.0]);
    assert!(!text_bounds(&tall, "Game Details").is_empty());
}

#[test]
fn copy_file_path_only_places_text_on_the_clipboard() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, vec![archive(7, "Fixture", Some("PSX"))], 7);
    app.detail = Some(checked(7, true));
    let output = settled(&context, &mut app, DESKTOP);
    let at = text_pos(&output, "Copy file path").unwrap() + egui::vec2(5.0, 5.0);
    let before = app.activity.jobs.len();
    let output = click(&context, &mut app, DESKTOP, at);
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(
                command,
                egui::OutputCommand::CopyText(copied) if copied == "/fixture/Fixture.iso"
            )),
        "{:?}",
        output.platform_output.commands
    );
    assert_eq!(app.activity.jobs.len(), before, "no job may start");
    assert_eq!(app.router.current, Route::Game(7));
}

fn disc_set(root: &std::path::Path) -> Vec<PersistedArchive> {
    [(1, 1), (2, 2)]
        .map(|(id, number)| {
            let path = root.join(format!("Fixture (Disc {number}).iso"));
            fs::write(&path, b"disc").unwrap();
            let mut game = archive(id, &format!("Fixture (Disc {number})"), Some("PSX"));
            game.absolute_path = path;
            game
        })
        .to_vec()
}

#[test]
fn a_disc_set_appears_without_the_database_snapshot_and_a_link_only_navigates() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = blue_app(&context, disc_set(root.path()), 1);
    app.detail = Some(checked(1, true));
    let size = [1920.0, 1400.0];
    frame(&context, &mut app, size);
    let workflows = app.native_workflows.as_ref().unwrap();
    assert!(workflows.app.database_state.snapshot().is_none());
    // The fixture has no worker, so run the job the first frame asked for.
    let library = app.library.clone();
    let _ = app
        .game_detail_facts
        .should_look_up(library.game(1).unwrap(), &library);
    let found = crate::gui_v2::game_detail_facts::look_up(&library, 1).unwrap();
    app.game_detail_facts.finish(found);
    let output = settled(&context, &mut app, size);
    let strings = text(&output);
    let headline = strings
        .iter()
        .find(|line| line.starts_with("2-disc game"))
        .unwrap_or_else(|| panic!("{strings:?}"));
    assert!(headline.contains("order not verified"), "{headline}");
    assert_eq!(strings.iter().filter(|line| *line == headline).count(), 2);
    let link = strings
        .iter()
        .find(|line| line.starts_with("Go to Disc 2"))
        .unwrap_or_else(|| panic!("{strings:?}"));
    let at = text_pos(&output, link).unwrap() + egui::vec2(5.0, 5.0);
    let jobs: Vec<_> = app.activity.jobs.keys().copied().collect();
    click(&context, &mut app, size, at);
    assert_eq!(app.router.current, Route::Game(2));
    // A launch would add a job and an attempt; following a link adds neither.
    let launched = settled(&context, &mut app, size);
    let workflows = app.native_workflows.as_ref().unwrap();
    assert!(!workflows.launch_was_requested(1));
    assert!(!workflows.launch_was_requested(2));
    assert!(
        app.activity
            .jobs
            .iter()
            .filter(|(id, _)| !jobs.contains(id))
            .all(|(_, job)| !job.title.to_lowercase().contains("launch")),
    );
    assert!(visible_text(&launched, "Fixture (Disc 2)"));
}
