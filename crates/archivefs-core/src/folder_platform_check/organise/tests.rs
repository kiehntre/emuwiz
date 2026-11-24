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

// --- Disc sets an earlier operation left split (owner review of #536) ---

/// A disc set an earlier organisation moved and could not make whole: the
/// cue and track 1 at the new place, track 2 back in its old folder, left
/// needing recovery as a rollback that could not finish leaves it. With
/// `bulk_rename_tags` the journal also carries the disc-set tag Bulk Rename
/// writes; without, it is exactly what Folder platform check writes (the
/// companion tag on the tracks only; no other tool writes that layout).
/// An unrelated game moves in the same
/// operation and must never be counted as part of the set. Returns the
/// stray track's path.
fn split_disc_set(lib: &Library, bulk_rename_tags: bool) -> PathBuf {
    split_disc_set_at(lib, bulk_rename_tags, &lib.root.join("mixed/Saga"))
}

const SET_FILES: [&str; 3] = [
    "Disc Game.cue",
    "Disc Game (Track 1).bin",
    "Disc Game (Track 2).bin",
];

/// A cue/bin set in `old` and an unrelated game, moved to the PS2 folder by
/// a real Folder platform check Apply; the set is whole at its new place.
/// With `bulk_rename_tags` the journal is rewritten with the disc-set tag
/// Bulk Rename writes. Returns the operation and the set's new folder.
fn moved_disc_set_at(
    lib: &Library,
    bulk_rename_tags: bool,
    old: &Path,
) -> (crate::dat::rename_apply::RenameTransaction, PathBuf) {
    use crate::dat::rename_apply::model::TransactionOperation;
    std::fs::create_dir_all(old).unwrap();
    std::fs::write(
        old.join("Disc Game.cue"),
        "FILE \"Disc Game (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\nFILE \"Disc Game (Track 2).bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(old.join("Disc Game (Track 1).bin"), b"data track").unwrap();
    std::fs::write(old.join("Disc Game (Track 2).bin"), b"audio track").unwrap();
    std::fs::write(lib.root.join("mixed/Unrelated.iso"), b"another game").unwrap();
    let chosen = [
        ChosenGame {
            archive_id: 0,
            path: old.join("Disc Game.cue"),
            platform: "PS2".into(),
        },
        ChosenGame {
            archive_id: 0,
            path: lib.root.join("mixed/Unrelated.iso"),
            platform: "PS2".into(),
        },
    ];
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
    assert_eq!(outcome.summary.applied, 4);
    let mut tx = outcome.transaction;
    let lead = old.join("Disc Game.cue").to_string_lossy().into_owned();
    if bulk_rename_tags {
        for entry in &mut tx.entries {
            if matches!(entry.operation, TransactionOperation::RenameMove)
                && entry.source_path.starts_with(old)
            {
                entry.unknown.insert(
                    crate::database::organisation_paths::DISC_SET_KEY.into(),
                    serde_json::json!({ "lead": lead }),
                );
            }
        }
        crate::dat::rename_apply::journal::write_journal(&lib.journal, &tx).unwrap();
    }
    let new = lib
        .root
        .join(crate::platform::canonical_layout_folder("PS2").unwrap());
    assert!(SET_FILES.iter().all(|f| new.join(f).exists()));
    (tx, new)
}

/// [`moved_disc_set_at`], then left split: track 2 back in `old`, the
/// operation left needing recovery. Returns the stray track's path.
fn split_disc_set_at(lib: &Library, bulk_rename_tags: bool, old: &Path) -> PathBuf {
    use crate::dat::rename_apply::model::TransactionState;
    let (mut tx, new) = moved_disc_set_at(lib, bulk_rename_tags, old);
    let lead = old.join("Disc Game.cue").to_string_lossy().into_owned();
    tx.state = TransactionState::RollbackFailed;
    crate::dat::rename_apply::journal::write_journal(&lib.journal, &tx).unwrap();
    let stray = old.join("Disc Game (Track 2).bin");
    std::fs::rename(new.join("Disc Game (Track 2).bin"), &stray).unwrap();
    // One set (never counted twice), with all three files at both places,
    // and nothing of the unrelated game.
    let split = crate::dat::rename_apply::split_disc_set_paths(&tx);
    assert_eq!(split.len(), 1, "the set is split: {split:?}");
    assert_eq!(split[0].0, lead);
    let mut paths = split[0].1.clone();
    paths.sort();
    let mut expected: Vec<PathBuf> = SET_FILES
        .iter()
        .flat_map(|f| [old.join(f), new.join(f)])
        .collect();
    expected.sort();
    assert_eq!(paths, expected);
    assert!(new.join("Unrelated.iso").exists());
    stray
}

fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).unwrap().ino()
}

#[test]
fn a_file_of_a_disc_set_left_split_is_never_moved_on_its_own() {
    for bulk_rename_tags in [true, false] {
        a_file_of_a_split_set_is_never_moved(bulk_rename_tags);
    }
}

fn a_file_of_a_split_set_is_never_moved(bulk_rename_tags: bool) {
    let lib = library(&["mixed/Other.zip"]);
    let stray = split_disc_set(&lib, bulk_rename_tags);
    let before = inode(&stray);
    let chosen = [ChosenGame {
        archive_id: 0,
        path: stray.clone(),
        platform: "SNES".into(),
    }];
    let mut plans =
        plan_organisation_with_choices(&[], &chosen, &[], std::slice::from_ref(&lib.root), 1);
    prepare(&mut plans[0], &lib.db).unwrap();
    let approved = approve_all(&plans[0]);
    assert!(
        approved.contains(&*stray.to_string_lossy()),
        "precondition: the stray track is offered for a move"
    );
    // The Bulk Rename / Quick Rename lock reads the same detector: it blocks
    // the file in a preview and refuses it at Apply, for either layout.
    use crate::dat::rom_organisation::rename_guard;
    assert!(rename_guard::refuse_split_disc_sets(&plans[0].plan, &approved, &lib.journal).is_err());
    let mut locked = plans[0].plan.clone();
    assert_eq!(
        rename_guard::block_split_disc_sets(&mut locked, &lib.journal),
        1
    );
    let result = apply(
        &plans[0],
        &approved,
        &lib.db,
        &lib.journal,
        &AtomicBool::new(false),
    );
    let Err((error, _)) = result else {
        panic!("a file of a disc set left in two places was moved");
    };
    assert!(error.to_string().contains("in two places"), "{error}");
    assert_eq!(inode(&stray), before, "the file never moved");
    let snes = lib
        .root
        .join(crate::platform::canonical_layout_folder("SNES").unwrap());
    assert!(!snes.exists(), "nothing was created");
}

#[test]
fn a_folder_holding_a_file_of_a_disc_set_left_split_is_never_moved() {
    for bulk_rename_tags in [true, false] {
        a_folder_holding_a_split_set_file_is_never_moved(bulk_rename_tags);
    }
}

fn a_folder_holding_a_split_set_file_is_never_moved(bulk_rename_tags: bool) {
    let lib = library(&["mixed/Other.zip"]);
    let stray = split_disc_set(&lib, bulk_rename_tags);
    let folder = stray.parent().unwrap().to_path_buf();
    let before = inode(&stray);
    let candidate = FolderCandidate {
        path: folder.clone(),
        platform: "ScummVM".into(),
        evidence: "your system choice".into(),
        chosen: true,
    };
    let previews = preview_folders(&lib.db, std::slice::from_ref(&lib.root), &[candidate]);
    let folder_move::FolderPreview::Move(plan) = &previews[0].1 else {
        panic!("precondition: expected a move, got {:?}", previews[0].1);
    };
    let Err(error) = folder_move::apply(plan, lib.db.path(), &lib.journal) else {
        panic!("a folder holding a file of a disc set left in two places was moved");
    };
    assert!(error.contains("in two places"), "{error}");
    assert_eq!(inode(&stray), before, "the file never moved");
    assert!(folder.is_dir() && !plan.destination.exists());
    assert!(
        folder_move::records(&lib.journal).is_empty(),
        "nothing started"
    );
}

