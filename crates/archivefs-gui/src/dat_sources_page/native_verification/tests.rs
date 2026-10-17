use super::*;
use archivefs_core::Database;
use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
use archivefs_core::safe_read::TrustedRoots;
use std::os::unix::ffi::OsStrExt;

struct Fixture {
    _dir: tempfile::TempDir,
    db: PathBuf,
    inventory: CatalogueInventorySnapshot,
    selection: Selection,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("roms");
        std::fs::create_dir(&root).unwrap();
        let dat = dir.path().join("gb.dat");
        std::fs::write(&dat,r#"<datafile><header><name>Nintendo - Game Boy</name><author>No-Intro</author><version>synthetic</version></header><game name="Synthetic"><rom name="game.gb" size="4" sha1="81fe8bfe87576c3ecb22426f8e57847382917acf"/></game></datafile>"#).unwrap();
        let rom = root.join("game.gb");
        std::fs::write(&rom, b"abcd").unwrap();
        let db = dir.path().join("library.sqlite3");
        Database::open_or_create(&db).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')",[root.as_os_str().as_bytes()]).unwrap();
        conn.execute("INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) VALUES(1,1,?1,?2,?1,'direct_game_image','Synthetic','synthetic','now','now','now','now')",rusqlite::params![b"game.gb".as_slice(),rom.as_os_str().as_bytes()]).unwrap();
        conn.execute("INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) VALUES(1,'Game Boy','manual',1,'now')",[]).unwrap();
        let mut registry = DatSourceRegistry::default();
        registry
            .add(DatSourceEntry::new(
                "test".into(),
                "Synthetic".into(),
                dat,
                DatSourceKind::File,
            ))
            .unwrap();
        let inventory = CatalogueInventorySnapshot {
            local_registry: registry,
            managed_sources: Default::default(),
            managed_root: dir.path().join("managed"),
        };
        let selection = Selection {
            platform: "Game Boy".into(),
            catalogue: CatalogueRef::local("test"),
            collection: None,
            database: Some(db.clone()),
            source_binding: "test".into(),
        };
        Self {
            _dir: dir,
            db,
            inventory,
            selection,
        }
    }
    fn preview(&self) -> Box<VerificationPreview> {
        Box::new(
            preview_library_verification(
                resolve_catalogue(&self.selection.catalogue, self.inventory.inputs()).unwrap(),
                self.db.clone(),
                "Game Boy".into(),
                None,
            )
            .unwrap(),
        )
    }
}
fn finish(c: &mut Controller) {
    let until = Instant::now() + std::time::Duration::from_secs(10);
    while c.busy() {
        c.poll();
        assert!(Instant::now() < until, "worker timed out");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
#[test]
fn terminal_completion_is_not_lost_or_reclassified_as_disconnect() {
    let (sender, receiver) = mpsc::channel();
    let mut c = Controller::default();
    c.receiver = Some(receiver);
    sender.send(Message::Progress("hashing".into())).unwrap();
    sender
        .send(Message::Finished(Ok(VerificationTotals {
            verified: 1,
            ..Default::default()
        })))
        .unwrap();
    drop(sender);
    c.poll();
    assert!(!c.busy());
    assert!(c.saved);
    assert_eq!(c.totals.as_ref().unwrap().verified, 1);
    assert!(c.error.is_none());
}
#[test]
fn changed_system_catalogue_collection_database_or_source_invalidates_confirmation() {
    let f = Fixture::new();
    for change in 0..5 {
        let mut c = Controller::default();
        c.select(f.selection.clone());
        c.preview = Some(f.preview());
        let mut changed = f.selection.clone();
        match change {
            0 => changed.platform = "Game Boy Color".into(),
            1 => changed.catalogue = CatalogueRef::local("another"),
            2 => changed.collection = Some(PathBuf::from("/other")),
            3 => changed.database = Some(PathBuf::from("/other.sqlite3")),
            _ => changed.source_binding = "changed source".into(),
        };
        c.select(changed);
        assert!(c.preview.is_none());
    }
}
fn frame(
    ctx: &egui::Context,
    c: &mut Controller,
    f: &Fixture,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                c.show(
                    ui,
                    Some(f.inventory.clone()),
                    Some(f.db.clone()),
                    TrustedRoots::none(),
                    true,
                );
            });
        },
    )
}
fn button(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == label => Some(t.pos + t.galley.size() / 2.0),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing button {label}"))
}
fn click(ctx: &egui::Context, c: &mut Controller, f: &Fixture, label: &str) {
    let output = frame(ctx, c, f, vec![]);
    let pos = button(&output, label);
    assert!(pos.y < 560.0, "primary action outside 1024×600: {pos:?}");
    frame(
        ctx,
        c,
        f,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
    );
    frame(
        ctx,
        c,
        f,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    );
}
#[test]
fn gui_preview_then_confirm_is_required_before_database_writes() {
    let f = Fixture::new();
    let mut c = Controller::default();
    c.select(f.selection.clone());
    let ctx = egui::Context::default();
    frame(&ctx, &mut c, &f, vec![]);
    click(&ctx, &mut c, &f, "Preview verification");
    finish(&mut c);
    assert!(c.preview.is_some(), "{:?}", c.error);
    assert!(
        Database::open_read_only(&f.db)
            .unwrap()
            .library_dat_identities_for_item(1)
            .unwrap()
            .is_empty()
    );
    click(&ctx, &mut c, &f, "Confirm verification");
    finish(&mut c);
    assert_eq!(c.totals.as_ref().unwrap().verified, 1);
    let evidence = Database::open_read_only(&f.db)
        .unwrap()
        .library_dat_identities_for_item(1)
        .unwrap();
    assert_eq!(evidence.len(), 1);
    assert!(evidence[0].source.catalogue_sha256.is_some());
}
#[test]
fn confirm_button_accepts_keyboard_at_1024_by_600() {
    let f = Fixture::new();
    let mut c = Controller::default();
    c.select(f.selection.clone());
    c.preview = Some(f.preview());
    let ctx = egui::Context::default();
    frame(&ctx, &mut c, &f, vec![]);
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    };
    frame(&ctx, &mut c, &f, vec![key(egui::Key::Tab)]);
    frame(&ctx, &mut c, &f, vec![key(egui::Key::Enter)]);
    assert!(c.busy(), "Enter must confirm focused native button");
    finish(&mut c);
    assert_eq!(c.totals.as_ref().unwrap().verified, 1);
}

