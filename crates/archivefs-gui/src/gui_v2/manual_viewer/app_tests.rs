use super::*;
use crate::gui_v2::manual_viewer::tests::{cbr, cbz};

fn wait_for_documents(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    count: usize,
) -> egui::FullOutput {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = frame(context, app, size);
        if !app.document_refresh.stale()
            && app
                .document_cache
                .as_ref()
                .is_some_and(|(_, _, docs)| docs.len() == count)
        {
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "document discovery did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

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
    wait_for_documents(&context, &mut app, [1280.0, 2400.0], 1);
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

#[test]
fn manual_viewer_reads_cbr_pages_inside_the_app_at_1024_by_600() {
    const SIZE: [f32; 2] = [1024.0, 600.0];
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.go(Route::Game(42));
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("guide.cbr");
    cbr(&path);
    let before = fs::read(&path).unwrap();
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
        .open(&context, path.clone(), "Strategy guide".into(), Some(saved));
    let deadline = Instant::now() + Duration::from_secs(10);
    let (strings, output) = loop {
        let output = frame(&context, &mut app, SIZE);
        let strings = text(&output);
        // Natural order: page1, page2, page10 -> resumed page 2 is "page2.png".
        if strings.iter().any(|s| s == "Page 2 of 3") && strings.iter().any(|s| s == "page2.png") {
            break (strings, output);
        }
        assert!(
            Instant::now() < deadline,
            "CBR never became viewable: {strings:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    for label in [
        "Strategy guide",
        "First",
        "Previous",
        "Next",
        "Last",
        "Close",
        "150%",
    ] {
        assert!(strings.iter().any(|s| s == label), "missing {label}");
    }
    for label in ["Previous", "Next", "Zoom −", "Zoom +", "Close"] {
        let position = text_pos(&output, label).expect("viewer control is laid out");
        assert!(
            position.x >= 0.0 && position.x < SIZE[0],
            "{label} is outside the 1024 px width"
        );
        assert!(
            position.y >= 0.0 && position.y < SIZE[1],
            "{label} is outside the 600 px height"
        );
    }
    assert!(
        !strings.iter().any(|s| s.contains("cannot show")),
        "CBR must not be reported unsupported: {strings:?}"
    );
    frame_with(&context, &mut app, SIZE, vec![key_event(egui::Key::End)]);
    assert_eq!(app.document_preferences.reading[&path].last_page, Some(3));
    frame_with(&context, &mut app, SIZE, vec![key_event(egui::Key::Equals)]);
    assert_eq!(
        app.document_preferences.reading[&path].zoom_percent,
        Some(200)
    );
    frame_with(&context, &mut app, SIZE, vec![key_event(egui::Key::Escape)]);
    assert_eq!(app.router.current, Route::Game(42));
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "the CBR must stay untouched"
    );
}

#[test]
fn selected_game_details_show_existing_guides_and_refresh_after_provider_index_change() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let root = tempfile::tempdir().unwrap();
    let mut game = archive(9, "Rez", Some("Dreamcast"));
    game.absolute_path = root.path().join("Rez.iso");
    app.library = Arc::new(Library::new(vec![game]));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Game(9);
    let output = wait_for_documents(&context, &mut app, [1280.0, 4800.0], 0);
    let strings = text(&output);
    assert!(strings.iter().any(|s| s == "Manuals & Guides"));
    assert!(strings.iter().any(|s| s == "Associate local document…"));
    assert!(app.document_cache.as_ref().unwrap().2.is_empty());
    let path = root.path().join("Strategy guide.cbz");
    cbz(&path);
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    let output = wait_for_documents(&context, &mut app, [1280.0, 4800.0], 1);
    let strings = text(&output);
    assert!(strings.iter().any(|s| s == "Open internally"));
    assert!(strings.iter().any(|s| s == "Strategy guide"));
    let docs = &app.document_cache.as_ref().unwrap().2;
    assert_eq!(docs.len(), 1);
    assert_eq!(
        docs[0].association,
        documents::GameDocumentAssociation::SameGameDirectory
    );
    assert_eq!(app.router.current, Route::Game(9));
}

#[test]
fn selected_game_details_refreshes_a_removed_document_at_1024_by_600() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Rez.cbz");
    cbz(&path);
    let mut game = archive(9, "Rez", Some("Dreamcast"));
    game.absolute_path = root.path().join("Rez.iso");
    app.library = Arc::new(Library::new(vec![game]));
    app.router.current = Route::Game(9);
    app.document_preferences
        .associations
        .insert(path.clone(), 9);
    wait_for_documents(&context, &mut app, [1024.0, 600.0], 1);
    assert!(
        app.document_cache.as_ref().unwrap().2[0]
            .document_id
            .is_some()
    );
    fs::remove_file(&path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        frame(&context, &mut app, [1024.0, 600.0]);
        if app.document_cache.as_ref().unwrap().2[0].viewer
            == documents::DocumentOpenCapability::MissingFile
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "background refresh did not detect removal"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let document = &app.document_cache.as_ref().unwrap().2[0];
    assert_eq!(
        document.viewer,
        documents::DocumentOpenCapability::MissingFile
    );
    assert!(document.document_id.is_none());
    assert_eq!(
        document.association,
        documents::GameDocumentAssociation::Explicit
    );
}

#[test]
fn selected_game_details_refreshes_changed_comic_format() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Rez.cbz");
    cbz(&path);
    let mut game = archive(9, "Rez", Some("Dreamcast"));
    game.absolute_path = root.path().join("Rez.iso");
    app.library = Arc::new(Library::new(vec![game]));
    app.router.current = Route::Game(9);
    wait_for_documents(&context, &mut app, [1024.0, 600.0], 1);
    assert_eq!(
        app.document_cache.as_ref().unwrap().2[0].format,
        documents::GameDocumentFormat::Cbz
    );
    cbr(&path);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        frame(&context, &mut app, [1024.0, 600.0]);
        if app.document_cache.as_ref().unwrap().2[0].format == documents::GameDocumentFormat::Cbr {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "background refresh did not detect replacement"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.document_cache.as_ref().unwrap().2[0]
            .document_id
            .is_some()
    );
}