#[test]
fn organise_refuses_file_and_folder_moves_while_history_cannot_be_read() {
    let lib = library(&["mixed/Game.sfc"]);
    std::fs::create_dir_all(&lib.journal).unwrap();
    std::fs::write(lib.journal.join("unreadable.json"), b"{ not a journal").unwrap();
    let file = lib.root.join("mixed/Game.sfc");
    let file_before = inode(&file);
    let chosen = [ChosenGame {
        archive_id: 0,
        path: file.clone(),
        platform: "SNES".into(),
    }];
    let mut plans =
        plan_organisation_with_choices(&[], &chosen, &[], std::slice::from_ref(&lib.root), 1);
    prepare(&mut plans[0], &lib.db).unwrap();
    let approved = approve_all(&plans[0]);
    assert!(approved.contains(&*file.to_string_lossy()), "precondition");
    let Err((error, _)) = apply(
        &plans[0],
        &approved,
        &lib.db,
        &lib.journal,
        &AtomicBool::new(false),
    ) else {
        panic!("a file was moved although History could not be read");
    };
    assert!(
        error
            .to_string()
            .contains("A History record could not be read")
            && error.to_string().contains("unreadable.json")
            && error.to_string().contains("Open History"),
        "{error}"
    );
    assert_eq!(inode(&file), file_before);

    let folder = lib.root.join("mixed/Adventure");
    std::fs::create_dir_all(folder.join("data")).unwrap();
    std::fs::write(folder.join("data/RESOURCE.001"), b"res").unwrap();
    let candidate = FolderCandidate {
        path: folder.clone(),
        platform: "ScummVM".into(),
        evidence: "your system choice".into(),
        chosen: true,
    };
    let previews = preview_folders(&lib.db, std::slice::from_ref(&lib.root), &[candidate]);
    let folder_move::FolderPreview::Move(plan) = &previews[0].1 else {
        panic!("precondition: expected a move, got {:?}", previews[0].1);
    };
    let Err(error) = folder_move::apply(plan, lib.db.path(), &lib.journal) else {
        panic!("a folder was moved although History could not be read");
    };
    assert!(
        error.contains("A History record could not be read") && error.contains("unreadable.json"),
        "{error}"
    );
    assert!(folder.join("data/RESOURCE.001").exists() && !plan.destination.exists());
}

/// What Folder platform check's file-move Apply returns.
type MoveResult = Result<
    ApplyOutcome,
    (
        ApplyError,
        Option<Box<crate::dat::rename_apply::RenameTransaction>>,
    ),
>;

/// A real file move of `path` to `platform` by Folder platform check.
fn move_file(lib: &Library, path: &Path, platform: &str) -> (RootPlan, MoveResult) {
    let chosen = [ChosenGame {
        archive_id: 0,
        path: path.to_path_buf(),
        platform: platform.into(),
    }];
    let mut plans =
        plan_organisation_with_choices(&[], &chosen, &[], std::slice::from_ref(&lib.root), 1);
    prepare(&mut plans[0], &lib.db).unwrap();
    let approved = approve_all(&plans[0]);
    let result = apply(
        &plans[0],
        &approved,
        &lib.db,
        &lib.journal,
        &AtomicBool::new(false),
    );
    (plans.remove(0), result)
}

/// A real folder move preview of `folder` to ScummVM.
fn folder_plan(lib: &Library, folder: &Path) -> folder_move::FolderMovePlan {
    std::fs::create_dir_all(folder.join("data")).unwrap();
    std::fs::write(folder.join("data/RESOURCE.001"), b"res").unwrap();
    let candidate = FolderCandidate {
        path: folder.to_path_buf(),
        platform: "ScummVM".into(),
        evidence: "your system choice".into(),
        chosen: true,
    };
    let previews = preview_folders(&lib.db, std::slice::from_ref(&lib.root), &[candidate]);
    match &previews[0].1 {
        folder_move::FolderPreview::Move(plan) => (**plan).clone(),
        other => panic!("precondition: expected a move, got {other:?}"),
    }
}

/// Review (false positive): a set the earlier operation moved and a later
/// operation moved on, whole, has no file at either recorded place. It is
/// not reported split, and its old paths are free again.
#[test]
fn a_set_moved_on_by_a_later_operation_is_not_locked() {
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let old = lib.root.join("mixed/Saga");
        let (first, ps2) = moved_disc_set_at(&lib, bulk_rename_tags, &old);
        let (_, later) = move_file(&lib, &ps2.join("Disc Game.cue"), "SNES");
        let later = later.unwrap();
        assert_eq!(later.summary.applied, 3, "the set moved on, whole");
        let snes = lib
            .root
            .join(crate::platform::canonical_layout_folder("SNES").unwrap());
        assert!(SET_FILES.iter().all(|f| snes.join(f).exists()));
        assert!(crate::dat::rename_apply::split_disc_set_paths(&first).is_empty());
        assert!(crate::dat::rename_apply::split_disc_set_paths(&later.transaction).is_empty());
        // The folder that held the set may move, and a file may move onto
        // a place the first operation recorded.
        let plan = folder_plan(&lib, &old);
        folder_move::apply(&plan, lib.db.path(), &lib.journal).unwrap();
        assert!(plan.destination.join("data/RESOURCE.001").exists());
        let elsewhere = lib.root.join("mixed/Elsewhere/Disc Game (Track 1).bin");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, b"another game's track").unwrap();
        let (_, onto) = move_file(&lib, &elsewhere, "PS2");
        assert!(onto.is_ok(), "{:?}", onto.err().map(|e| e.0.to_string()));
        let occupant = ps2.join("Disc Game (Track 1).bin");
        assert!(occupant.exists());
        // Documented limit (fails closed): once something sits at a place
        // the first operation recorded, that set's paths are locked again,
        // the occupant included. A later move of files other than the main
        // file is never taken as keeping the set together.
        use crate::dat::rom_organisation::rename_guard;
        assert!(
            rename_guard::refuse_split_disc_set_paths([occupant.as_path()], &lib.journal)
                .unwrap_err()
                .contains("in two places")
        );
    }
}

/// A genuine split stays locked when a file of the set is missing: track 1
/// at its new place, track 2 at its old place, the cue gone.
#[test]
fn a_split_set_with_a_missing_file_stays_locked() {
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let stray = split_disc_set(&lib, bulk_rename_tags);
        let ps2 = lib
            .root
            .join(crate::platform::canonical_layout_folder("PS2").unwrap());
        std::fs::remove_file(ps2.join("Disc Game.cue")).unwrap();
        let (stray_before, track_one_before) =
            (inode(&stray), inode(&ps2.join("Disc Game (Track 1).bin")));
        let (_, result) = move_file(&lib, &stray, "SNES");
        let Err((error, _)) = result else {
            panic!("a file of a split set with a missing file was moved");
        };
        assert!(error.to_string().contains("in two places"), "{error}");
        let plan = folder_plan(&lib, stray.parent().unwrap());
        let error = folder_move::apply(&plan, lib.db.path(), &lib.journal).unwrap_err();
        assert!(error.contains("in two places"), "{error}");
        assert_eq!(inode(&stray), stray_before);
        assert_eq!(
            inode(&ps2.join("Disc Game (Track 1).bin")),
            track_one_before
        );
        assert!(!plan.destination.exists());
    }
}

