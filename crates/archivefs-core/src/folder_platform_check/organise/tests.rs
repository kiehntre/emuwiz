//! Disposable-fixture journeys: identify -> plan -> approve -> move -> undo.
use super::*;
use crate::dat::rom_organisation::{OrganisationStatus, rollback_organisation_transaction};
use rusqlite::Connection;

fn finding(path: &Path, tier: Tier, detected: &str) -> Finding {
    Finding {
        archive_id: 0,
        title: "Game".into(),
        path: path.to_path_buf(),
        folder: path.parent().unwrap().to_path_buf(),
        assigned: "SNES".into(),
        assigned_source: Some("folder_alias".into()),
        tier,
        detected: detected.into(),
        lines: vec!["evidence".into()],
        has_unh: false,
    }
}

struct Library {
    _temp: tempfile::TempDir,
    root: PathBuf,
    db: crate::Database,
    journal: PathBuf,
}

fn library(files: &[&str]) -> Library {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    for f in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, format!("bytes of {f}")).unwrap();
    }
    let mut db = crate::Database::open_or_create(temp.path().join("lib.sqlite3")).unwrap();
    db.register_source_folders(std::slice::from_ref(&root))
        .unwrap();
    let folder = db.list_source_folders().unwrap()[0].id;
    for f in files {
        let a = crate::Archive::from_path_in_root(root.join(f), &root).unwrap();
        db.upsert_archive(folder, &root, &a).unwrap();
    }
    let journal = temp.path().join("journal");
    Library {
        _temp: temp,
        root,
        db,
        journal,
    }
}

fn platform_of(db: &crate::Database, path: &Path) -> Option<String> {
    let id = db.find_archive_id_by_absolute_path(path).unwrap()?;
    Connection::open(db.path())
        .unwrap()
        .query_row(
            "SELECT platform FROM platform_assignments WHERE archive_id=?1 AND is_current=1",
            [id],
            |r| r.get(0),
        )
        .ok()
}

#[test]
fn mixed_library_journey_moves_confirmed_games_and_leaves_the_rest() {
    let lib = library(&[
        "snes/Dat Game.zip",
        "snes/Homebrew.zip",
        "snes/Maybe.zip",
        "snes/Mixed.zip",
        "mixed/Mystery.zip",
    ]);
    let s = |f: &str| lib.root.join(f);
    let findings = [
        finding(&s("snes/Dat Game.zip"), Tier::Confirmed, "NES"),
        finding(&s("snes/Homebrew.zip"), Tier::PlatformConfirmed, "NES"),
        finding(&s("snes/Maybe.zip"), Tier::Probable, "NES"),
        finding(&s("snes/Mixed.zip"), Tier::Ambiguous, "NES"),
    ];
    let refs: Vec<&Finding> = findings.iter().collect();
    let unknown = [UnknownGame {
        archive_id: 0,
        path: s("mixed/Mystery.zip"),
    }];
    let mut plans = plan_organisation(&refs, &unknown, std::slice::from_ref(&lib.root), 1);
    assert_eq!(plans.len(), 1);
    let summary = summarise(&plans);
    assert_eq!(
        (summary.to_move, summary.to_unknown, summary.left_alone),
        (2, 1, 2)
    );
    // Nothing moved by planning.
    assert!(s("snes/Dat Game.zip").exists());
    assert!(!lib.root.join("Nintendo Entertainment System").exists());

    prepare(&mut plans[0], &lib.db).unwrap();
    let approved: BTreeSet<String> = plans[0]
        .plan
        .suggested()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect();
    let outcome = apply(
        &plans[0],
        &approved,
        &lib.db,
        &lib.journal,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 3);

    let folder = crate::platform::canonical_layout_folder("NES").unwrap();
    let moved = lib.root.join(&folder).join("Dat Game.zip");
    assert_eq!(
        std::fs::read(&moved).unwrap(),
        b"bytes of snes/Dat Game.zip"
    );
    assert!(lib.root.join(&folder).join("Homebrew.zip").exists());
    assert!(lib.root.join("Unnamed/Unknown/Mystery.zip").exists());
    // Probable and Ambiguous stay exactly where they were.
    assert!(s("snes/Maybe.zip").exists() && s("snes/Mixed.zip").exists());
    assert_eq!(platform_of(&lib.db, &moved).as_deref(), Some("NES"));
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&s("snes/Dat Game.zip"))
            .unwrap()
            .is_none()
    );

    let mut tx = outcome.transaction;
    rollback_organisation_transaction(&mut tx, &lib.journal, &AtomicBool::new(false), &lib.root)
        .unwrap();
    assert!(
        s("snes/Dat Game.zip").exists()
            && s("snes/Homebrew.zip").exists()
            && s("mixed/Mystery.zip").exists()
    );
    assert!(!moved.exists());
    assert_ne!(
        platform_of(&lib.db, &s("snes/Dat Game.zip")).as_deref(),
        Some("NES")
    );
}

#[test]
fn an_existing_destination_is_a_collision_and_is_never_overwritten() {
    let lib = library(&["snes/Game.zip", "nes/Game.zip"]);
    let f = finding(&lib.root.join("snes/Game.zip"), Tier::Confirmed, "NES");
    let plans = plan_organisation(&[&f], &[], std::slice::from_ref(&lib.root), 1);
    let entry = &plans[0].plan.entries[0];
    assert_eq!(entry.status, OrganisationStatus::Conflict);
    assert_eq!(summarise(&plans).collisions, 1);
    assert_eq!(
        std::fs::read(lib.root.join("nes/Game.zip")).unwrap(),
        b"bytes of nes/Game.zip"
    );
}

#[test]
fn games_outside_registered_roots_are_left_alone_with_a_reason() {
    let lib = library(&["snes/Game.zip"]);
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("Game.zip");
    std::fs::write(&path, b"x").unwrap();
    let inside = finding(&lib.root.join("snes/Game.zip"), Tier::Confirmed, "NES");
    let stray = finding(&path, Tier::Confirmed, "NES");
    let plans = plan_organisation(&[&inside, &stray], &[], std::slice::from_ref(&lib.root), 1);
    assert!(
        plans[0]
            .left_alone
            .iter()
            .any(|(p, why)| p == &path && why == OUTSIDE_SOURCES_REASON)
    );
}

#[test]
fn a_picked_game_outside_every_source_is_reported_even_with_no_plan() {
    let lib = library(&["snes/Game.zip"]);
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("Picked.zip");
    std::fs::write(&path, b"x").unwrap();
    let chosen = [ChosenGame {
        archive_id: 0,
        path: path.clone(),
        platform: "nes".into(),
    }];
    let (plans, excluded) =
        plan_organisation_with_exclusions(&[], &chosen, &[], std::slice::from_ref(&lib.root), 1);
    assert!(plans.is_empty(), "nothing inside a source was picked");
    assert_eq!(
        excluded,
        vec![(path.clone(), OUTSIDE_SOURCES_REASON.to_string())]
    );
    assert!(OUTSIDE_SOURCES_REASON.contains("Sources & Providers"));
    // The file is untouched: planning never moves anything.
    assert_eq!(std::fs::read(&path).unwrap(), b"x");
}

