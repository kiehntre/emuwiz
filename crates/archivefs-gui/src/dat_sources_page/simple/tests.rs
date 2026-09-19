use super::*;
use std::time::{Duration, Instant};

const MAME: &str = r#"<?xml version="1.0"?><mame build="0.174"><machine name="test"><description>Test Arcade Game</description><year>1990</year><manufacturer>Test</manufacturer><rom name="game.bin" size="3" crc="352441c2" sha1="a9993e364706816aba3e25717850c26c9cd0d89d"/></machine></mame>"#;

fn page(root: &Path) -> DatSourcesPageState {
    DatSourcesPageState::load_with_transaction_dir_and_managed_paths(
        root.join("dat_sources.toml"),
        vec![root.join("games")],
        TrustedRoots::none(),
        root.join("journals"),
        root.join("managed.toml"),
        root.join("managed"),
    )
}

fn finish(page: &mut DatSourcesPageState) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while page.is_busy() {
        page.poll();
        assert!(Instant::now() < deadline, "worker timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    page.poll();
}

fn import(page: &mut DatSourcesPageState, path: &Path) -> String {
    page.apply(DatSourcesPageAction::ImportVerificationData { path: path.into() });
    finish(page);
    page.draft
        .entries()
        .iter()
        .find(|row| row.path == path)
        .unwrap()
        .id
        .clone()
}

fn text_shapes(shape: &egui::epaint::Shape, lines: &mut Vec<String>) {
    match shape {
        egui::epaint::Shape::Text(text) => lines.push(text.galley.text().to_string()),
        egui::epaint::Shape::Vec(shapes) => {
            for shape in shapes {
                text_shapes(shape, lines);
            }
        }
        _ => {}
    }
}

fn render(view: &DatSourcesPageView, state: &mut DatSourcesPageUi) -> Vec<String> {
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 2200.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(show(ui, view, state).is_none());
            });
        },
    );
    let mut lines = Vec::new();
    for shape in output.shapes {
        text_shapes(&shape.shape, &mut lines);
    }
    lines
}

fn click(
    view: &DatSourcesPageView,
    state: &mut DatSourcesPageUi,
    label: &str,
) -> DatSourcesPageAction {
    let ctx = egui::Context::default();
    let mut frame = |events| {
        let mut action = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 2200.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = show(ui, view, state);
                });
            },
        );
        (output, action)
    };
    let _ = frame(vec![]);
    let (output, _) = frame(vec![]);
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing button {label}"));
    frame(vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ])
    .1
    .unwrap_or_else(|| panic!("{label} emitted no action"))
}

#[test]
fn simple_mame_import_confirm_save_verify_survives_reload_without_executable() {
    let dir = tempfile::tempdir().unwrap();
    let dat = dir.path().join("unhelpful-name.dat");
    std::fs::write(&dat, MAME).unwrap();
    let games = dir.path().join("games");
    std::fs::create_dir(&games).unwrap();
    std::fs::write(games.join("game.bin"), b"abc").unwrap();
    let mut state = page(dir.path());
    let id = import(&mut state, &dat);
    let view = state.view();
    assert_eq!(
        view.rows[0].arcade_verification.as_deref(),
        Some("MAME 0.174 Arcade")
    );
    assert_eq!(arcade_ready_count(&view), 1);
    assert!(
        view.rows[0].platform_id.is_none(),
        "assignment needs consent"
    );
    let text = render(&view, &mut Default::default()).join("\n");
    assert!(text.contains("Use it for Arcade?"), "{text}");
    assert!(text.contains("Yes — use for Arcade"));
    let mut setup_ui = DatSourcesPageUi::default();
    let action = click(&view, &mut setup_ui, "Yes — use for Arcade");
    assert_eq!(
        action,
        DatSourcesPageAction::SetPlatform {
            id: id.clone(),
            platform: Some("Arcade".into())
        }
    );
    state.apply(action);
    let action = click(&state.view(), &mut setup_ui, "Save setup");
    state.apply(action);
    assert!(!state.is_dirty());
    let mut reloaded = page(dir.path());
    let choices = choices(&reloaded.view(), &[], "Arcade");
    assert_eq!(status(&choices, None, true), Status::Ready);
    let mut ui = DatSourcesPageUi::default();
    ui.simple.platform = Some("Arcade".into());
    let text = render(&reloaded.view(), &mut ui);
    assert_eq!(
        text.iter()
            .filter(|text| text.as_str() == "Verify Arcade Collection")
            .count(),
        1
    );
    assert!(text.iter().any(|t| t.contains("optional")));
    assert!(
        !text
            .iter()
            .any(|t| t.contains("snapshot") || t.contains("software lists"))
    );
    assert!(text.iter().any(|t| t == "← Back to platforms"));
    let action = click(&reloaded.view(), &mut ui, "Verify Arcade Collection");
    assert_eq!(
        action,
        DatSourcesPageAction::Audit {
            id,
            scan_root: games.clone()
        }
    );
    reloaded.apply(action);
    finish(&mut reloaded);
    let view = reloaded.view();
    assert!(view.audit_error.is_none(), "{:?}", view.audit_error);
    let audit = view.audit.unwrap();
    assert!(
        audit
            .categories
            .iter()
            .any(|category| category.label == "Exact" && category.count == 1)
    );
    assert_eq!(std::fs::read(&dat).unwrap(), MAME.as_bytes());
    assert_eq!(std::fs::read(games.join("game.bin")).unwrap(), b"abc");
}

