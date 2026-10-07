use super::*;
use crate::gui_v2::manual_viewer::tests::cbz;

#[test]
fn manual_viewer_opens_from_existing_artwork_manual_panel_without_desktop_handler() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Rez.cbz");
    cbz(&path);
    let mut game = archive(9, "Rez", Some("Dreamcast"));
    game.absolute_path = root.path().join("Rez.iso");
    app.library = Arc::new(Library::new(vec![game]));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    let route = Route::Task {
        section: Section::Artwork,
        game: 9,
    };
    app.router.current = route.clone();
    frame(&context, &mut app, [1280.0, 2400.0]);
    let documents = &mut app.document_cache.as_mut().unwrap().2;
    assert_eq!(documents.len(), 1);
    documents[0].viewer = documents::DocumentOpenCapability::NoHandler;
    let output = frame(&context, &mut app, [1280.0, 2400.0]);
    let strings = text(&output);
    assert!(strings.iter().any(|s| s == "Manuals & Guides"));
    assert!(strings.iter().any(|s| s == "Open externally"));
    let position = text_pos(&output, "Open internally").unwrap() + egui::vec2(8.0, 8.0);
    for pressed in [true, false] {
        frame_with(
            &context,
            &mut app,
            [1280.0, 2400.0],
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.document_preferences.reading.contains_key(&path) {
        frame(&context, &mut app, [1280.0, 2400.0]);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(app.document_preferences.reading[&path].last_page, Some(1));
    assert_eq!(app.router.current, route);
    frame_with(
        &context,
        &mut app,
        [1280.0, 2400.0],
        vec![key_event(egui::Key::Escape)],
    );
    assert_eq!(app.router.current, route);
}

#[test]
fn manual_viewer_escape_preserves_game_document_context_and_history() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.go(Route::Section(Section::Games));
    app.go(Route::Game(42));
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    cbz(&path);
    app.document_cache = Some((42, "game.rom".into(), Vec::new()));
    app.manual_viewer
        .open(&context, path, "Game manual".into(), None);
    frame_with(
        &context,
        &mut app,
        [1024.0, 700.0],
        vec![key_event(egui::Key::Escape)],
    );
    assert_eq!(app.router.current, Route::Game(42));
    assert!(
        app.document_cache.is_none(),
        "closing re-inspects associated files on the next visit"
    );
    assert!(!app.show_manual_viewer(&context));
    // Closing did not add/remove history entries either.
    app.back();
    assert_eq!(app.router.current, Route::Section(Section::Games));
}

#[test]
fn manual_viewer_renders_controls_and_updates_existing_preference_path() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.go(Route::Game(42));
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    cbz(&path);
    let saved = documents::DocumentReadingState {
        last_page: Some(2),
        zoom_percent: Some(150),
        document_id: Some(
            archivefs_core::manual_document::ManualDocument::open(
                &path,
                &archivefs_core::manual_document::ManualLimits::default(),
            )
            .unwrap()
            .id()
            .clone(),
        ),
        ..Default::default()
    };
    app.document_preferences
        .reading
        .insert(path.clone(), saved.clone());
    app.manual_viewer
        .open(&context, path.clone(), "Game manual".into(), Some(saved));
    let deadline = Instant::now() + Duration::from_secs(10);
    let strings = loop {
        let strings = text(&frame(&context, &mut app, [1024.0, 700.0]));
        if strings.iter().any(|s| s == "Page 2 of 3") {
            break strings;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    for label in [
        "Game manual",
        "First",
        "Previous",
        "Next",
        "Last",
        "Zoom −",
        "Zoom +",
        "Fit Page",
        "Fit Width",
        "Close",
        "150%",
        "page2.png",
    ] {
        assert!(strings.iter().any(|s| s == label), "missing {label}");
    }
    frame_with(
        &context,
        &mut app,
        [1024.0, 700.0],
        vec![key_event(egui::Key::End)],
    );
    assert_eq!(app.document_preferences.reading[&path].last_page, Some(3));
    assert!(app.preferences_dirty.is_some());
    frame_with(
        &context,
        &mut app,
        [1024.0, 700.0],
        vec![key_event(egui::Key::Equals)],
    );
    assert_eq!(
        app.document_preferences.reading[&path].zoom_percent,
        Some(200)
    );
    frame_with(
        &context,
        &mut app,
        [1024.0, 700.0],
        vec![key_event(egui::Key::Escape)],
    );
    assert_eq!(app.router.current, Route::Game(42));
    assert_eq!(app.document_preferences.reading[&path].last_page, Some(3));
}

#[test]
fn manual_viewer_reading_state_uses_existing_atomic_preferences_file() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    cbz(&path);
    app.manual_viewer
        .open(&ctx, path.clone(), "Manual".into(), None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.document_preferences.reading.contains_key(&path) {
        frame(&ctx, &mut app, [1024.0, 700.0]);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    frame_with(
        &ctx,
        &mut app,
        [1024.0, 700.0],
        vec![key_event(egui::Key::End)],
    );
    let settings = root.path().join("gui-v2.json");
    let preferences = Preferences {
        document_reading: app.document_preferences.reading.clone(),
        ..Preferences::default()
    };
    backend::save_preferences(&settings, &preferences).unwrap();
    let restored: Preferences = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(restored.document_reading[&path].last_page, Some(3));
    assert_eq!(
        restored.document_reading[&path].document_id,
        app.document_preferences.reading[&path].document_id
    );
}