/// Files that disappear (a disk not attached) and come back split are
/// locked again: every check reads the disk afresh.
#[test]
fn a_split_set_whose_files_disappear_and_return_is_locked_again() {
    let lib = library(&["mixed/Other.zip"]);
    let stray = split_disc_set(&lib, false);
    let ps2 = lib
        .root
        .join(crate::platform::canonical_layout_folder("PS2").unwrap());
    let present = [
        ps2.join("Disc Game.cue"),
        ps2.join("Disc Game (Track 1).bin"),
        stray.clone(),
    ];
    let before: Vec<u64> = present.iter().map(|p| inode(p)).collect();
    let away = lib._temp.path().join("not attached");
    std::fs::create_dir_all(&away).unwrap();
    for (i, path) in present.iter().enumerate() {
        std::fs::rename(path, away.join(i.to_string())).unwrap();
    }
    use crate::dat::rom_organisation::rename_guard;
    assert!(
        rename_guard::refuse_split_disc_set_paths(
            present.iter().map(PathBuf::as_path),
            &lib.journal
        )
        .is_ok(),
        "nothing of the set is here to protect"
    );
    for (i, path) in present.iter().enumerate() {
        std::fs::rename(away.join(i.to_string()), path).unwrap();
    }
    assert!(rename_guard::refuse_split_disc_set_paths([stray.as_path()], &lib.journal).is_err());
    let (_, result) = move_file(&lib, &stray, "SNES");
    assert!(result.is_err(), "the returned split set is locked again");
    let after: Vec<u64> = present.iter().map(|p| inode(p)).collect();
    assert_eq!(after, before);
}

/// The main file of a split set (its track 2 left behind) never moves.
#[test]
fn the_main_file_of_a_split_set_never_moves() {
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let stray = split_disc_set(&lib, bulk_rename_tags);
        let ps2 = lib
            .root
            .join(crate::platform::canonical_layout_folder("PS2").unwrap());
        let cue = ps2.join("Disc Game.cue");
        let before = [
            inode(&cue),
            inode(&ps2.join("Disc Game (Track 1).bin")),
            inode(&stray),
        ];
        let (plan, _) = move_file(&lib, &cue, "SNES");
        // Refused before Apply by the set check (track 2 is not beside it):
        // never offered, and given a reason. The split-set lock covers the
        // main file as well (it is one of the set's recorded paths).
        assert!(
            !approve_all(&plan).contains(&*cue.to_string_lossy()),
            "the main file of a split set was offered: {plan:?}"
        );
        assert!(
            plan.left_alone.iter().any(|(path, _)| path == &cue)
                || plan
                    .plan
                    .entries
                    .iter()
                    .any(|e| e.source_path == cue && e.status != OrganisationStatus::Suggested),
            "the main file was not given a reason: {plan:?}"
        );
        use crate::dat::rom_organisation::rename_guard;
        assert!(
            rename_guard::refuse_split_disc_set_paths([cue.as_path()], &lib.journal)
                .unwrap_err()
                .contains("in two places")
        );
        let after = [
            inode(&cue),
            inode(&ps2.join("Disc Game (Track 1).bin")),
            inode(&stray),
        ];
        assert_eq!(after, before);
    }
}

/// Another game's file never moves onto a place a split set recorded (the
/// vacant new place of the stray track).
#[test]
fn no_file_moves_onto_a_place_of_a_split_set() {
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let _stray = split_disc_set(&lib, bulk_rename_tags);
        let ps2 = lib
            .root
            .join(crate::platform::canonical_layout_folder("PS2").unwrap());
        let vacant = ps2.join("Disc Game (Track 2).bin");
        assert!(!vacant.exists());
        let other = lib.root.join("mixed/Other/Disc Game (Track 2).bin");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"another game's track").unwrap();
        let before = inode(&other);
        let (plan, result) = move_file(&lib, &other, "PS2");
        assert!(
            approve_all(&plan).contains(&*other.to_string_lossy()),
            "precondition: offered"
        );
        let Err((error, _)) = result else {
            panic!("a file was moved onto a place of a split set");
        };
        assert!(error.to_string().contains("in two places"), "{error}");
        assert_eq!(inode(&other), before);
        assert!(!vacant.exists());
    }
}

/// A folder never moves to a destination that holds a file of a split set
/// (the set appeared there after the folder's preview).
#[test]
fn a_folder_never_moves_onto_a_split_set() {
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let folder = lib.root.join("mixed/Saga Two");
        let plan = folder_plan(&lib, &folder);
        assert!(
            !plan.destination.exists(),
            "precondition: destination free at preview"
        );
        let stray = split_disc_set_at(&lib, bulk_rename_tags, &plan.destination);
        assert!(stray.starts_with(&plan.destination));
        let before = (inode(&stray), inode(&folder.join("data/RESOURCE.001")));
        let error = folder_move::apply(&plan, lib.db.path(), &lib.journal).unwrap_err();
        assert!(error.contains("in two places"), "{error}");
        assert_eq!(
            (inode(&stray), inode(&folder.join("data/RESOURCE.001"))),
            before
        );
        assert!(
            folder_move::records(&lib.journal).is_empty(),
            "nothing started"
        );
    }
}

/// Review (final candidate): a split set whose files are back at their
/// recorded places as copies (a restore, or the disk returning under
/// another device: same paths, new inodes) stays locked.
#[test]
fn a_split_set_restored_as_copies_stays_locked() {
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let stray = split_disc_set(&lib, bulk_rename_tags);
        let ps2 = lib
            .root
            .join(crate::platform::canonical_layout_folder("PS2").unwrap());
        let present = [
            ps2.join("Disc Game.cue"),
            ps2.join("Disc Game (Track 1).bin"),
            stray.clone(),
        ];
        let originals: Vec<u64> = present.iter().map(|p| inode(p)).collect();
        for path in &present {
            let copy = path.with_extension("restoring");
            std::fs::copy(path, &copy).unwrap();
            std::fs::rename(&copy, path).unwrap();
        }
        let copies: Vec<u64> = present.iter().map(|p| inode(p)).collect();
        assert!(
            copies.iter().zip(&originals).all(|(a, b)| a != b),
            "precondition: every file is a copy with a new inode"
        );
        let (_, result) = move_file(&lib, &stray, "SNES");
        let Err((error, _)) = result else {
            panic!("a restored file of a split set was moved");
        };
        assert!(error.to_string().contains("in two places"), "{error}");
        let plan = folder_plan(&lib, stray.parent().unwrap());
        let error = folder_move::apply(&plan, lib.db.path(), &lib.journal).unwrap_err();
        assert!(error.contains("in two places"), "{error}");
        assert_eq!(present.iter().map(|p| inode(p)).collect::<Vec<_>>(), copies);
        assert!(!plan.destination.exists());
    }
}

/// Replaces each of `paths` with a byte-identical copy that has a new inode
/// (a restore from backup, or the files seen through another device).
fn restore_as_copies(paths: &[PathBuf]) {
    let before: Vec<u64> = paths.iter().map(|p| inode(p)).collect();
    for path in paths {
        let copy = path.with_extension("restoring");
        std::fs::copy(path, &copy).unwrap();
        std::fs::rename(&copy, path).unwrap();
    }
    let after: Vec<u64> = paths.iter().map(|p| inode(p)).collect();
    assert!(
        after.iter().zip(&before).all(|(a, b)| a != b),
        "precondition: every file is a copy with a new inode"
    );
}

/// Review (final candidate, should-fix B): a whole set restored as copies at
/// its new place, its old places empty, is not locked by the old journal
/// (whether the copies' contents are right is for the usual checks).
#[test]
fn a_whole_set_restored_as_copies_at_its_new_place_is_not_locked() {
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let (first, ps2) = moved_disc_set_at(&lib, bulk_rename_tags, &lib.root.join("mixed/Saga"));
        let set: Vec<PathBuf> = SET_FILES.iter().map(|f| ps2.join(f)).collect();
        restore_as_copies(&set);
        // Apply and Undo still report it strictly: not the files it moved.
        assert_eq!(
            crate::dat::rename_apply::split_disc_set_reports(&first).len(),
            1
        );
        // But nothing locks the restored set: it moves whole.
        assert!(crate::dat::rename_apply::split_disc_set_paths(&first).is_empty());
        let (_, result) = move_file(&lib, &set[0], "SNES");
        let outcome = result.map_err(|e| e.0.to_string()).unwrap();
        assert_eq!(outcome.summary.applied, 3);
    }
}