#[test]
fn simple_mame_software_lists_and_filename_clues_never_become_arcade() {
    let dir = tempfile::tempdir().unwrap();
    let dat = dir.path().join("MAME.0.174.Arcade.XML.dat");
    std::fs::write(&dat, r#"<softwarelist name="test" description="MAME Arcade"><software name="test"><description>Test</description><year>1990</year><publisher>Test</publisher><part name="cart" interface="cart"><dataarea name="rom" size="3"><rom name="test" size="3" crc="352441c2"/></dataarea></part></software></softwarelist>"#).unwrap();
    let mut state = page(dir.path());
    import(&mut state, &dat);
    assert_eq!(arcade_ready_count(&state.view()), 0);
    assert!(state.view().rows[0].arcade_verification.is_none());
}

#[test]
fn simple_ambiguous_catalogues_ask_instead_of_guessing() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = page(dir.path());
    for name in ["one.dat", "two.dat"] {
        let dat = dir.path().join(name);
        std::fs::write(&dat, MAME).unwrap();
        let id = import(&mut state, &dat);
        state.apply(DatSourcesPageAction::SetPlatform {
            id,
            platform: Some("Arcade".into()),
        });
    }
    let choices = choices(&state.view(), &[], "Arcade");
    assert!(selected_choice(&choices, None).is_none());
    assert_eq!(status(&choices, None, true), Status::NeedsAttention);
    assert_eq!(
        status(&choices, Some(&choices[0].reference), true),
        Status::Ready
    );
    let mut ui = DatSourcesPageUi::default();
    ui.simple.platform = Some("Arcade".into());
    let text = render(&state.view(), &mut ui).join("\n");
    assert!(text.contains("More than one set of verification data"));
    assert!(text.contains("Choose verification data"));
}

#[test]
fn simple_stale_missing_disabled_and_empty_data_are_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let dat = dir.path().join("arcade.dat");
    std::fs::write(&dat, MAME).unwrap();
    let mut state = page(dir.path());
    let id = import(&mut state, &dat);
    state.apply(DatSourcesPageAction::SetPlatform {
        id: id.clone(),
        platform: Some("Arcade".into()),
    });
    state.apply(DatSourcesPageAction::SetEnabled {
        id: id.clone(),
        enabled: false,
    });
    assert!(choices(&state.view(), &[], "Arcade").is_empty());
    state.apply(DatSourcesPageAction::SetEnabled {
        id: id.clone(),
        enabled: true,
    });
    std::fs::write(&dat, format!("{MAME}\n")).unwrap();
    assert_eq!(arcade_ready_count(&state.view()), 0);
    assert_eq!(
        status(&choices(&state.view(), &[], "Arcade"), None, true),
        Status::NeedsAttention
    );
    std::fs::remove_file(&dat).unwrap();
    assert_eq!(
        status(&choices(&state.view(), &[], "Arcade"), None, true),
        Status::NeedsAttention
    );
    assert_eq!(status(&[], None, true), Status::NeedsSetup);
    assert_eq!(status(&[], None, false), Status::NotSupported);
    assert_eq!(
        status(&[], Some(&CatalogueRef::local(id)), true),
        Status::NeedsAttention
    );
}

#[test]
fn simple_exploration_does_not_start_jobs_save_or_activate() {
    let dir = tempfile::tempdir().unwrap();
    let state = page(dir.path());
    let view = state.view();
    let mut ui = DatSourcesPageUi::default();
    for platform in [None, Some("Arcade"), Some("Amiga"), Some("Dreamcast")] {
        ui.simple.platform = platform.map(str::to_string);
        ui.simple.setup = true;
        let text = render(&view, &mut ui).join("\n");
        assert!(text.contains("Check My Games"));
        assert!(text.contains("Advanced details"));
        assert!(!text.contains("Activate") && !text.contains("provider"));
    }
    assert!(!state.is_busy() && !state.is_dirty());
    assert!(!dir.path().join("dat_sources.toml").exists());
}

#[test]
fn simple_identify_separates_arcade_and_software_list_readiness() {
    let dir = tempfile::tempdir().unwrap();
    let dat = dir.path().join("arcade.dat");
    std::fs::write(&dat, MAME).unwrap();
    let mut state = page(dir.path());
    import(&mut state, &dat);
    assert_eq!(arcade_ready_count(&state.view()), 1);
    assert!(!state.view().managed_rows.iter().any(|row| row.installed));
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 2200.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_identify_rename_page(ui, &state.view(), &mut Default::default());
            });
        },
    );
    let mut text = Vec::new();
    for shape in output.shapes {
        text_shapes(&shape.shape, &mut text);
    }
    assert!(text.iter().any(|s| s == "Arcade verification data"));
    assert!(
        !text
            .iter()
            .any(|s| s == "MAME" || s.contains("0 installed software"))
    );
}
