//! Check Hasheous on Blue's Game Details through the real `App::show`, with a
//! fake lookup. Opening the page contacts nothing; only the click does.
use super::*;
use crate::selected_evidence_page::HasheousCheckOutcome;
use archivefs_core::dat::library_identity_summary::LibraryItemHashes;
use std::sync::atomic::AtomicUsize;

const SIZE: [f32; 2] = [1672.0, 3000.0];
const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";

fn app(context: &egui::Context, calls: &Arc<AtomicUsize>) -> App {
    let mut app = fixture(context);
    app.simple.enabled = true;
    let mut library = Library::new(vec![
        archive(7, "Fixture Quest", Some("GB")),
        archive(8, "Other Game", Some("GB")),
    ]);
    library.games[0].saved_hashes = Some(LibraryItemHashes {
        sha1: Some(SHA1.into()),
        ..LibraryItemHashes::default()
    });
    app.library = Arc::new(library);
    app.loaded = true;
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Game(7);
    let counted = Arc::clone(calls);
    app.hasheous_check = super::super::hasheous_check::State::with(
        Arc::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            HasheousCheckOutcome::NoMatch
        }),
        Arc::new(archivefs_core::safe_read::TrustedRoots::none),
    );
    app
}

fn settled(context: &egui::Context, app: &mut App) -> egui::FullOutput {
    for _ in 0..3 {
        frame(context, app, SIZE);
    }
    frame(context, app, SIZE)
}

#[test]
fn blue_game_details_check_hasheous_runs_only_when_clicked() {
    let context = egui::Context::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = app(&context, &calls);
    let output = settled(&context, &mut app);
    let strings = text(&output);
    assert!(strings.iter().any(|s| s == "Check Hasheous"), "{strings:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "opening the page sent a lookup"
    );

    let at = text_bounds(&output, "Check Hasheous")[0].center();
    for pressed in [true, false] {
        frame_with(
            &context,
            &mut app,
            SIZE,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "click started nothing"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut strings = Vec::new();
    while !strings
        .iter()
        .any(|s: &String| s == super::super::hasheous_check::NO_MATCH)
    {
        assert!(std::time::Instant::now() < deadline, "{strings:?}");
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.hasheous_check.poll();
        strings = text(&settled(&context, &mut app));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Another game never shows this game's answer. With no saved SHA-1 it
    // offers to work one out, and nothing is read until that click.
    app.router.current = Route::Game(8);
    let other = text(&settled(&context, &mut app));
    assert!(
        !other
            .iter()
            .any(|s| s.contains("matched the submitted SHA-1"))
    );
    assert!(
        other
            .iter()
            .any(|s| s == super::super::hasheous_check::IDLE_HASH_FIRST),
        "{other:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

fn press(context: &egui::Context, app: &mut App, size: [f32; 2], at: egui::Pos2) {
    for pressed in [true, false] {
        frame_with(
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
        );
    }
}

#[test]
fn the_identity_shortcut_reveals_and_focuses_the_lookup_without_sending() {
    use super::super::hasheous_check::SHORTCUT_LABEL;
    const SMALL: [f32; 2] = [1024.0, 600.0];
    let context = egui::Context::default();
    context.style_mut(|style| style.animation_time = 0.0);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut app = app(&context, &calls);
    for _ in 0..3 {
        frame(&context, &mut app, SMALL);
    }
    let output = frame(&context, &mut app, SMALL);
    // On the first screen of the smallest supported window, beside Identity.
    assert!(visible_text(&output, SHORTCUT_LABEL), "{:?}", text(&output));
    assert!(!visible_text(&output, "Hasheous (online)"));
    let at = text_bounds(&output, SHORTCUT_LABEL)[0].center();
    press(&context, &mut app, SMALL, at);
    let mut revealed = frame(&context, &mut app, SMALL);
    for _ in 0..3 {
        revealed = frame(&context, &mut app, SMALL);
    }
    assert!(
        visible_text(&revealed, "Hasheous (online)"),
        "{:?}",
        text(&revealed)
    );
    assert!(app.router.current == Route::Game(7));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the shortcut sent a lookup"
    );
    // The section's button now has focus: Enter (keyboard, or a controller's
    // confirm mapped to it) is what starts the lookup.
    frame_with(
        &context,
        &mut app,
        SMALL,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    frame_with(
        &context,
        &mut app,
        SMALL,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "Enter on the focused button did nothing"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