/// A restored set with a file also at one of its old places stays locked.
#[test]
fn a_restored_set_with_a_file_at_an_old_place_stays_locked() {
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let old = lib.root.join("mixed/Saga");
        let (_, ps2) = moved_disc_set_at(&lib, bulk_rename_tags, &old);
        let set: Vec<PathBuf> = SET_FILES.iter().map(|f| ps2.join(f)).collect();
        restore_as_copies(&set);
        let at_old = old.join("Disc Game (Track 2).bin");
        std::fs::write(&at_old, b"audio track").unwrap();
        let before: Vec<u64> = set.iter().chain([&at_old]).map(|p| inode(p)).collect();
        let (_, result) = move_file(&lib, &at_old, "SNES");
        let Err((error, _)) = result else {
            panic!("a file at an old place of a set was moved");
        };
        assert!(error.to_string().contains("in two places"), "{error}");
        use crate::dat::rom_organisation::rename_guard;
        assert!(
            rename_guard::refuse_split_disc_set_paths([set[0].as_path()], &lib.journal)
                .unwrap_err()
                .contains("in two places")
        );
        let after: Vec<u64> = set.iter().chain([&at_old]).map(|p| inode(p)).collect();
        assert_eq!(after, before);
    }
}

/// Review (final candidate, should-fix A): a file of a whole set that
/// cannot be read is still where it was recorded; the set is neither
/// reported split nor locked (location is read from metadata only).
#[test]
fn an_unreadable_file_of_a_whole_set_is_not_a_split() {
    use std::os::unix::fs::PermissionsExt;
    for bulk_rename_tags in [true, false] {
        let lib = library(&["mixed/Other.zip"]);
        let (first, ps2) = moved_disc_set_at(&lib, bulk_rename_tags, &lib.root.join("mixed/Saga"));
        let locked_file = ps2.join("Disc Game (Track 1).bin");
        std::fs::set_permissions(&locked_file, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(
            std::fs::File::open(&locked_file).is_err(),
            "precondition: the file cannot be read"
        );
        let reports = crate::dat::rename_apply::split_disc_set_reports(&first);
        let paths = crate::dat::rename_apply::split_disc_set_paths(&first);
        std::fs::set_permissions(&locked_file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(reports.is_empty(), "{reports:?}");
        assert!(paths.is_empty(), "{paths:?}");
    }
}

// --- A later custom rename of the main file (owner approval, option B) ---

#[derive(Clone, Copy, Debug)]
enum Sheet {
    Cue,
    M3u,
    Gdi,
    /// An .m3u whose discs are .cue sheets with their own tracks.
    Nested,
}

const SHEETS: [Sheet; 4] = [Sheet::Cue, Sheet::M3u, Sheet::Gdi, Sheet::Nested];

fn cue_naming(track: &str) -> String {
    format!("FILE \"{track}\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n")
}

impl Sheet {
    /// The sheet's name and text, and the files of its set with their bytes.
    fn files(self) -> (&'static str, String, Vec<(&'static str, Vec<u8>)>) {
        match self {
            Sheet::Cue => (
                "Disc Game.cue",
                "FILE \"Disc Game (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\nFILE \"Disc Game (Track 2).bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n".into(),
                vec![
                    ("Disc Game (Track 1).bin", b"data track".to_vec()),
                    ("Disc Game (Track 2).bin", b"audio track".to_vec()),
                ],
            ),
            Sheet::M3u => (
                "Saga.m3u",
                "Saga (Disc 1).chd\nSaga (Disc 2).chd\n".into(),
                vec![
                    ("Saga (Disc 1).chd", b"disc one".to_vec()),
                    ("Saga (Disc 2).chd", b"disc two".to_vec()),
                ],
            ),
            Sheet::Gdi => (
                "Dream.gdi",
                "3\n1 0 4 2352 track01.bin 0\n2 600 0 2352 track02.raw 0\n3 45000 4 2352 track03.bin 0\n".into(),
                vec![
                    ("track01.bin", vec![1_u8; 2352]),
                    ("track02.raw", vec![2_u8; 2352]),
                    ("track03.bin", vec![3_u8; 2352 * 2]),
                ],
            ),
            Sheet::Nested => (
                "Epic.m3u",
                "Epic (Disc 1).cue\nEpic (Disc 2).cue\n".into(),
                vec![
                    ("Epic (Disc 1).cue", cue_naming("Epic (Disc 1).bin").into_bytes()),
                    ("Epic (Disc 1).bin", b"epic one".to_vec()),
                    ("Epic (Disc 2).cue", cue_naming("Epic (Disc 2).bin").into_bytes()),
                    ("Epic (Disc 2).bin", b"epic two".to_vec()),
                ],
            ),
        }
    }
}

/// A disc game whose sheet is a library row, organised into the PS2 folder
/// by a real Folder platform check Apply (optionally rewritten with the
/// disc-set tag Bulk Rename writes).
struct Organised {
    first: crate::dat::rename_apply::RenameTransaction,
    old: PathBuf,
    new: PathBuf,
    sheet: String,
    members: Vec<(&'static str, Vec<u8>)>,
    id: i64,
}

fn organised_game(lib: &mut Library, sheet: Sheet, bulk_rename_tags: bool) -> Organised {
    use crate::dat::rename_apply::model::TransactionOperation;
    let old = lib.root.join("mixed/Disc");
    std::fs::create_dir_all(&old).unwrap();
    let (name, text, members) = sheet.files();
    std::fs::write(old.join(name), &text).unwrap();
    for (member, bytes) in &members {
        std::fs::write(old.join(member), bytes).unwrap();
    }
    catalogue_as(
        lib,
        &lib.root.join("mixed/Placeholder.zip"),
        &old.join(name),
    );
    let id = lib
        .db
        .find_archive_id_by_absolute_path(&old.join(name))
        .unwrap()
        .unwrap();
    // The game has a system, as a scanned game does.
    lib.db.set_manual_platform(id, "PS2").unwrap();
    let chosen = [ChosenGame {
        archive_id: id,
        path: old.join(name),
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
    .unwrap_or_else(|e| panic!("{sheet:?}: {}", e.0));
    assert_eq!(outcome.summary.applied, 1 + members.len(), "{sheet:?}");
    let mut first = outcome.transaction;
    if bulk_rename_tags {
        let lead = old.join(name).to_string_lossy().into_owned();
        for entry in &mut first.entries {
            if matches!(entry.operation, TransactionOperation::RenameMove)
                && entry.source_path.starts_with(&old)
            {
                entry.unknown.insert(
                    crate::database::organisation_paths::DISC_SET_KEY.into(),
                    serde_json::json!({ "lead": lead }),
                );
            }
        }
        crate::dat::rename_apply::journal::write_journal(&lib.journal, &first).unwrap();
    }
    let new = lib
        .root
        .join(crate::platform::canonical_layout_folder("PS2").unwrap());
    Organised {
        first,
        old,
        new,
        sheet: name.into(),
        members,
        id,
    }
}

/// The real "Rename this game's file", from the game's current path.
fn rename_game(
    lib: &Library,
    path: &Path,
    typed: &str,
) -> crate::dat::rename_apply::RenameTransaction {
    use crate::dat::rom_organisation::game_rename::{
        RetroArchSaveLocations, apply_game_rename, preview_game_rename,
    };
    let none = RetroArchSaveLocations::default();
    let preview = preview_game_rename(&lib.db, path, typed, 1, &none)
        .unwrap_or_else(|e| panic!("preview of {}: {e}", path.display()));
    apply_game_rename(
        &preview,
        &lib.db,
        &lib.journal,
        &AtomicBool::new(false),
        &none,
    )
    .unwrap_or_else(|e| panic!("rename of {}: {e}", path.display()))
    .transaction
}

/// The movement lock's refusal of `path` for a split set, if any (other
/// refusals fail the test).
fn lock_message(lib: &Library, path: &Path) -> Option<String> {
    use crate::dat::rom_organisation::rename_guard;
    match rename_guard::refuse_split_disc_set_paths([path], &lib.journal) {
        Ok(()) => None,
        Err(e) if e.contains("in two places") => Some(e),
        Err(e) => panic!("unexpected refusal: {e}"),
    }
}

fn is_locked(lib: &Library, path: &Path) -> bool {
    lock_message(lib, path).is_some()
}

fn renamed(o: &Organised, stem: &str) -> PathBuf {
    let extension = Path::new(&o.sheet)
        .extension()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    o.new.join(format!("{stem}.{extension}"))
}

/// Every file of the set is in `dir` with its bytes, and each is named by
/// the sheet or by another file of the set (a nested .cue names its track).
fn assert_whole_in(dir: &Path, sheet_name: &std::ffi::OsStr, o: &Organised) {
    let sheet_text = std::fs::read_to_string(dir.join(sheet_name)).unwrap();
    for (member, bytes) in &o.members {
        assert_eq!(&std::fs::read(dir.join(member)).unwrap(), bytes, "{member}");
        let named = sheet_text.contains(member)
            || o.members.iter().any(|(other, _)| {
                other != member
                    && String::from_utf8_lossy(&std::fs::read(dir.join(other)).unwrap())
                        .contains(member)
            });
        assert!(named, "{member} is still named by its sheet");
    }
}

/// Later History moves, recorded as one real RenameMove record (valid
/// records of moves that do not keep the set together), with the files
/// moved. `id` and `at` pin the record's id and timestamps.
fn record_later_moves(
    lib: &Library,
    template: &crate::dat::rename_apply::RenameTransaction,
    moves: &[(&Path, &Path)],
    id: Option<String>,
    at: Option<u64>,
) -> crate::dat::rename_apply::RenameTransaction {
    use crate::dat::rename_apply::model::{EntryState, TransactionOperation};
    use crate::dat::rename_apply::{capture_identity, journal};
    let now = at.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    });
    let mut tx = template.clone();
    tx.transaction_id = id.unwrap_or_else(|| journal::new_transaction_id(now));
    tx.created_at_unix = now;
    let base = template
        .entries
        .iter()
        .find(|e| matches!(e.operation, TransactionOperation::RenameMove))
        .unwrap()
        .clone();
    tx.entries = moves
        .iter()
        .map(|(from, to)| {
            let mut entry = base.clone();
            entry.source_path = from.to_path_buf();
            entry.destination_path = to.to_path_buf();
            entry.identity = capture_identity(from).unwrap();
            entry.unknown.clear();
            entry.state = EntryState::Applied;
            entry.applied_at_unix = Some(now);
            entry.rolled_back_at_unix = None;
            entry
        })
        .collect();
    for (from, to) in moves {
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(from, to).unwrap();
    }
    journal::write_journal(&lib.journal, &tx).unwrap();
    tx
}

fn record_later_move(
    lib: &Library,
    template: &crate::dat::rename_apply::RenameTransaction,
    from: &Path,
    to: &Path,
) -> crate::dat::rename_apply::RenameTransaction {
    record_later_moves(lib, template, &[(from, to)], None, None)
}

fn undo_rename(lib: &Library, tx: &mut crate::dat::rename_apply::RenameTransaction) {
    let undo =
        rollback_organisation_transaction(tx, &lib.journal, &AtomicBool::new(false), &lib.root)
            .unwrap();
    assert_eq!(
        undo.rollback.result,
        crate::dat::rename_apply::RollbackResult::FullyRolledBack
    );
}

/// The original defect, then the fix: a custom rename of the main file
/// after Folder platform check (or Bulk Rename) organised the game no longer
/// locks the game, and the renamed game moves whole, with its references
/// and its library record intact.
#[test]
fn a_custom_rename_of_the_main_file_does_not_lock_the_game() {
    for sheet in SHEETS {
        for bulk_rename_tags in [true, false] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let o = organised_game(&mut lib, sheet, bulk_rename_tags);
            rename_game(&lib, &o.new.join(&o.sheet), "My Custom Name");
            let sheet_now = renamed(&o, "My Custom Name");
            assert!(sheet_now.exists());
            for path in std::iter::once(sheet_now.clone())
                .chain(o.members.iter().map(|(m, _)| o.new.join(m)))
            {
                assert!(
                    !is_locked(&lib, &path),
                    "{sheet:?} tags={bulk_rename_tags}: {} is locked",
                    path.display()
                );
            }
            // The Bulk Rename / Quick Rename lock agrees.
            let mut plan = plan_organisation_with_choices(
                &[],
                &[ChosenGame {
                    archive_id: o.id,
                    path: sheet_now.clone(),
                    platform: "SNES".into(),
                }],
                &[],
                std::slice::from_ref(&lib.root),
                1,
            )
            .remove(0);
            assert_eq!(
                crate::dat::rom_organisation::rename_guard::block_split_disc_sets(
                    &mut plan.plan,
                    &lib.journal
                ),
                0
            );
            // The renamed game moves whole to another system folder.
            let (_, moved) = move_file(&lib, &sheet_now, "SNES");
            let moved = moved.unwrap_or_else(|e| panic!("{sheet:?}: {}", e.0));
            assert_eq!(moved.summary.applied, 1 + o.members.len(), "{sheet:?}");
            let snes = lib
                .root
                .join(crate::platform::canonical_layout_folder("SNES").unwrap());
            let sheet_there = snes.join(sheet_now.file_name().unwrap());
            assert_whole_in(&snes, sheet_now.file_name().unwrap(), &o);
            assert_eq!(
                lib.db
                    .find_archive_id_by_absolute_path(&sheet_there)
                    .unwrap(),
                Some(o.id),
                "{sheet:?}: the library record follows the game"
            );
        }
    }
}

/// Several consecutive custom renames are followed through History, and
/// the game still moves whole afterwards.
#[test]
fn several_consecutive_custom_renames_do_not_lock_the_game() {
    for sheet in SHEETS {
        for bulk_rename_tags in [true, false] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let o = organised_game(&mut lib, sheet, bulk_rename_tags);
            let mut at = o.new.join(&o.sheet);
            for stem in ["Name One", "Name Two", "Name Three"] {
                rename_game(&lib, &at, stem);
                at = renamed(&o, stem);
            }
            assert!(!is_locked(&lib, &at), "{sheet:?}");
            for (member, _) in &o.members {
                assert!(!is_locked(&lib, &o.new.join(member)), "{sheet:?} {member}");
            }
            let (_, moved) = move_file(&lib, &at, "SNES");
            assert_eq!(
                moved
                    .unwrap_or_else(|e| panic!("{sheet:?}: {}", e.0))
                    .summary
                    .applied,
                1 + o.members.len()
            );
        }
    }
}

