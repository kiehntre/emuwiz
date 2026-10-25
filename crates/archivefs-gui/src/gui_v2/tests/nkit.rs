use super::*;

#[test]
fn game_details_offer_nkit_representation_guidance_without_conversion_actions() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("ordinary.iso");
    let mut bytes = vec![0u8; 0x2440];
    bytes[..6].copy_from_slice(b"GTSE01");
    bytes[0x1c..0x20].copy_from_slice(&0xc2339f3du32.to_be_bytes());
    bytes[0x200..0x208].copy_from_slice(b"NKIT v01");
    bytes[0x210..0x214].copy_from_slice(&0x57058000u32.to_be_bytes());
    fs::write(&path, &bytes).unwrap();
    let mut game = archive(7, "Synthetic NKit", Some("GameCube"));
    game.absolute_path = path.clone();
    app.library = Arc::new(Library::new(vec![game]));
    app.artwork.index = Some(Arc::new(MediaIndex::default()));
    app.router.current = Route::Game(7);
    let output = frame(&context, &mut app, [1280.0, 4800.0]);
    assert!(
        text(&output)
            .iter()
            .any(|line| line == "Disc representation & preservation")
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut output = output;
    while !text(&output)
        .iter()
        .any(|line| line.contains("NKit v1 header recognized"))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "bounded header inspection did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
        output = frame(&context, &mut app, [1280.0, 4800.0]);
    }
    let strings = text(&output);
    assert!(
        strings
            .iter()
            .any(|line| line.contains("disc body unverified"))
    );
    assert!(strings.iter().any(|line| line.contains("exact DAT match")));
    assert!(
        strings
            .iter()
            .any(|line| line.contains("Recoverability has not been established"))
    );
    assert!(
        !strings
            .iter()
            .any(|line| line == "Convert to ISO" || line == "Restore NKit")
    );
    app.router.current = Route::Task {
        section: Section::Launch,
        game: 7,
    };
    let output = frame(&context, &mut app, [1280.0, 4800.0]);
    assert!(
        text(&output)
            .iter()
            .any(|line| line.contains("NKit v1 header recognized"))
    );
    let position =
        text_pos(&output, "Verify stored representation").unwrap() + egui::vec2(5.0, 5.0);
    for pressed in [true, false] {
        frame_with(
            &context,
            &mut app,
            [1280.0, 4800.0],
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
    assert_eq!(
        app.router.current,
        Route::Task {
            section: Section::Check,
            game: 7
        }
    );
    assert_eq!(app.check_platform.as_deref(), Some("GameCube"));
    assert_eq!(fs::read(path).unwrap(), bytes);
}