#[test]
fn an_unbounded_picked_file_outside_every_source_is_still_reported() {
    let lib = library(&["snes/Game.zip"]);
    let outside = tempfile::tempdir().unwrap();
    // A CUE whose track is missing cannot be bounded as a set.
    let cue = outside.path().join("Disc.cue");
    std::fs::write(
        &cue,
        "FILE \"Disc (Track 1).bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    let chosen = [ChosenGame {
        archive_id: 0,
        path: cue.clone(),
        platform: "psx".into(),
    }];
    let (plans, excluded) =
        plan_organisation_with_exclusions(&[], &chosen, &[], std::slice::from_ref(&lib.root), 1);
    assert!(plans.is_empty());
    assert_eq!(excluded.len(), 1);
    assert_eq!(excluded[0].0, cue);
}

fn approve_all(plan: &RootPlan) -> BTreeSet<String> {
    plan.plan
        .suggested()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_picked_cue_bin_set_moves_complete_and_undo_restores_it() {
    // The scan never catalogues a .cue outside a PlayStation folder, so the
    // person picks it from disk (archive id 0).
    let lib = library(&["mixed/Other.zip"]);
    let s = |f: &str| lib.root.join(f);
    std::fs::write(
        s("mixed/Disc Game.cue"),
        "FILE \"Disc Game (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\nFILE \"Disc Game (Track 2).bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(s("mixed/Disc Game (Track 1).bin"), b"data track").unwrap();
    std::fs::write(s("mixed/Disc Game (Track 2).bin"), b"audio track").unwrap();
    let chosen = [ChosenGame {
        archive_id: 0,
        path: s("mixed/Disc Game.cue"),
        platform: "PS2".into(),
    }];
    let mut plans =
        plan_organisation_with_choices(&[], &chosen, &[], std::slice::from_ref(&lib.root), 1);
    assert_eq!(
        plans[0].sets.get(&s("mixed/Disc Game.cue")).map(Vec::len),
        Some(2)
    );
    prepare(&mut plans[0], &lib.db).unwrap();
    let outcome = apply(
        &plans[0],
        &approve_all(&plans[0]),
        &lib.db,
        &lib.journal,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 3);
    let dest = lib
        .root
        .join(crate::platform::canonical_layout_folder("PS2").unwrap());
    assert_eq!(
        std::fs::read(dest.join("Disc Game (Track 2).bin")).unwrap(),
        b"audio track"
    );
    assert!(dest.join("Disc Game.cue").exists() && dest.join("Disc Game (Track 1).bin").exists());
    assert!(!s("mixed/Disc Game (Track 1).bin").exists());
    assert!(s("mixed/Other.zip").exists());
    assert!(crate::dat::rom_organisation::is_organisation_transaction(
        &outcome.transaction
    ));

    let mut tx = outcome.transaction;
    rollback_organisation_transaction(&mut tx, &lib.journal, &AtomicBool::new(false), &lib.root)
        .unwrap();
    for f in [
        "Disc Game.cue",
        "Disc Game (Track 1).bin",
        "Disc Game (Track 2).bin",
    ] {
        assert!(s(&format!("mixed/{f}")).exists(), "{f} restored");
        assert!(!dest.join(f).exists());
    }
    assert!(!dest.exists(), "the created system folder is removed again");
}

/// Points an existing library row at `to` (a fixture shortcut for rows the
/// scanner would only create from real media).
fn catalogue_as(lib: &Library, from: &Path, to: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let meta = std::fs::symlink_metadata(to).unwrap();
    let mtime = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let relative = to.strip_prefix(&lib.root).unwrap();
    Connection::open(lib.db.path())
        .unwrap()
        .execute(
            "UPDATE archives SET relative_path=?1, absolute_path_cached=?2, file_name_cached=?3, size_bytes=?4, modified_time_unix_seconds=?5 WHERE absolute_path_cached=?6",
            rusqlite::params![
                relative.as_os_str().as_bytes(),
                to.as_os_str().as_bytes(),
                to.file_name().unwrap().as_bytes(),
                meta.len() as i64,
                mtime,
                from.as_os_str().as_bytes()
            ],
        )
        .unwrap();
}

#[test]
fn a_catalogued_cue_row_moves_with_its_tracks_and_keeps_its_library_record() {
    let lib = library(&["psx/Placeholder.zip"]);
    let s = |f: &str| lib.root.join(f);
    std::fs::write(s("psx/PS2 Game.cue"), "FILE \"PS2 Game.bin\" BINARY\n").unwrap();
    std::fs::write(s("psx/PS2 Game.bin"), b"ps2 data").unwrap();
    catalogue_as(&lib, &s("psx/Placeholder.zip"), &s("psx/PS2 Game.cue"));
    let id = lib
        .db
        .find_archive_id_by_absolute_path(&s("psx/PS2 Game.cue"))
        .unwrap()
        .unwrap();
    let chosen = [ChosenGame {
        archive_id: id,
        path: s("psx/PS2 Game.cue"),
        platform: "PS2".into(),
    }];
    let mut plans =
        plan_organisation_with_choices(&[], &chosen, &[], std::slice::from_ref(&lib.root), 1);
    prepare(&mut plans[0], &lib.db).unwrap();
    let outcome = apply(
        &plans[0],
        &approve_all(&plans[0]),
        &lib.db,
        &lib.journal,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 2);
    let dest = lib
        .root
        .join(crate::platform::canonical_layout_folder("PS2").unwrap());
    assert_eq!(
        lib.db
            .find_archive_id_by_absolute_path(&dest.join("PS2 Game.cue"))
            .unwrap(),
        Some(id)
    );
    assert_eq!(
        std::fs::read(dest.join("PS2 Game.bin")).unwrap(),
        b"ps2 data"
    );
    let mut tx = outcome.transaction;
    rollback_organisation_transaction(&mut tx, &lib.journal, &AtomicBool::new(false), &lib.root)
        .unwrap();
    assert_eq!(
        lib.db
            .find_archive_id_by_absolute_path(&s("psx/PS2 Game.cue"))
            .unwrap(),
        Some(id)
    );
    assert!(s("psx/PS2 Game.bin").exists());
}

#[test]
fn a_set_with_a_taken_track_name_or_a_missing_track_never_moves() {
    let lib = library(&["mixed/x.zip"]);
    let s = |f: &str| lib.root.join(f);
    std::fs::write(s("mixed/A.cue"), "FILE \"A.bin\" BINARY\n").unwrap();
    std::fs::write(s("mixed/A.bin"), b"a").unwrap();
    std::fs::write(s("mixed/B.cue"), "FILE \"Missing.bin\" BINARY\n").unwrap();
    let dest = lib
        .root
        .join(crate::platform::canonical_layout_folder("PSX").unwrap());
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("A.bin"), b"someone else's").unwrap();
    let chosen: Vec<ChosenGame> = ["mixed/A.cue", "mixed/B.cue"]
        .iter()
        .map(|f| ChosenGame {
            archive_id: 0,
            path: s(f),
            platform: "PSX".into(),
        })
        .collect();
    let plans =
        plan_organisation_with_choices(&[], &chosen, &[], std::slice::from_ref(&lib.root), 1);
    assert_eq!(plans[0].plan.suggested().count(), 0);
    let a = plans[0]
        .plan
        .entries
        .iter()
        .find(|e| e.source_path == s("mixed/A.cue"))
        .unwrap();
    assert_eq!(a.status, OrganisationStatus::Conflict);
    assert!(
        plans[0]
            .left_alone
            .iter()
            .any(|(p, why)| p == &s("mixed/B.cue") && why.contains("missing"))
    );
    assert_eq!(
        std::fs::read(dest.join("A.bin")).unwrap(),
        b"someone else's"
    );
}

fn ps3_folder(at: &Path, title_id: &str) {
    std::fs::create_dir_all(at.join("PS3_GAME/USRDIR")).unwrap();
    std::fs::write(
        at.join("PS3_GAME/PARAM.SFO"),
        crate::game_package::tests::sfo_bytes(&[("CATEGORY", "DG"), ("TITLE_ID", title_id)]),
    )
    .unwrap();
    std::fs::write(at.join("PS3_GAME/USRDIR/EBOOT.BIN"), b"SCE\0eboot").unwrap();
    std::fs::write(at.join("PS3_DISC.SFB"), b"disc").unwrap();
}

#[test]
fn a_ps3_folder_is_found_moved_whole_verified_and_undone() {
    let lib = library(&["mixed/Some.zip"]);
    let game = lib.root.join("mixed/Great Game [BLES00001]");
    ps3_folder(&game, "BLES00001");
    let found = found_game_folders(std::slice::from_ref(&lib.root));
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].path.as_path(), found[0].platform.as_str()),
        (game.as_path(), "PS3")
    );

    let previews = preview_folders(&lib.db, std::slice::from_ref(&lib.root), &found);
    let folder_move::FolderPreview::Move(plan) = &previews[0].1 else {
        panic!("expected a move, got {:?}", previews[0].1);
    };
    assert!(plan.creates_folder);
    assert!(game.exists(), "preview never moves");
    let inode_before = std::fs::metadata(game.join("PS3_GAME/USRDIR/EBOOT.BIN")).unwrap();

    let mut record = folder_move::apply(plan, lib.db.path(), &lib.journal).unwrap();
    let dest = lib
        .root
        .join(crate::platform::canonical_layout_folder("PS3").unwrap())
        .join("Great Game [BLES00001]");
    assert_eq!(record.destination, dest);
    assert!(!game.exists());
    let after = std::fs::metadata(dest.join("PS3_GAME/USRDIR/EBOOT.BIN")).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(after.ino(), inode_before.ino());
    assert_eq!(std::fs::read(dest.join("PS3_DISC.SFB")).unwrap(), b"disc");
    assert!(lib.root.join("mixed/Some.zip").exists());
    assert_eq!(
        folder_move::records(&lib.journal)[0].state,
        folder_move::FolderMoveState::Committed
    );

    // Moving it again is already in place.
    let again = folder_move::preview(&lib.db, std::slice::from_ref(&lib.root), &dest, "PS3", "");
    assert!(matches!(again, folder_move::FolderPreview::AlreadyInPlace));

    folder_move::undo(&mut record, &lib.journal).unwrap();
    assert!(game.join("PS3_GAME/PARAM.SFO").exists());
    assert!(!dest.exists());
    assert!(
        !dest.parent().unwrap().exists(),
        "the created PS3 folder is removed"
    );
    assert_eq!(record.state, folder_move::FolderMoveState::Undone);
}