/// B1: a file of the set moved on its own by a later, valid History record
/// (after the main file was renamed several times) is a genuine split: the
/// set stays locked, the moved file's current place too, and following the
/// message (Undo that record) makes it whole.
#[test]
fn a_file_moved_on_its_own_after_custom_renames_keeps_the_game_locked() {
    for sheet in SHEETS {
        for bulk_rename_tags in [true, false] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let o = organised_game(&mut lib, sheet, bulk_rename_tags);
            let mut at = o.new.join(&o.sheet);
            let mut last = None;
            for stem in ["Name One", "Name Two"] {
                last = Some(rename_game(&lib, &at, stem));
                at = renamed(&o, stem);
            }
            let (member, bytes) = &o.members[0];
            let away = lib.root.join("Elsewhere").join(member);
            // The later record also moves another game's file.
            let other = lib.root.join("Another Game.iso");
            std::fs::write(&other, b"another game").unwrap();
            let other_away = lib.root.join("Elsewhere/Another Game.iso");
            let mut lone = record_later_moves(
                &lib,
                last.as_ref().unwrap(),
                &[(&o.new.join(member), &away), (&other, &other_away)],
                None,
                None,
            );
            let message = lock_message(&lib, &o.new.join(o.members[1].0))
                .unwrap_or_else(|| panic!("{sheet:?}: a genuinely split set was released"));
            assert!(is_locked(&lib, &away), "{sheet:?}: the moved file's place");
            assert!(is_locked(&lib, &at), "{sheet:?}: the renamed main file");
            assert!(message.contains(&o.first.transaction_id), "{message}");
            assert!(
                message.contains(&format!(
                    "Undo History operation {} in History (a later operation, listed in History as \"Moved 2 files\"). This Undo reverses that whole operation, not just this game: it puts back all 2 files that operation changed, 1 of them not part of this disc set.",
                    lone.transaction_id
                )),
                "{message}"
            );
            assert!(
                !message.contains(&format!(
                    "Undo History operation {}",
                    o.first.transaction_id
                )),
                "{message}"
            );
            // Following it: Undo of that record puts the file back, the
            // renamed game is whole again and moves whole.
            let undo = crate::dat::rename_apply::rollback_transaction(
                &mut lone,
                &lib.journal,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(
                undo.result,
                crate::dat::rename_apply::RollbackResult::FullyRolledBack
            );
            assert_eq!(&std::fs::read(o.new.join(member)).unwrap(), bytes);
            // As disclosed, the whole operation was reversed.
            assert!(other.exists() && !other_away.exists());
            assert!(!is_locked(&lib, &at), "{sheet:?}");
            let (_, moved) = move_file(&lib, &at, "SNES");
            assert!(moved.is_ok(), "{sheet:?}");
        }
    }
}

