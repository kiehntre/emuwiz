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
        let a = crate::Archive::from_path_in_root(&root.join(f), &root).unwrap();
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
            .any(|(p, why)| p == &path && why.contains("registered source folder"))
    );
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