#[test]
fn a_chosen_folder_row_moves_with_its_library_path_and_undo_restores_it() {
    let lib = library(&["mixed/Adventure.zip"]);
    let path = lib.root.join("mixed/Adventure");
    std::fs::create_dir_all(path.join("data")).unwrap();
    std::fs::write(path.join("data/RESOURCE.001"), b"res").unwrap();
    catalogue_as(&lib, &lib.root.join("mixed/Adventure.zip"), &path);
    let candidate = FolderCandidate {
        path: path.clone(),
        platform: "ScummVM".into(),
        evidence: "your system choice".into(),
        chosen: true,
    };
    let previews = preview_folders(&lib.db, std::slice::from_ref(&lib.root), &[candidate]);
    let folder_move::FolderPreview::Move(plan) = &previews[0].1 else {
        panic!("expected a move, got {:?}", previews[0].1);
    };
    assert_eq!(plan.rows.len(), 1);
    let mut record = folder_move::apply(plan, lib.db.path(), &lib.journal).unwrap();
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&record.destination)
            .unwrap()
            .is_some()
    );
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&path)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        std::fs::read(record.destination.join("data/RESOURCE.001")).unwrap(),
        b"res"
    );
    folder_move::undo(&mut record, &lib.journal).unwrap();
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&path)
            .unwrap()
            .is_some()
    );
    assert!(path.join("data/RESOURCE.001").exists());
}

#[test]
fn shared_system_and_colliding_folders_are_refused_without_change() {
    let lib = library(&["mixed/Pack/One.zip", "mixed/Pack/Two.zip"]);
    let roots = std::slice::from_ref(&lib.root);
    let refused = |path: &Path, platform: &str| match folder_move::preview(
        &lib.db, roots, path, platform, "",
    ) {
        folder_move::FolderPreview::Refused(why) => why,
        other => panic!("expected a refusal for {}, got {other:?}", path.display()),
    };
    // A folder holding other library games is shared.
    assert!(refused(&lib.root.join("mixed/Pack"), "PS3").contains("other game"));
    // A system folder is never one game.
    std::fs::create_dir_all(lib.root.join("snes")).unwrap();
    assert!(refused(&lib.root.join("snes"), "PS3").contains("system folder"));
    // The library root itself.
    assert!(refused(&lib.root, "PS3").contains("library folder"));
    // An existing destination is never overwritten.
    let game = lib.root.join("mixed/Game");
    ps3_folder(&game, "BLUS00002");
    let taken = lib
        .root
        .join(crate::platform::canonical_layout_folder("PS3").unwrap())
        .join("Game");
    std::fs::create_dir_all(&taken).unwrap();
    assert!(refused(&game, "PS3").contains("already exists"));
    // Links inside a folder are refused.
    std::fs::remove_dir(&taken).unwrap();
    std::os::unix::fs::symlink("/etc", game.join("link")).unwrap();
    assert!(refused(&game, "PS3").contains("link"));
    assert!(game.join("PS3_GAME/PARAM.SFO").exists());
    assert!(lib.root.join("mixed/Pack/One.zip").exists());
}