/// B1: the main file moved to another folder by a later record is not a
/// rename that keeps the set together.
#[test]
fn a_main_file_moved_to_another_folder_keeps_the_game_locked() {
    for sheet in SHEETS {
        let mut lib = library(&["mixed/Placeholder.zip"]);
        let o = organised_game(&mut lib, sheet, false);
        let first_rename = rename_game(&lib, &o.new.join(&o.sheet), "Custom");
        let at = renamed(&o, "Custom");
        let away = lib.root.join("Elsewhere").join(at.file_name().unwrap());
        record_later_move(&lib, &first_rename, &at, &away);
        assert!(is_locked(&lib, &o.new.join(o.members[0].0)), "{sheet:?}");
        assert!(is_locked(&lib, &away), "{sheet:?}");
    }
}

/// A genuine split after a custom rename (a file moved back by hand): the
/// set stays locked, with its renamed main file; the message offers the
/// one step that works, and following it makes the set whole.
#[test]
fn a_genuine_split_after_a_custom_rename_is_locked_and_the_offered_step_works() {
    for sheet in SHEETS {
        for bulk_rename_tags in [true, false] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let o = organised_game(&mut lib, sheet, bulk_rename_tags);
            rename_game(&lib, &o.new.join(&o.sheet), "Custom");
            let at = renamed(&o, "Custom");
            let (stray, _) = &o.members[0];
            std::fs::rename(o.new.join(stray), o.old.join(stray)).unwrap();
            let before = inode(&o.old.join(stray));
            assert!(is_locked(&lib, &at), "{sheet:?}: the renamed main file");
            let (_, result) = move_file(&lib, &o.old.join(stray), "SNES");
            assert!(
                result.is_err(),
                "{sheet:?}: a file of a genuinely split set moved"
            );
            let error = lock_message(&lib, &o.old.join(stray)).unwrap();
            assert!(
                error.contains(&format!("History operation {}", o.first.transaction_id)),
                "{error}"
            );
            let step = format!(
                "Move {stray} yourself from {} to {}.",
                o.old.join(stray).display(),
                o.new.join(stray).display()
            );
            assert!(error.contains(&step), "{error}");
            assert!(error.contains("whole at its new place"), "{error}");
            assert_eq!(inode(&o.old.join(stray)), before);
            // Following the message.
            std::fs::rename(o.old.join(stray), o.new.join(stray)).unwrap();
            assert!(!is_locked(&lib, &at), "{sheet:?}");
            let (_, moved) = move_file(&lib, &at, "SNES");
            assert!(moved.is_ok(), "{sheet:?}");
        }
    }
}

/// S1/S2: an interrupted Undo (one file already back, its record says so)
/// after two custom renames recorded in the same second: the message lists
/// the renames newest first by the order History chains them, then the
/// Undo of the organisation; following it exactly ends whole at the old
/// place.
#[test]
fn the_offered_undo_order_works_even_with_identical_timestamps() {
    use crate::dat::rename_apply::journal::write_journal;
    use crate::dat::rename_apply::model::{EntryState, TransactionState};
    for sheet in SHEETS {
        for bulk_rename_tags in [true, false] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let mut o = organised_game(&mut lib, sheet, bulk_rename_tags);
            let mut second = rename_game(&lib, &o.new.join(&o.sheet), "Name One");
            let mut third = rename_game(&lib, &renamed(&o, "Name One"), "Name Two");
            let same = third.created_at_unix;
            for tx in [&mut second, &mut third] {
                tx.created_at_unix = same;
                for entry in &mut tx.entries {
                    if entry.applied_at_unix.is_some() {
                        entry.applied_at_unix = Some(same);
                    }
                }
                write_journal(&lib.journal, tx).unwrap();
            }
            let (back, _) = o.members[0];
            std::fs::rename(o.new.join(back), o.old.join(back)).unwrap();
            for entry in &mut o.first.entries {
                if entry.source_path == o.old.join(back) {
                    entry.state = EntryState::RolledBack;
                    entry.rolled_back_at_unix = Some(same);
                }
            }
            o.first.state = TransactionState::RollbackFailed;
            write_journal(&lib.journal, &o.first).unwrap();
            let message = lock_message(&lib, &o.new.join(o.members[1].0)).unwrap();
            let step =
                |n: usize, id: &str| format!("Step {n}: Undo History operation {id} in History");
            assert!(
                message.contains(&step(1, &third.transaction_id)),
                "{sheet:?}: {message}"
            );
            assert!(
                message.contains(&step(2, &second.transaction_id)),
                "{message}"
            );
            assert!(
                message.contains(&step(3, &o.first.transaction_id)),
                "{message}"
            );
            assert!(
                message.contains("This Undo reverses that whole operation, not just this game"),
                "{message}"
            );
            assert!(message.contains("not guaranteed to succeed"), "{message}");
            assert!(message.contains("whole at its old place"), "{message}");
            undo_rename(&lib, &mut third);
            undo_rename(&lib, &mut second);
            undo_rename(&lib, &mut o.first);
            assert!(o.old.join(&o.sheet).exists(), "{sheet:?}");
            for (member, bytes) in &o.members {
                assert_eq!(&std::fs::read(o.old.join(member)).unwrap(), bytes);
            }
            assert!(!is_locked(&lib, &o.old.join(&o.sheet)), "{sheet:?}");
        }
    }
}

/// Undo after several renames, newest first, then of the organisation:
/// each step leaves the game unlocked and the last puts it back whole.
#[test]
fn undo_after_several_custom_renames_restores_the_game_whole() {
    for sheet in SHEETS {
        for bulk_rename_tags in [true, false] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let mut o = organised_game(&mut lib, sheet, bulk_rename_tags);
            let mut renames = Vec::new();
            let mut at = o.new.join(&o.sheet);
            for stem in ["Name One", "Name Two"] {
                renames.push(rename_game(&lib, &at, stem));
                at = renamed(&o, stem);
            }
            for mut tx in renames.into_iter().rev() {
                undo_rename(&lib, &mut tx);
                assert!(!is_locked(&lib, &o.new.join(o.members[0].0)), "{sheet:?}");
            }
            assert!(o.new.join(&o.sheet).exists());
            undo_rename(&lib, &mut o.first);
            assert!(o.old.join(&o.sheet).exists());
            for (member, bytes) in &o.members {
                assert_eq!(&std::fs::read(o.old.join(member)).unwrap(), bytes);
            }
        }
    }
}

