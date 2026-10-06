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

#[test]
fn a_platform_chosen_by_the_caller_survives_into_setup_with_plain_guidance() {
    for platform in [
        "BBC Micro",
        "Game Boy Advance",
        "Amiga",
        "Atari ST",
        "ZX Spectrum",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let page = page(dir.path());
        let view = page.view();
        let mut state = DatSourcesPageUi::default();
        state.simple.select_platform(platform);
        assert_eq!(state.simple.platform(), Some(platform));
        let lines = render(&view, &mut state);
        // Still on the chosen platform: no chooser, no searching again.
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("Check My Games › {platform}"))),
            "{platform}: {lines:?}"
        );
        assert!(!lines.iter().any(|line| line.contains("Choose a platform")));
        assert_eq!(state.simple.platform(), Some(platform));
    }
}

#[test]
fn missing_identification_data_opens_setup_and_says_so_in_plain_words() {
    let dir = tempfile::tempdir().unwrap();
    let page = page(dir.path());
    let view = page.view();
    let mut state = DatSourcesPageUi::default();
    state.simple.select_platform("Game Boy Advance");
    let lines = render(&view, &mut state);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("identification data is not set up yet"))
    );
    assert!(lines.iter().any(|l| l.contains("Set up Game Boy Advance")));
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("DAT registry") || l.contains("projection"))
    );
}

#[test]
fn ready_data_offers_verify_and_the_guidance_says_you_can_check_now() {
    let dir = tempfile::tempdir().unwrap();
    let mut page = page(dir.path());
    let path = dir.path().join("mame.xml");
    std::fs::write(&path, MAME).unwrap();
    let id = import(&mut page, &path);
    page.apply(DatSourcesPageAction::SetPlatform {
        id,
        platform: Some("Arcade".into()),
    });
    finish(&mut page);
    page.apply(DatSourcesPageAction::Save);
    finish(&mut page);
    let view = page.view();
    let mut state = DatSourcesPageUi::default();
    state.simple.select_platform("Arcade");
    let lines = render(&view, &mut state);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Verification data is ready. You can now check these games.")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("Verify Arcade Collection")));
}

// ---- the audit verifies the catalogue's game units, not the whole folder -----

mod audit_population {
    use super::*;
    use archivefs_core::dat::sources::audit_targets::AuditTargets;
    use archivefs_core::{Config, Database, scan_and_persist};

    const ROM: &[u8] = b"abcd";
    // sha1/crc32 of "abcd".
    const DAT: &str = r#"<?xml version="1.0"?><datafile><header><name>Nintendo - Game Boy Advance</name></header><game name="Game"><rom name="game.gba" size="4" crc="ed82cd11" sha1="81fe8bfe87576c3ecb22426f8e57847382917acf"/></game></datafile>"#;

    struct World {
        dir: tempfile::TempDir,
        folder: PathBuf,
        db: PathBuf,
    }