#[test]
fn a_folder_changed_after_preview_is_not_moved() {
    let lib = library(&["mixed/Some.zip"]);
    let game = lib.root.join("mixed/Game");
    ps3_folder(&game, "BLES00003");
    let previews = preview_folders(
        &lib.db,
        std::slice::from_ref(&lib.root),
        &found_game_folders(std::slice::from_ref(&lib.root)),
    );
    let folder_move::FolderPreview::Move(plan) = &previews[0].1 else {
        panic!("expected a move");
    };
    std::fs::write(game.join("NEW.TXT"), b"added").unwrap();
    let error = folder_move::apply(plan, lib.db.path(), &lib.journal).unwrap_err();
    assert!(error.contains("changed since the preview"));
    assert!(game.join("NEW.TXT").exists());
}

#[test]
fn an_interrupted_folder_move_is_settled_from_disk_and_library() {
    let lib = library(&["mixed/Some.zip"]);
    let game = lib.root.join("mixed/Game");
    ps3_folder(&game, "BLES00004");
    let previews = preview_folders(
        &lib.db,
        std::slice::from_ref(&lib.root),
        &found_game_folders(std::slice::from_ref(&lib.root)),
    );
    let folder_move::FolderPreview::Move(plan) = &previews[0].1 else {
        panic!("expected a move");
    };
    let mut record = folder_move::apply(plan, lib.db.path(), &lib.journal).unwrap();
    // Simulate a crash after the rename and before the journal was finished.
    record.state = folder_move::FolderMoveState::Planned;
    assert_eq!(
        folder_move::recover(&mut record, &lib.journal).unwrap(),
        folder_move::FolderMoveState::Committed
    );
    folder_move::undo(&mut record, &lib.journal).unwrap();
    assert!(game.exists());
}

/// Review 2026-11-14: recovery settles an interrupted folder move from what
/// the library shows and then records the outcome. What it saw may be a
/// commit that is visible but not yet durable (a process that died between
/// SQLite's commit point and its folder fsync), so the library is made
/// durable before the outcome is recorded, in both directions.
#[test]
fn recovering_a_folder_move_with_library_rows_makes_the_library_durable_first() {
    use crate::database::organisation_paths::PERSISTED;
    // A folder that is itself one library row (as a ScummVM game is).
    let lib = library(&["mixed/Adventure.zip"]);
    let game = lib.root.join("mixed/Adventure");
    std::fs::create_dir_all(game.join("data")).unwrap();
    std::fs::write(game.join("data/RESOURCE.001"), b"res").unwrap();
    catalogue_as(&lib, &lib.root.join("mixed/Adventure.zip"), &game);
    let candidate = FolderCandidate {
        path: game.clone(),
        platform: "ScummVM".into(),
        evidence: "your system choice".into(),
        chosen: true,
    };
    let previews = preview_folders(&lib.db, std::slice::from_ref(&lib.root), &[candidate]);
    let folder_move::FolderPreview::Move(plan) = &previews[0].1 else {
        panic!("expected a move, got {:?}", previews[0].1);
    };
    assert_eq!(plan.rows.len(), 1, "the folder has a library row");
    let mut record = folder_move::apply(plan, lib.db.path(), &lib.journal).unwrap();
    let persisted = || PERSISTED.with(|p| std::mem::take(&mut *p.borrow_mut()));
    let on_disk = |record: &folder_move::FolderMoveRecord| {
        folder_move::records(&lib.journal)
            .into_iter()
            .find(|r| r.id == record.id)
            .map(|r| r.state)
    };

    // Interrupted after the library commit: settled as Committed, and only
    // once the library is durable.
    record.state = folder_move::FolderMoveState::Planned;
    persisted();
    assert_eq!(
        folder_move::recover(&mut record, &lib.journal).unwrap(),
        folder_move::FolderMoveState::Committed
    );
    assert!(
        persisted().contains(&record.database),
        "the library was made durable before Committed was recorded"
    );
    assert_eq!(
        on_disk(&record),
        Some(folder_move::FolderMoveState::Committed)
    );

    // Undone, then interrupted as if the move had never committed: settled
    // as RolledBack, again only after the library is durable.
    folder_move::undo(&mut record, &lib.journal).unwrap();
    assert!(game.exists());
    record.state = folder_move::FolderMoveState::Planned;
    persisted();
    assert_eq!(
        folder_move::recover(&mut record, &lib.journal).unwrap(),
        folder_move::FolderMoveState::RolledBack
    );
    assert!(persisted().contains(&record.database));
    assert!(game.join("data/RESOURCE.001").exists());
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&game)
            .unwrap()
            .is_some(),
        "the library names the folder where it is"
    );
}

// ---- folder-move durability and interrupted recovery (review 2026-11-14) ----
//
// Every affected transition of apply, Undo and recovery is interrupted (a
// thread panic stands in for process death: nothing else runs, and an open
// library transaction rolls back), every directory sync is failed in turn,
// and recovery is repeated. After each, the journal may only claim what the
// files and the library both show.

use crate::game_package::capture_folder_layout;
use folder_move::{FolderMoveRecord, FolderMoveState as St, faults};

/// A game folder ready to move: one that is itself a library row (ScummVM)
/// or one without a row (a PS3 disc folder). Both create their system folder.
struct FolderFx {
    lib: Library,
    game: PathBuf,
    plan: folder_move::FolderMovePlan,
    has_row: bool,
}

impl FolderFx {
    fn new(has_row: bool) -> Self {
        let lib = library(&["mixed/Adventure.zip"]);
        let (game, previews) = if has_row {
            let game = lib.root.join("mixed/Adventure");
            std::fs::create_dir_all(game.join("data")).unwrap();
            std::fs::write(game.join("data/RESOURCE.001"), b"res").unwrap();
            catalogue_as(&lib, &lib.root.join("mixed/Adventure.zip"), &game);
            let candidate = FolderCandidate {
                path: game.clone(),
                platform: "ScummVM".into(),
                evidence: "your system choice".into(),
                chosen: true,
            };
            let p = preview_folders(&lib.db, std::slice::from_ref(&lib.root), &[candidate]);
            (game, p)
        } else {
            let game = lib.root.join("mixed/Game");
            ps3_folder(&game, "BLES00009");
            let p = preview_folders(
                &lib.db,
                std::slice::from_ref(&lib.root),
                &found_game_folders(std::slice::from_ref(&lib.root)),
            );
            (game, p)
        };
        let folder_move::FolderPreview::Move(plan) = previews[0].1.clone() else {
            panic!("expected a move, got {:?}", previews[0].1);
        };
        assert!(plan.creates_folder, "the fixture creates its system folder");
        assert_eq!(plan.rows.is_empty(), !has_row);
        faults::reset();
        Self {
            lib,
            game,
            plan: *plan,
            has_row,
        }
    }