/// B2: a recovery destination occupied by an unrelated file. Nothing
/// offers to overwrite it or an Undo that would refuse; the file is left
/// alone; once it is moved away, the steps are offered.
#[test]
fn an_occupied_recovery_destination_is_never_overwritten_or_offered() {
    for sheet in SHEETS {
        let mut lib = library(&["mixed/Placeholder.zip"]);
        let o = organised_game(&mut lib, sheet, true);
        rename_game(&lib, &o.new.join(&o.sheet), "Custom");
        let (stray, _) = &o.members[0];
        std::fs::rename(o.new.join(stray), o.old.join(stray)).unwrap();
        std::fs::write(o.new.join(stray), b"someone else's file").unwrap();
        let message = lock_message(&lib, &o.old.join(stray)).unwrap();
        assert!(
            message.contains(&format!(
                "{} is occupied by a different file, and nothing will overwrite it",
                o.new.join(stray).display()
            )),
            "{message}"
        );
        assert!(!message.contains("Undo History operation"), "{message}");
        assert!(!message.contains("Undo the later"), "{message}");
        assert_eq!(
            std::fs::read(o.new.join(stray)).unwrap(),
            b"someone else's file"
        );
        std::fs::rename(o.new.join(stray), lib.root.join("kept-aside")).unwrap();
        let message = lock_message(&lib, &o.old.join(stray)).unwrap();
        assert!(message.contains("yourself from"), "{message}");
    }
}

/// B2: a replaced file (another file at its recorded place) is named as
/// replaced; no Undo is suggested, since Undo would refuse it.
#[test]
fn a_replaced_file_is_named_and_no_undo_is_suggested() {
    for sheet in SHEETS {
        for bulk_rename_tags in [true, false] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let o = organised_game(&mut lib, sheet, bulk_rename_tags);
            rename_game(&lib, &o.new.join(&o.sheet), "Custom");
            let replaced = o.new.join(o.members[0].0);
            let copy = replaced.with_extension("replacing");
            std::fs::write(&copy, b"an unrelated file").unwrap();
            std::fs::rename(&copy, &replaced).unwrap();
            let (other, _) = &o.members[1];
            std::fs::rename(o.new.join(other), o.old.join(other)).unwrap();
            let message = lock_message(&lib, &replaced).unwrap();
            assert!(message.contains("has been replaced"), "{message}");
            assert!(message.contains("no Undo is suggested"), "{message}");
            assert!(!message.contains("Undo History operation"), "{message}");
            assert!(!message.contains("put it at"), "{message}");
        }
    }
}

/// B2: a missing file: no Undo is suggested; a copy put back where History
/// last put it releases the lock by place, and the message says a copy is
/// not the original file. Following it does release the lock.
#[test]
fn a_missing_file_offers_a_copy_by_place_and_following_it_releases_the_lock() {
    for sheet in SHEETS {
        for bulk_rename_tags in [true, false] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let o = organised_game(&mut lib, sheet, bulk_rename_tags);
            rename_game(&lib, &o.new.join(&o.sheet), "Custom");
            let (gone, bytes) = &o.members[0];
            std::fs::remove_file(o.new.join(gone)).unwrap();
            let message = lock_message(&lib, &o.new.join(o.members[1].0)).unwrap();
            assert!(message.contains("is missing"), "{message}");
            assert!(
                message.contains(&format!("put it at {}", o.new.join(gone).display())),
                "{message}"
            );
            assert!(
                message.contains("does not restore the original file's identity"),
                "{message}"
            );
            assert!(!message.contains("Undo History operation"), "{message}");
            std::fs::write(o.new.join(gone), bytes).unwrap();
            assert!(!is_locked(&lib, &o.new.join(o.members[1].0)), "{sheet:?}");
        }
    }
}

/// Missing, ambiguous or inconsistent History never counts as evidence, and
/// the message says which.
#[test]
fn missing_ambiguous_or_inconsistent_history_keeps_the_game_locked() {
    use crate::dat::rename_apply::{journal::write_journal, journal_path};
    for bulk_rename_tags in [true, false] {
        for (case, says) in [
            ("missing", "is missing"),
            ("ambiguous", "more than one History operation"),
            ("changed", "not this file as History recorded it"),
            ("undone", "was undone, but it is not back"),
        ] {
            let mut lib = library(&["mixed/Placeholder.zip"]);
            let o = organised_game(&mut lib, Sheet::Cue, bulk_rename_tags);
            let later = rename_game(&lib, &o.new.join(&o.sheet), "Custom");
            let path = journal_path(&lib.journal, &later.transaction_id).unwrap();
            match case {
                // The later rename's record is gone.
                "missing" => std::fs::remove_file(&path).unwrap(),
                // A second record, same timestamps, claims a move of the
                // same file from the same place.
                "ambiguous" => {
                    let mut twin = later.clone();
                    twin.transaction_id = crate::dat::rename_apply::journal::new_transaction_id(
                        later.created_at_unix,
                    );
                    twin.entries[0].destination_path = o.new.join("Elsewhere.cue");
                    write_journal(&lib.journal, &twin).unwrap();
                }
                // The record says the file was different (size) when it moved.
                "changed" => {
                    let mut changed = later.clone();
                    changed.entries[0].identity.size_bytes += 1;
                    write_journal(&lib.journal, &changed).unwrap();
                }
                // The record says the rename was undone, but the file is not back.
                "undone" => {
                    let mut undone = later.clone();
                    undone.state = crate::dat::rename_apply::model::TransactionState::RolledBack;
                    write_journal(&lib.journal, &undone).unwrap();
                }
                _ => unreachable!(),
            }
            let message = lock_message(&lib, &o.new.join(o.members[0].0))
                .unwrap_or_else(|| panic!("{case}: the game must stay locked"));
            assert!(message.contains(says), "{case}: {message}");
            assert!(
                !message.contains("Undo History operation"),
                "{case}: {message}"
            );
        }
    }
}

/// Whole-set relocation without any rename keeps working.
#[test]
fn a_whole_set_relocation_keeps_working_without_any_rename() {
    for sheet in SHEETS {
        let mut lib = library(&["mixed/Placeholder.zip"]);
        let o = organised_game(&mut lib, sheet, false);
        let (_, moved) = move_file(&lib, &o.new.join(&o.sheet), "SNES");
        assert_eq!(
            moved
                .unwrap_or_else(|e| panic!("{sheet:?}: {}", e.0))
                .summary
                .applied,
            1 + o.members.len()
        );
        let snes = lib
            .root
            .join(crate::platform::canonical_layout_folder("SNES").unwrap());
        assert_whole_in(&snes, std::ffi::OsStr::new(&o.sheet), &o);
    }
}