#[test]
fn blocked_document_mount_does_not_stall_game_details_or_leave_readiness_trusted() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Rez.cbz");
    cbz(&path);
    let mut game = archive(9, "Rez", Some("Dreamcast"));
    game.absolute_path = root.path().join("Rez.iso");
    app.library = Arc::new(Library::new(vec![game.clone()]));
    let game = app.library.game(9).unwrap().clone();
    app.router.current = Route::Game(9);
    wait_for_documents(&context, &mut app, [1024.0, 600.0], 1);
    let release = app.document_refresh.block_worker_for_test();
    let start = Instant::now();
    frame(&context, &mut app, [1024.0, 600.0]);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "slow mount blocked the GUI frame"
    );
    app.document_refresh.expire_for_test();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.documents_panel(ui, 9, &game));
        },
    );
    let strings = text(&output);
    assert!(
        strings
            .iter()
            .any(|line| line.contains("Opening is paused")),
        "stale readiness was not labelled: {strings:?}"
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.document_refresh.stale() {
        frame(&context, &mut app, [1024.0, 600.0]);
        assert!(
            Instant::now() < deadline,
            "freshness did not recover after the mount returned"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn blocked_first_document_load_and_manual_refresh_do_not_block_the_gui() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Rez.cbz");
    cbz(&path);
    let mut game = archive(9, "Rez", Some("Dreamcast"));
    game.absolute_path = root.path().join("Rez.iso");
    app.library = Arc::new(Library::new(vec![game]));
    app.router.current = Route::Game(9);

    let release = app.document_refresh.block_worker_for_test();
    let start = Instant::now();
    frame(&context, &mut app, [1024.0, 600.0]);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "first discovery blocked the GUI"
    );
    assert!(app.document_refresh.stale());
    assert!(app.document_cache.as_ref().unwrap().2.is_empty());
    release.send(()).unwrap();
    wait_for_documents(&context, &mut app, [1024.0, 600.0], 1);

    let release = app.document_refresh.block_worker_for_test();
    app.document_cache = None; // The Refresh documents button invalidates this cache.
    let start = Instant::now();
    frame(&context, &mut app, [1024.0, 600.0]);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "manual refresh blocked the GUI"
    );
    assert!(app.document_refresh.stale());
    release.send(()).unwrap();
    wait_for_documents(&context, &mut app, [1024.0, 600.0], 1);
}