    fn apply(&self) -> Result<FolderMoveRecord, String> {
        folder_move::apply(&self.plan, self.lib.db.path(), &self.lib.journal)
    }

    fn on_disk(&self) -> FolderMoveRecord {
        let all = folder_move::records(&self.lib.journal);
        assert_eq!(all.len(), 1, "one record");
        all.into_iter().next().unwrap()
    }

    /// The journal's state agrees with the files and the library.
    fn agrees(&self, state: St, context: &str) {
        let at_source = self.game.exists();
        let at_destination = self.plan.destination.exists();
        let row = |p: &Path| {
            self.lib
                .db
                .find_archive_id_by_absolute_path(p)
                .unwrap()
                .is_some()
        };
        match state {
            St::Committed => {
                assert!(!at_source && at_destination, "{context}: files");
                if self.has_row {
                    assert!(
                        row(&self.plan.destination) && !row(&self.game),
                        "{context}: library"
                    );
                }
            }
            St::RolledBack | St::Undone => {
                assert!(at_source && !at_destination, "{context}: files");
                if self.has_row {
                    assert!(
                        row(&self.game) && !row(&self.plan.destination),
                        "{context}: library"
                    );
                }
                for dir in &self.on_disk().created_dirs {
                    assert!(
                        !dir.exists(),
                        "{context}: created {} removed",
                        dir.display()
                    );
                }
            }
            St::Planned | St::Undoing => {}
            St::NeedsReview => panic!("{context}: needs review"),
        }
        // Whatever happened, the game's files are intact in one place.
        let here = if at_source {
            &self.game
        } else {
            &self.plan.destination
        };
        assert_eq!(
            capture_folder_layout(here).unwrap().entries.len(),
            self.plan.layout.entries.len(),
            "{context}: contents"
        );
    }

    /// Recovers the record on disk twice; both reach the same agreed outcome.
    fn recover_twice(&self, context: &str) -> St {
        let mut record = self.on_disk();
        assert!(
            folder_move::interrupted(&self.lib.journal)
                .iter()
                .any(|r| r.id == record.id)
                || !matches!(record.state, St::Planned | St::Undoing),
            "{context}: an open record is listed as interrupted"
        );
        let first = folder_move::recover(&mut record, &self.lib.journal).unwrap();
        assert!(
            matches!(first, St::Committed | St::RolledBack | St::Undone),
            "{context}: {first:?}"
        );
        assert_eq!(self.on_disk().state, first, "{context}: recorded");
        self.agrees(first, context);
        let mut again = self.on_disk();
        assert_eq!(
            folder_move::recover(&mut again, &self.lib.journal).unwrap(),
            first,
            "{context}: repeated recovery"
        );
        self.agrees(first, context);
        assert!(folder_move::interrupted(&self.lib.journal).is_empty());
        first
    }
}

fn crashed<T>(f: impl FnOnce() -> T) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err()
}

const APPLY_POINTS: [&str; 4] = [
    "apply:planned",
    "apply:created-dir",
    "apply:renamed",
    "apply:rename-durable",
];

#[test]
fn a_folder_move_interrupted_at_any_step_recovers_to_one_agreed_outcome() {
    for has_row in [true, false] {
        for point in APPLY_POINTS.iter().chain(&["apply:committed"]) {
            let fx = FolderFx::new(has_row);
            faults::crash_at(point);
            assert!(crashed(|| fx.apply()), "{point} reached");
            let context = format!("{point} (row: {has_row})");
            assert_eq!(fx.on_disk().state, St::Planned, "{context}");
            let outcome = fx.recover_twice(&context);
            let expected = if *point == "apply:committed" || (!has_row && point.contains("rename"))
            {
                St::Committed
            } else {
                St::RolledBack
            };
            assert_eq!(outcome, expected, "{context}");
            if outcome == St::Committed {
                let mut record = fx.on_disk();
                folder_move::undo(&mut record, &fx.lib.journal).unwrap();
                fx.agrees(St::Undone, &context);
            }
        }
    }
}

#[test]
fn a_folder_undo_interrupted_at_any_step_is_finished_or_reversed_by_recovery() {
    let points = [
        "undo:undoing",
        "undo:renamed",
        "undo:rename-durable",
        "undo:committed",
        "undo:removed-created",
    ];
    for has_row in [true, false] {
        for point in points {
            let fx = FolderFx::new(has_row);
            let mut record = fx.apply().unwrap();
            faults::crash_at(point);
            assert!(
                crashed(|| folder_move::undo(&mut record, &fx.lib.journal)),
                "{point} reached"
            );
            let context = format!("{point} (row: {has_row})");
            assert_eq!(fx.on_disk().state, St::Undoing, "{context}");
            let outcome = fx.recover_twice(&context);
            // Before the library change an Undo is reversed (the move
            // stands and can be undone again); after it, it is finished.
            // Without a library row the files alone decide.
            let finished = matches!(point, "undo:committed" | "undo:removed-created")
                || (!has_row && point != "undo:undoing");
            assert_eq!(
                outcome,
                if finished { St::Undone } else { St::Committed },
                "{context}"
            );
            if outcome == St::Committed {
                let mut again = fx.on_disk();
                folder_move::undo(&mut again, &fx.lib.journal).unwrap();
                fx.agrees(St::Undone, &context);
            }
        }
    }
}

#[test]
fn recovery_interrupted_after_putting_a_folder_back_settles_on_the_next_run() {
    // Library row present, folder renamed, library never committed: the
    // first recovery puts the folder back and dies before recording.
    let fx = FolderFx::new(true);
    faults::crash_at("apply:rename-durable");
    assert!(crashed(|| fx.apply()));
    faults::crash_at("recover:put-back");
    let mut record = fx.on_disk();
    assert!(crashed(|| folder_move::recover(
        &mut record,
        &fx.lib.journal
    )));
    assert_eq!(fx.on_disk().state, St::Planned);
    assert_eq!(fx.recover_twice("after put-back"), St::RolledBack);
    // An Undo whose library change never committed, recovered with a crash
    // after returning the folder.
    let fx = FolderFx::new(true);
    let mut record = fx.apply().unwrap();
    faults::crash_at("undo:rename-durable");
    assert!(crashed(|| folder_move::undo(&mut record, &fx.lib.journal)));
    faults::crash_at("recover:returned");
    let mut record = fx.on_disk();
    assert!(crashed(|| folder_move::recover(
        &mut record,
        &fx.lib.journal
    )));
    assert_eq!(fx.on_disk().state, St::Undoing);
    assert_eq!(fx.recover_twice("after return"), St::Committed);
}