/// F5: the order comes from how History chains the moves, not from ids or
/// timestamps: two renames recorded in the same second, the newer one with
/// the id that sorts first as text and as a number.
#[test]
fn the_undo_order_follows_the_history_chain_not_ids() {
    use crate::dat::rename_apply::journal::write_journal;
    use crate::dat::rename_apply::model::{EntryState, TransactionState};
    for bulk_rename_tags in [true, false] {
        let mut lib = library(&["mixed/Placeholder.zip"]);
        let mut o = organised_game(&mut lib, Sheet::Cue, bulk_rename_tags);
        let same = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let one = o.new.join("Name One.cue");
        let two = o.new.join("Name Two.cue");
        let mut older = record_later_moves(
            &lib,
            &o.first,
            &[(&o.new.join(&o.sheet), &one)],
            Some(format!("{same}-9001")),
            Some(same),
        );
        let mut newer = record_later_moves(
            &lib,
            &o.first,
            &[(&one, &two)],
            Some(format!("{same}-9000")),
            Some(same),
        );
        let (back, _) = o.members[0];
        std::fs::rename(o.new.join(back), o.old.join(back)).unwrap();
        for entry in &mut o.first.entries {
            if entry.source_path == o.old.join(back) {
                entry.state = EntryState::RolledBack;
                entry.rolled_back_at_unix = Some(same);
            }
        }
        o.first.state = TransactionState::RollbackFailed;
        write_journal(&lib.journal, &o.first).unwrap();
        let message = lock_message(&lib, &two).unwrap();
        assert!(
            message.contains(&format!(
                "Step 1: Undo History operation {}",
                newer.transaction_id
            )),
            "{message}"
        );
        assert!(
            message.contains(&format!(
                "Step 2: Undo History operation {}",
                older.transaction_id
            )),
            "{message}"
        );
        for tx in [&mut newer, &mut older] {
            let undo = crate::dat::rename_apply::rollback_transaction(
                tx,
                &lib.journal,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(
                undo.result,
                crate::dat::rename_apply::RollbackResult::FullyRolledBack
            );
        }
        undo_rename(&lib, &mut o.first);
        assert!(o.old.join(&o.sheet).exists());
        for (member, bytes) in &o.members {
            assert_eq!(&std::fs::read(o.old.join(member)).unwrap(), bytes);
        }
    }
}

/// F1: a file whose earlier Undo failed, or whose Apply or Undo was
/// interrupted, is never planned as if History had not moved it: no step
/// and no Undo is offered (whether Undo would finish it cannot be worked
/// out reliably here), wherever the file is.
#[test]
fn an_unfinished_step_of_a_file_in_place_offers_no_undo() {
    use crate::dat::rename_apply::journal::write_journal;
    use crate::dat::rename_apply::model::{EntryState, TransactionState};
    for bulk_rename_tags in [true, false] {
        for (state, says) in [
            (
                EntryState::RollbackFailed,
                "an earlier Undo of Disc Game (Track 1).bin did not finish, so whether Undo would finish it now cannot be worked out reliably here",
            ),
            (
                EntryState::RollingBack,
                "an Undo of Disc Game (Track 1).bin was interrupted, so whether Undo would finish it now cannot be worked out reliably here",
            ),
            (
                EntryState::Applying,
                "the move of Disc Game (Track 1).bin was interrupted while it was being applied",
            ),
        ] {
            // The file still at its new place, or already back at its old one.
            for file_back in [false, true] {
                let mut lib = library(&["mixed/Placeholder.zip"]);
                let mut o = organised_game(&mut lib, Sheet::Cue, bulk_rename_tags);
                let (unfinished, _) = o.members[0];
                let (other, _) = o.members[1];
                if file_back {
                    std::fs::rename(o.new.join(unfinished), o.old.join(unfinished)).unwrap();
                } else {
                    // Split another way: the other track moved back by hand.
                    std::fs::rename(o.new.join(other), o.old.join(other)).unwrap();
                }
                for entry in &mut o.first.entries {
                    if entry.source_path == o.old.join(unfinished) {
                        entry.state = state;
                    }
                }
                o.first.state = TransactionState::RollbackFailed;
                write_journal(&lib.journal, &o.first).unwrap();
                let message = lock_message(&lib, &o.new.join(&o.sheet))
                    .unwrap_or_else(|| panic!("{state:?} back={file_back}: not locked"));
                assert!(message.contains(says), "{message}");
                assert!(message.contains("no Undo is suggested"), "{message}");
                assert!(!message.contains("Undo History operation"), "{message}");
                assert!(!message.contains("yourself from"), "{message}");
            }
        }
    }
}

/// F2: advice never includes an Undo that would also move other files of
/// the set (here a later whole-set move back to the old folder, after a
/// custom rename). F3 (documented limit): such a set stays locked with no
/// steps offered.
#[test]
fn advice_never_undoes_an_operation_that_also_moves_other_set_files() {
    for bulk_rename_tags in [true, false] {
        let mut lib = library(&["mixed/Placeholder.zip"]);
        let o = organised_game(&mut lib, Sheet::Cue, bulk_rename_tags);
        let rename = rename_game(&lib, &o.new.join(&o.sheet), "Custom");
        let lead_now = renamed(&o, "Custom");
        let lead_back = o.old.join("Custom.cue");
        let moves: Vec<(PathBuf, PathBuf)> = std::iter::once((lead_now.clone(), lead_back.clone()))
            .chain(
                o.members
                    .iter()
                    .map(|(m, _)| (o.new.join(m), o.old.join(m))),
            )
            .collect();
        let refs: Vec<(&Path, &Path)> = moves
            .iter()
            .map(|(a, b)| (a.as_path(), b.as_path()))
            .collect();
        record_later_moves(&lib, &rename, &refs, None, None);
        let message = lock_message(&lib, &o.old.join(o.members[0].0))
            .expect("F3 limit: the set moved back as a whole after a rename stays locked");
        assert!(
            message.contains("would also move other files of this disc set"),
            "{message}"
        );
        assert!(!message.contains("Undo History operation"), "{message}");
        assert!(message.contains("no Undo is suggested"), "{message}");
    }
}

/// Batch disclosure for the operation that left the set split: it moved
/// another game too; the message says Undo reverses all of it, and how
/// many files are not of this set; following it does exactly that. When
/// History cannot tell (an unfinished step of another file), it says so.
#[test]
fn the_undo_of_the_splitting_operation_discloses_its_whole_batch() {
    use crate::dat::rename_apply::journal::write_journal;
    use crate::dat::rename_apply::model::{EntryState, TransactionState};
    for bulk_rename_tags in [true, false] {
        // Some(true): counts known; Some(false): another file's Undo did not
        // finish; None: another file was moved by hand since.
        for reliable in [Some(true), Some(false), None] {
            let lib = library(&["mixed/Other.zip"]);
            let old = lib.root.join("mixed/Saga");
            let (mut tx, new) = moved_disc_set_at(&lib, bulk_rename_tags, &old);
            let back = "Disc Game (Track 2).bin";
            std::fs::rename(new.join(back), old.join(back)).unwrap();
            for entry in &mut tx.entries {
                if entry.source_path == old.join(back) {
                    entry.state = EntryState::RolledBack;
                    entry.rolled_back_at_unix = entry.applied_at_unix;
                }
                if reliable == Some(false) && entry.source_path.ends_with("Unrelated.iso") {
                    entry.state = EntryState::RollbackFailed;
                }
            }
            tx.state = TransactionState::RollbackFailed;
            write_journal(&lib.journal, &tx).unwrap();
            if reliable.is_none() {
                let iso = tx
                    .entries
                    .iter()
                    .find(|entry| entry.source_path.ends_with("Unrelated.iso"))
                    .unwrap()
                    .destination_path
                    .clone();
                std::fs::rename(&iso, lib.root.join("moved-by-hand.iso")).unwrap();
            }
            let message = lock_message(&lib, &new.join("Disc Game.cue")).unwrap();
            assert!(
                message.contains("This Undo reverses that whole operation, not just this game:"),
                "{message}"
            );
            assert!(
                message.contains(&format!(
                    "Step 1: Undo History operation {} in History (the operation that left the set split, listed in History as \"Moved 4 files\"",
                    tx.transaction_id
                )),
                "{message}"
            );
            if reliable == Some(true) {
                assert!(
                    message.contains("it puts back all 3 files that operation changed, 1 of them not part of this disc set."),
                    "{message}"
                );
                undo_rename(&lib, &mut tx);
                assert!(
                    lib.root.join("mixed/Unrelated.iso").exists(),
                    "as disclosed"
                );
                for file in SET_FILES {
                    assert!(old.join(file).exists(), "{file}");
                }
            } else {
                assert!(
                    message.contains("cannot be worked out reliably from History"),
                    "{message}"
                );
            }
        }
    }
}
