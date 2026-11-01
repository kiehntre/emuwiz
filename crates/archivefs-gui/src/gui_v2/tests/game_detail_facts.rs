//! "At a glance" on Game Details through the real `App::show`, synthetic data
//! only. Nothing here starts a launch or touches a real library.
use super::*;
use crate::gui_v2::library::Detail;

const DESKTOP: [f32; 2] = [1672.0, 941.0];
const COMPACT: [f32; 2] = [1024.0, 600.0];

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

#[test]
fn compact_details_keep_the_title_and_readiness_above_the_facts() {
    let context = egui::Context::default();
    let mut app = blue_app(&context, vec![archive(7, "Fixture", Some("PSX"))], 7);
    app.detail = Some(checked(7, true));
    let tall = settled(&context, &mut app, [COMPACT[0], 1800.0]);
    let facts = text_bounds(&tall, "At a glance");
    let more = text_bounds(&tall, "More for this game");
    assert!(!facts.is_empty() && !more.is_empty(), "{:?}", text(&tall));
    assert!(facts[0].min.y >= more[0].max.y);
    assert!(text_bounds(&tall, "Play")[0].max.y < facts[0].min.y);
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

#[test]
fn a_real_disc_set_is_summarised_and_links_to_the_other_disc() {
    let root = tempfile::tempdir().unwrap();
    let mut games = Vec::new();
    for (id, number) in [(1, 1), (2, 2)] {
        let path = root.path().join(format!("Fixture (Disc {number}).iso"));
        fs::write(&path, b"disc").unwrap();
        let mut game = archive(id, &format!("Fixture (Disc {number})"), Some("PSX"));
        game.absolute_path = path;
        games.push(game);
    }
    let context = egui::Context::default();
    let mut app = blue_app(&context, games.clone(), 1);
    app.detail = Some(checked(1, true));
    frame(&context, &mut app, DESKTOP);
    app.native_workflows.as_mut().unwrap().app.database_state = crate::DatabaseState::Ready {
        snapshot: Box::new(crate::tests::cached_snapshot(games)),
        last_scan_summary: None,
    };
    let output = settled(&context, &mut app, [1672.0, 2400.0]);
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
    click(&context, &mut app, [1672.0, 2400.0], at);
    assert_eq!(app.router.current, Route::Game(2));
}