    /// A library whose `gba` folder holds one catalogued game beside artwork,
    /// manuals and an uncatalogued 3DS install, with a real (temporary) catalogue.
    fn world(extra_games: usize) -> World {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("games");
        let folder = source.join("gba");
        std::fs::create_dir_all(folder.join("assets")).unwrap();
        std::fs::write(folder.join("game.gba"), ROM).unwrap();
        for i in 0..extra_games {
            std::fs::write(folder.join(format!("extra{i}.gba")), format!("rom{i}")).unwrap();
        }
        for name in ["a.png", "a.jpg", "manual.pdf", "notes.txt"] {
            std::fs::write(folder.join("assets").join(name), vec![9u8; 4096]).unwrap();
        }
        std::fs::write(folder.join("Castlevania.cia"), vec![8u8; 4096]).unwrap();
        let config = Config {
            source_folders: vec![source],
            mount_root: dir.path().join("mounts"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        };
        let db = dir.path().join("library.sqlite3");
        let mut database = Database::open_or_create(&db).unwrap();
        scan_and_persist(&mut database, &config, "test").unwrap();
        World { dir, folder, db }
    }

    fn audit(world: &World) -> DatSourcesPageState {
        let mut page = page(world.dir.path()).with_database_path(Some(world.db.clone()));
        let dat = world.dir.path().join("gba.dat");
        std::fs::write(&dat, DAT).unwrap();
        let id = import(&mut page, &dat);
        page.apply(DatSourcesPageAction::SetPlatform {
            id: id.clone(),
            platform: Some("Game Boy Advance".into()),
        });
        page.apply(DatSourcesPageAction::Save);
        finish(&mut page);
        page.apply(DatSourcesPageAction::Audit {
            id,
            scan_root: world.folder.clone(),
        });
        finish(&mut page);
        page
    }

    #[test]
    fn a_platform_audit_reads_only_the_catalogued_games() {
        let w = world(2);
        let page = audit(&w);
        let view = page.view();
        assert!(view.audit_error.is_none(), "{:?}", view.audit_error);
        let result = view.audit.as_deref().expect("audit result");
        // 3 catalogued .gba games: not the png/jpg/pdf/txt nor the .cia beside them.
        assert_eq!(result.files_scanned, 3);
        assert!(
            result
                .population_note
                .contains("Checked 3 catalogued games"),
            "{}",
            result.population_note
        );
        assert!(
            result
                .population_note
                .contains("Artwork, manuals and other files beside the games were not read")
        );
        let exact = result
            .categories
            .iter()
            .find(|c| c.label == "Exact")
            .map_or(0, |c| c.count);
        assert_eq!(exact, 1, "the one ROM in the DAT still verifies exactly");
    }

    #[test]
    fn the_result_page_states_what_was_checked() {
        let w = world(0);
        let mut page = audit(&w);
        let view = page.view();
        let mut state = DatSourcesPageUi::default();
        state.simple.select_platform("Game Boy Advance");
        // Mark this result as the one the simple page shows for the platform.
        let lines = render(&view, &mut state);
        // Rendering never starts work; the note is part of the stored result.
        assert!(
            view.audit
                .as_deref()
                .unwrap()
                .population_note
                .contains("Checked 1 catalogued game.")
        );
        let _ = (&mut page, lines);
    }

    #[test]
    fn target_derivation_falls_back_to_the_folder_walk_only_where_it_must() {
        let w = world(0);
        // No platform, Arcade, or no catalogue: the games-only folder walk.
        assert_eq!(
            super::super::super::audit_targets_for(None, &w.folder, Some(&w.db)),
            AuditTargets::FolderWalkGamesOnly
        );
        assert_eq!(
            super::super::super::audit_targets_for(Some("Arcade"), &w.folder, Some(&w.db)),
            AuditTargets::FolderWalkGamesOnly
        );
        assert_eq!(
            super::super::super::audit_targets_for(Some("Game Boy Advance"), &w.folder, None),
            AuditTargets::FolderWalkGamesOnly
        );
        // A platform with a catalogue uses exactly its rows.
        let AuditTargets::Catalogue(targets) = super::super::super::audit_targets_for(
            Some("Game Boy Advance"),
            &w.folder,
            Some(&w.db),
        ) else {
            panic!("a catalogued platform audits its catalogue rows");
        };
        assert_eq!(targets.files, vec![w.folder.join("game.gba")]);
        assert_eq!(targets.catalogue_rows_unavailable, 0);
        // A different platform has no rows here: an empty population, not a walk.
        let AuditTargets::Catalogue(none) =
            super::super::super::audit_targets_for(Some("SNES"), &w.folder, Some(&w.db))
        else {
            panic!("an assigned platform never silently becomes a folder walk");
        };
        assert!(none.files.is_empty());
    }

    #[test]
    fn unassigned_rows_are_included_and_rows_of_another_platform_are_not() {
        let w = world(2);
        // One game loses its platform; another is assigned to a different system.
        let connection = rusqlite::Connection::open(&w.db).unwrap();
        connection
            .execute(
                "UPDATE platform_assignments SET is_current = 0 WHERE archive_id = \
                 (SELECT id FROM archives WHERE file_name_cached = CAST('extra0.gba' AS BLOB))",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE platform_assignments SET platform = 'SNES' WHERE is_current = 1 AND archive_id = \
                 (SELECT id FROM archives WHERE file_name_cached = CAST('extra1.gba' AS BLOB))",
                [],
            )
            .unwrap();
        drop(connection);
        let AuditTargets::Catalogue(targets) = super::super::super::audit_targets_for(
            Some("Game Boy Advance"),
            &w.folder,
            Some(&w.db),
        ) else {
            panic!("catalogue targets");
        };
        let names: Vec<_> = targets
            .files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["extra0.gba", "game.gba"], "{names:?}");
    }

    #[test]
    fn a_stale_catalogue_row_is_reported_unavailable_not_dropped() {
        let w = world(1);
        std::fs::remove_file(w.folder.join("extra0.gba")).unwrap();
        let page = audit(&w);
        let note = page
            .view()
            .audit
            .as_deref()
            .unwrap()
            .population_note
            .clone();
        assert!(note.contains("Checked 1 catalogued game."), "{note}");
        assert!(
            note.contains("1 catalogue entry was not on disk, so it was not checked"),
            "{note}"
        );
    }
}