#[test]
fn full_blue_platform_page_keeps_confirm_visible_at_1024_by_600() {
    let f = Fixture::new();
    let root = f._dir.path();
    let mut page = DatSourcesPageState::load_with_transaction_dir_and_managed_paths(
        root.join("sources.toml"),
        vec![root.join("roms")],
        TrustedRoots::none(),
        root.join("journals"),
        root.join("managed.toml"),
        root.join("managed"),
    );
    page.draft = f.inventory.local_registry.clone();
    page.start_validate("test".into());
    let until = Instant::now() + std::time::Duration::from_secs(10);
    while page.is_busy() {
        page.poll();
        assert!(Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    page.poll();
    let mut other = page.draft.get("test").unwrap().clone();
    other.id = "other".into();
    let other_path = root.join("other.dat");
    std::fs::copy(&other.path, &other_path).unwrap();
    other.path = other_path;
    other.display_name = "Other installed catalogue".into();
    page.draft.add(other).unwrap();
    page.saved = page.draft.clone();
    let inventory = CatalogueInventorySnapshot {
        local_registry: page.draft.clone(),
        ..f.inventory.clone()
    };
    let mut state = DatSourcesPageUi::default();
    state.simple.open_verification(
        CatalogueRef::local("test"),
        Some("Game Boy".into()),
        root.join("roms"),
    );
    state.catalogue_picker.ensure_loaded_with(inventory.clone());
    while state.catalogue_picker.loading {
        state.catalogue_picker.poll();
        assert!(Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let selection = Selection {
        collection: Some(root.join("roms")),
        source_binding: format!("{:?}", inventory.local_registry.get("test")),
        ..f.selection.clone()
    };
    state.simple.verification.select(selection.clone());
    state.simple.verification.preview = Some(Box::new(
        preview_library_verification(
            resolve_catalogue(&selection.catalogue, inventory.inputs()).unwrap(),
            f.db.clone(),
            "Game Boy".into(),
            selection.collection.clone(),
        )
        .unwrap(),
    ));
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                super::super::simple::show_with_inputs(
                    ui,
                    &page.view(),
                    &mut state,
                    Some(f.db.clone()),
                    TrustedRoots::none(),
                );
            });
        },
    );
    let pos = button(&output, "Confirm verification");
    assert!(pos.y < 560.0, "full page primary below viewport: {pos:?}");
    assert!(
        Database::open_read_only(&f.db)
            .unwrap()
            .library_dat_identities_for_item(1)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn source_cards_preserve_arcade_member_audits_and_discovery_but_bound_rom_verification() {
    use archivefs_core::dat::catalogue_selection::LocalCatalogueMemberRef;
    let dir = tempfile::tempdir().unwrap();
    let catalogue_root = dir.path().join("catalogues");
    std::fs::create_dir(&catalogue_root).unwrap();
    let mut registry = DatSourceRegistry::default();
    let mut entry = DatSourceEntry::new(
        "arcade".into(),
        "Arcade".into(),
        catalogue_root,
        DatSourceKind::Folder,
    );
    entry.platform = Some("Arcade".into());
    registry.add(entry).unwrap();
    let action = DatSourcesPageAction::VerifyCatalogue {
        reference: CatalogueRef::Local {
            source_id: "arcade".into(),
            member: Some(LocalCatalogueMemberRef {
                relative_path: PathBuf::from("member.dat"),
            }),
        },
        scan_root: PathBuf::from("/disposable/roms"),
    };
    assert!(source_card_selection(&action, &registry, &[], false).is_none());
    registry.get_mut("arcade").unwrap().platform = Some("Game Boy".into());
    let (reference, platform, collection) =
        source_card_selection(&action, &registry, &[], false).unwrap();
    assert_eq!(platform.as_deref(), Some("Game Boy"));
    assert_eq!(collection, PathBuf::from("/disposable/roms"));
    assert!(matches!(
        reference,
        CatalogueRef::Local {
            member: Some(_),
            ..
        }
    ));
    assert!(source_card_selection(&action, &registry, &[], true).is_none());
}