/// Runs `op` once with no faults to count its directory syncs, then once per
/// sync with that sync failing. After each failure the journal claims only
/// what is true, and recovery (with no faults) settles it.
fn every_sync_failure(has_row: bool, undo: bool) {
    let prepare = |fx: &FolderFx| -> Option<FolderMoveRecord> { undo.then(|| fx.apply().unwrap()) };
    let run = |fx: &FolderFx, record: &mut Option<FolderMoveRecord>| -> Result<(), String> {
        match record {
            Some(r) => folder_move::undo(r, &fx.lib.journal),
            None => fx.apply().map(|_| ()),
        }
    };
    let fx = FolderFx::new(has_row);
    let mut record = prepare(&fx);
    faults::reset();
    run(&fx, &mut record).unwrap();
    let count = faults::syncs();
    assert!(count >= 4, "syncs counted: {count}");
    for index in 0..count {
        let fx = FolderFx::new(has_row);
        let mut record = prepare(&fx);
        faults::fail_sync(index);
        let context = format!("sync {index} of {count} (row: {has_row}, undo: {undo})");
        assert!(
            run(&fx, &mut record).is_err(),
            "{context}: the failure is reported"
        );
        if folder_move::records(&fx.lib.journal).is_empty() {
            // The very first journal write failed: nothing was started.
            assert!(
                !undo && fx.game.exists() && !fx.plan.destination.exists(),
                "{context}"
            );
            continue;
        }
        let state = fx.on_disk().state;
        assert_ne!(state, St::NeedsReview, "{context}");
        fx.agrees(state, &context);
        let outcome = fx.recover_twice(&context);
        fx.agrees(outcome, &context);
    }
}

#[test]
fn no_failed_directory_sync_is_ignored_by_a_folder_move_or_its_undo() {
    for has_row in [true, false] {
        every_sync_failure(has_row, false);
        every_sync_failure(has_row, true);
    }
}

#[test]
fn a_failed_sync_during_recovery_leaves_the_record_open() {
    for has_row in [true, false] {
        // Count the syncs one recovery of a crashed move makes.
        let fx = FolderFx::new(has_row);
        faults::crash_at("apply:renamed");
        assert!(crashed(|| fx.apply()));
        faults::reset();
        let mut record = fx.on_disk();
        folder_move::recover(&mut record, &fx.lib.journal).unwrap();
        let count = faults::syncs();
        assert!(count >= 2, "recovery syncs: {count}");
        for index in 0..count {
            let fx = FolderFx::new(has_row);
            faults::crash_at("apply:renamed");
            assert!(crashed(|| fx.apply()));
            faults::fail_sync(index);
            let mut record = fx.on_disk();
            let context = format!("recovery sync {index} of {count} (row: {has_row})");
            assert!(
                folder_move::recover(&mut record, &fx.lib.journal).is_err(),
                "{context}"
            );
            let state = fx.on_disk().state;
            assert_ne!(state, St::NeedsReview, "{context}");
            fx.agrees(state, &context);
            fx.recover_twice(&context);
        }
    }
}

/// The order of durable steps, from the trace of a clean move, Undo and
/// recovery: what each outcome claims is synced before it is recorded.
#[test]
fn folder_move_steps_are_made_durable_in_order() {
    let pos = |trace: &[String], what: &str| -> usize {
        trace
            .iter()
            .position(|t| t == what)
            .unwrap_or_else(|| panic!("{what} not in {trace:#?}"))
    };
    let last = |trace: &[String], what: &str| -> usize {
        trace
            .iter()
            .rposition(|t| t == what)
            .unwrap_or_else(|| panic!("{what} not in {trace:#?}"))
    };
    let fx = FolderFx::new(true);
    let src = fx.game.clone();
    let dst = fx.plan.destination.clone();
    let src_parent = src.parent().unwrap().display().to_string();
    let dst_parent = dst.parent().unwrap().display().to_string();
    let created = dst.parent().unwrap();
    let mut record = fx.apply().unwrap();
    let t = faults::take_trace();
    let mkdir = pos(&t, &format!("mkdir {}", created.display()));
    let created_parent_sync = mkdir
        + 1
        + t[mkdir + 1..]
            .iter()
            .position(|e| *e == format!("sync {}", created.parent().unwrap().display()))
            .expect("the created folder's parent is synced");
    let rename = pos(
        &t,
        &format!("rename {} -> {}", src.display(), dst.display()),
    );
    assert!(created_parent_sync < rename, "{t:#?}");
    let commit = pos(&t, "commit");
    for side in [&src_parent, &dst_parent] {
        let synced = last(&t[..commit], &format!("sync {side}"));
        assert!(rename < synced && synced < commit, "{side}: {t:#?}");
    }
    assert!(commit < pos(&t, "record Committed"), "{t:#?}");

    folder_move::undo(&mut record, &fx.lib.journal).unwrap();
    let t = faults::take_trace();
    let intent = pos(&t, "record Undoing");
    let rename = pos(
        &t,
        &format!("rename {} -> {}", dst.display(), src.display()),
    );
    let commit = pos(&t, "commit");
    assert!(intent < rename && rename < commit, "{t:#?}");
    for side in [&src_parent, &dst_parent] {
        let synced = last(&t[..commit], &format!("sync {side}"));
        assert!(rename < synced, "{side}: {t:#?}");
    }
    let rmdir = pos(&t, &format!("rmdir {}", created.display()));
    let done = pos(&t, "record Undone");
    let removal_synced = last(
        &t[..done],
        &format!("sync {}", created.parent().unwrap().display()),
    );
    assert!(
        commit < rmdir && rmdir < removal_synced && removal_synced < done,
        "{t:#?}"
    );

    // Recovery that puts a folder back syncs both sides before recording.
    let fx = FolderFx::new(true);
    faults::crash_at("apply:rename-durable");
    assert!(crashed(|| fx.apply()));
    faults::take_trace();
    let mut record = fx.on_disk();
    folder_move::recover(&mut record, &fx.lib.journal).unwrap();
    let t = faults::take_trace();
    let back = pos(
        &t,
        &format!(
            "rename {} -> {}",
            fx.plan.destination.display(),
            fx.game.display()
        ),
    );
    let recorded = pos(&t, "record RolledBack");
    let src_parent = fx.game.parent().unwrap().display().to_string();
    let synced = last(&t[..recorded], &format!("sync {src_parent}"));
    assert!(back < synced && synced < recorded, "{t:#?}");
}

#[test]
fn records_written_before_undoing_existed_keep_their_meaning() {
    // A version-1 record as earlier releases wrote it (no Undoing state):
    // each settled state is reported unchanged and is not reinterpreted.
    let fx = FolderFx::new(true);
    let record = fx.apply().unwrap();
    for state in [St::Committed, St::Undone, St::RolledBack, St::NeedsReview] {
        let mut old = record.clone();
        old.state = state;
        let text = serde_json::to_string(&old).unwrap();
        let mut back: FolderMoveRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(back.version, 1);
        assert_eq!(
            folder_move::recover(&mut back, &fx.lib.journal).unwrap(),
            state
        );
    }
    fx.agrees(St::Committed, "untouched");
}

/// The folder-move record format of the release installed before this
/// change, copied verbatim from 756d9740 (identical at 4a0f635d, as is its
/// `FolderRow`), with the previous `records()` reader.
mod previous_release {
    use crate::database::organisation_paths::FolderRow;
    use serde::{Deserialize, Serialize};
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum FolderMoveState {
        Planned,
        Committed,
        Undone,
        RolledBack,
        NeedsReview,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct FolderMoveRecord {
        pub version: u32,
        pub id: String,
        pub source: PathBuf,
        pub destination: PathBuf,
        pub root: PathBuf,
        pub platform: String,
        pub database: PathBuf,
        pub layout_digest: String,
        pub entries: usize,
        pub total_bytes: u64,
        pub rows: Vec<FolderRow>,
        pub created_dirs: Vec<PathBuf>,
        pub state: FolderMoveState,
        pub at_unix: u64,
    }

    pub fn records(journal_dir: &Path) -> Vec<FolderMoveRecord> {
        let mut found: Vec<FolderMoveRecord> =
            std::fs::read_dir(journal_dir.join(super::folder_move::JOURNAL_SUBDIR))
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|text| serde_json::from_str(&text).ok())
                .collect();
        found.sort_by(|a, b| b.id.cmp(&a.id));
        found
    }
}

fn record_text(fx: &FolderFx) -> String {
    let dir = fx.lib.journal.join(folder_move::JOURNAL_SUBDIR);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(files.len(), 1);
    std::fs::read_to_string(files.pop().unwrap()).unwrap()
}

/// Every state this release writes, read by the previous release's reader:
/// all decode with the same meaning except `Undoing`, which the previous
/// release cannot decode and so silently skips. That is why the upgrade
/// preflight and the installer refuse while an `Undoing` record exists.
#[test]
fn the_previous_release_reads_every_state_except_undoing_which_it_would_skip() {
    let decode = |text: &str| serde_json::from_str::<previous_release::FolderMoveRecord>(text);
    // Committed, then Undone.
    let fx = FolderFx::new(true);
    let mut record = fx.apply().unwrap();
    let old = decode(&record_text(&fx)).unwrap();
    assert_eq!(old.state, previous_release::FolderMoveState::Committed);
    assert_eq!(
        (old.version, &old.id, &old.rows),
        (1, &record.id, &record.rows)
    );
    folder_move::undo(&mut record, &fx.lib.journal).unwrap();
    assert_eq!(
        decode(&record_text(&fx)).unwrap().state,
        previous_release::FolderMoveState::Undone
    );
    // Planned (interrupted move), then RolledBack after recovery.
    let fx = FolderFx::new(true);
    faults::crash_at("apply:renamed");
    assert!(crashed(|| fx.apply()));
    assert_eq!(
        decode(&record_text(&fx)).unwrap().state,
        previous_release::FolderMoveState::Planned
    );
    fx.recover_twice("rolled back");
    assert_eq!(
        decode(&record_text(&fx)).unwrap().state,
        previous_release::FolderMoveState::RolledBack
    );
    // Undoing: not decodable, so invisible to the previous release.
    let fx = FolderFx::new(true);
    let mut record = fx.apply().unwrap();
    faults::crash_at("undo:renamed");
    assert!(crashed(|| folder_move::undo(&mut record, &fx.lib.journal)));
    let error = decode(&record_text(&fx)).unwrap_err().to_string();
    assert!(error.contains("unknown variant `Undoing`"), "{error}");
    assert!(previous_release::records(&fx.lib.journal).is_empty());
    // This release lists it as interrupted and does not report it unreadable.
    assert_eq!(folder_move::interrupted(&fx.lib.journal).len(), 1);
    assert!(folder_move::unreadable(&fx.lib.journal).is_empty());
}

/// Records the previous release wrote are read by this release with the
/// same meaning, and recovery of an open one behaves as before.
#[test]
fn records_the_previous_release_wrote_keep_their_meaning_here() {
    let fx = FolderFx::new(true);
    let record = fx.apply().unwrap();
    let mut old: previous_release::FolderMoveRecord =
        serde_json::from_str(&record_text(&fx)).unwrap();
    for (state, now) in [
        (previous_release::FolderMoveState::Planned, St::Planned),
        (previous_release::FolderMoveState::Committed, St::Committed),
        (previous_release::FolderMoveState::Undone, St::Undone),
        (
            previous_release::FolderMoveState::RolledBack,
            St::RolledBack,
        ),
        (
            previous_release::FolderMoveState::NeedsReview,
            St::NeedsReview,
        ),
    ] {
        old.state = state;
        let read: FolderMoveRecord = serde_json::from_str(&serde_json::to_string(&old).unwrap())
            .expect("this release reads the previous format");
        assert_eq!((read.state, read.version, &read.id), (now, 1, &record.id));
    }
    // An open record from the previous release, for a move that completed:
    // recovered as Committed, as the previous release would have.
    old.state = previous_release::FolderMoveState::Planned;
    let mut open: FolderMoveRecord =
        serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
    assert_eq!(
        folder_move::recover(&mut open, &fx.lib.journal).unwrap(),
        St::Committed
    );
    fx.agrees(St::Committed, "previous-format record");
}

#[test]
fn unreadable_or_unknown_folder_move_records_are_reported_not_skipped() {
    let fx = FolderFx::new(true);
    fx.apply().unwrap();
    let dir = fx.lib.journal.join(folder_move::JOURNAL_SUBDIR);
    std::fs::write(dir.join("folder-move-damaged.json"), b"{ not json").unwrap();
    let mut future: serde_json::Value =
        serde_json::from_str(&record_text_named(&dir, "folder-move-")).unwrap();
    future["state"] = "Relocating".into();
    future["id"] = "folder-move-from-a-newer-release".into();
    std::fs::write(
        dir.join("folder-move-from-a-newer-release.json"),
        serde_json::to_vec(&future).unwrap(),
    )
    .unwrap();
    let reported = folder_move::unreadable(&fx.lib.journal);
    let names: Vec<_> = reported
        .iter()
        .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "folder-move-damaged.json",
            "folder-move-from-a-newer-release.json"
        ]
    );
    assert!(
        reported[1].1.contains("unknown variant `Relocating`"),
        "{:?}",
        reported[1]
    );
    // The readable record is unaffected and nothing was changed.
    assert_eq!(folder_move::records(&fx.lib.journal).len(), 1);
    fx.agrees(St::Committed, "after reporting");
}

#[test]
fn a_record_with_a_newer_version_is_reported_and_never_acted_on() {
    let fx = FolderFx::new(true);
    faults::crash_at("apply:renamed");
    assert!(crashed(|| fx.apply()));
    let dir = fx.lib.journal.join(folder_move::JOURNAL_SUBDIR);
    let path = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "json"))
        .unwrap();
    let mut newer: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(newer["state"], "Planned", "a known state");
    newer["version"] = (folder_move::RECORD_VERSION + 1).into();
    let text = serde_json::to_vec(&newer).unwrap();
    std::fs::write(&path, &text).unwrap();
    let reported = folder_move::unreadable(&fx.lib.journal);
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(reported[0].0, path);
    assert!(reported[0].1.contains("newer EmuWiz"), "{:?}", reported[0]);
    assert!(folder_move::records(&fx.lib.journal).is_empty());
    assert!(
        folder_move::interrupted(&fx.lib.journal).is_empty(),
        "never recovered"
    );
    assert_eq!(std::fs::read(&path).unwrap(), text, "left untouched");
}

#[test]
fn recovery_settles_a_move_whose_emptied_source_folder_was_removed() {
    for has_row in [true, false] {
        let fx = FolderFx::new(has_row);
        faults::crash_at("apply:committed");
        assert!(crashed(|| fx.apply()));
        assert_eq!(fx.on_disk().state, St::Planned);
        // The person removed the folder the game came from (in this test
        // scratch library it still holds the fixture's other file).
        assert!(!fx.game.exists() && fx.plan.destination.exists());
        std::fs::remove_dir_all(fx.game.parent().unwrap()).unwrap();
        faults::reset();
        let mut record = fx.on_disk();
        assert_eq!(
            folder_move::recover(&mut record, &fx.lib.journal).unwrap(),
            St::Committed,
            "row: {has_row}"
        );
        assert_eq!(fx.on_disk().state, St::Committed);
        assert!(folder_move::interrupted(&fx.lib.journal).is_empty());
        assert!(fx.plan.destination.exists());
    }
}

/// The text of the one record whose name starts with `prefix` and that is
/// not one of the test's own files.
fn record_text_named(dir: &Path, prefix: &str) -> String {
    let path = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with(prefix) && name.ends_with(".json") && !name.contains("damaged")
        })
        .unwrap();
    std::fs::read_to_string(path).unwrap()
}

/// Builds (phase `create`) or settles (phase `settle`) a disposable EmuWiz
/// profile for the manual downgrade check in docs/RELEASE_ENGINEERING.md:
/// `<dir>/home` with a library and two interrupted folder Undos (one with a
/// library row, one without), games under `<dir>/games`. Run explicitly:
///   EMUWIZ_ORGANISE_DOWNGRADE_DIR=<empty dir> EMUWIZ_ORGANISE_DOWNGRADE_PHASE=create \
///   cargo test -p archivefs-core --lib downgrade_profile -- --ignored
#[test]
#[ignore = "manual downgrade check: run explicitly with EMUWIZ_ORGANISE_DOWNGRADE_DIR"]
fn downgrade_profile() {
    let Some(dir) = std::env::var_os("EMUWIZ_ORGANISE_DOWNGRADE_DIR").map(PathBuf::from) else {
        eprintln!("SKIPPED: EMUWIZ_ORGANISE_DOWNGRADE_DIR is not set");
        return;
    };
    let data = dir.join("home/.local/share/emuwiz");
    let journal = data.join(crate::dat::rename_apply::journal::RENAME_TRANSACTIONS_DIRECTORY);
    let phase = std::env::var("EMUWIZ_ORGANISE_DOWNGRADE_PHASE").unwrap_or_default();
    if phase == "settle" {
        for mut record in folder_move::interrupted(&journal) {
            let state = folder_move::recover(&mut record, &journal).unwrap();
            println!("SETTLED {} {state:?}", record.id);
        }
        return;
    }
    assert_eq!(
        phase, "create",
        "EMUWIZ_ORGANISE_DOWNGRADE_PHASE must be create or settle"
    );
    assert!(
        std::fs::read_dir(&dir).unwrap().next().is_none(),
        "the folder must be empty"
    );
    let root = dir.join("games");
    std::fs::create_dir_all(root.join("mixed")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(root.join("mixed/Adventure.zip"), b"bytes").unwrap();
    let mut db = crate::Database::open_or_create(data.join("library.sqlite3")).unwrap();
    db.register_source_folders(std::slice::from_ref(&root))
        .unwrap();
    let folder = db.list_source_folders().unwrap()[0].id;
    let a = crate::Archive::from_path_in_root(root.join("mixed/Adventure.zip"), &root).unwrap();
    db.upsert_archive(folder, &root, &a).unwrap();
    let lib = Library {
        _temp: tempfile::tempdir().unwrap(),
        root: root.clone(),
        db,
        journal: journal.clone(),
    };
    let scumm = root.join("mixed/Adventure");
    std::fs::create_dir_all(scumm.join("data")).unwrap();
    std::fs::write(scumm.join("data/RESOURCE.001"), b"res").unwrap();
    catalogue_as(&lib, &root.join("mixed/Adventure.zip"), &scumm);
    let ps3 = root.join("mixed/Game");
    ps3_folder(&ps3, "BLES00010");
    let mut candidates = found_game_folders(std::slice::from_ref(&root));
    candidates.push(FolderCandidate {
        path: scumm.clone(),
        platform: "ScummVM".into(),
        evidence: "your system choice".into(),
        chosen: true,
    });
    for (_, preview) in preview_folders(&lib.db, std::slice::from_ref(&root), &candidates) {
        let folder_move::FolderPreview::Move(plan) = preview else {
            panic!("expected a move, got {preview:?}");
        };
        let mut record = folder_move::apply(&plan, lib.db.path(), &journal).unwrap();
        faults::crash_at("undo:renamed");
        assert!(crashed(|| folder_move::undo(&mut record, &journal)));
        println!(
            "UNDOING {} rows={} source={} destination={}",
            record.id,
            record.rows.len(),
            record.source.display(),
            record.destination.display()
        );
    }
    assert_eq!(folder_move::interrupted(&journal).len(), 2);
}
