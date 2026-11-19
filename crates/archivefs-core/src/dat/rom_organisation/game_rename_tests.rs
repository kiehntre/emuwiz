//! One-game typed rename: real files and a disposable library database.
use super::game_rename::*;
use super::rollback_organisation_transaction;
use crate::dat::rename_apply::{RollbackResult, TransactionState};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

struct Lib {
    temp: tempfile::TempDir,
    db: crate::Database,
    root: PathBuf,
    game: PathBuf,
    id: i64,
}

impl Lib {
    fn new(file: &str, platform: Option<&str>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        std::fs::create_dir_all(root.join("nes")).unwrap();
        let game = root.join("nes").join(file);
        std::fs::write(&game, b"NES\x1a synthetic rom").unwrap();
        let mut db = crate::Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
        db.register_source_folders(std::slice::from_ref(&root))
            .unwrap();
        let folder = db.list_source_folders().unwrap()[0].id;
        let archive = crate::Archive::from_path_in_root(&game, &root).unwrap();
        let id = db
            .upsert_archive(folder, &root, &archive)
            .unwrap()
            .archive_id;
        if let Some(platform) = platform {
            db.set_manual_platform(id, platform).unwrap();
        }
        Self {
            temp,
            db,
            root,
            game,
            id,
        }
    }
    fn journal(&self) -> PathBuf {
        self.temp.path().join("journal")
    }
    fn id_at(&self, path: &Path) -> Option<i64> {
        self.db.find_archive_id_by_absolute_path(path).unwrap()
    }
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn typed_name_keeps_the_extension() {
    let source = Path::new("/g/nes/Old.nes");
    assert_eq!(
        normalise_new_file_name(source, " Super Game ").unwrap(),
        "Super Game.nes"
    );
    assert_eq!(
        normalise_new_file_name(source, "Super Game.NES").unwrap(),
        "Super Game.nes"
    );
    assert_eq!(
        normalise_new_file_name(source, "Super Game (USA, Rev. 1)").unwrap(),
        "Super Game (USA, Rev. 1).nes"
    );
    assert!(
        normalise_new_file_name(source, "Game.zip")
            .unwrap_err()
            .contains(".nes")
    );
    for bad in ["", "  ", "a/b", "a\\b", ".hidden", "..", "bad\nname"] {
        assert!(normalise_new_file_name(source, bad).is_err(), "{bad:?}");
    }
    assert!(normalise_new_file_name(source, &"x".repeat(300)).is_err());
}

#[test]
fn rename_applies_rebinds_the_same_game_and_undoes() {
    let lib = Lib::new("Old Name.nes", Some("nes"));
    let preview = preview_game_rename(&lib.db, &lib.game, "New Name (USA)", 1).unwrap();
    let wanted = lib.root.join("nes").join("New Name (USA).nes");
    assert_eq!(preview.destination, wanted);
    assert!(lib.game.exists(), "a preview changes nothing");
    let outcome = apply_game_rename(&preview, &lib.db, &lib.journal(), &no_cancel()).unwrap();
    assert_eq!(outcome.transaction.state, TransactionState::Applied);
    assert!(!lib.game.exists());
    assert_eq!(std::fs::read(&wanted).unwrap(), b"NES\x1a synthetic rom");
    assert_eq!(
        lib.id_at(&wanted),
        Some(lib.id),
        "same library entry, new path"
    );
    assert_eq!(lib.id_at(&lib.game), None);

    let mut transaction = outcome.transaction;
    let undo = rollback_organisation_transaction(
        &mut transaction,
        &lib.journal(),
        &no_cancel(),
        &preview.source_root,
    )
    .unwrap();
    assert_eq!(undo.rollback.result, RollbackResult::FullyRolledBack);
    assert!(lib.game.exists());
    assert!(!wanted.exists());
    assert_eq!(lib.id_at(&lib.game), Some(lib.id));
}

#[test]
fn existing_destination_is_refused_at_preview_and_at_apply() {
    let lib = Lib::new("Old.nes", Some("nes"));
    let taken = lib.root.join("nes").join("Taken.nes");
    std::fs::write(&taken, b"someone else").unwrap();
    assert!(preview_game_rename(&lib.db, &lib.game, "Taken", 1).is_err());
    // Case-only clash on a case-insensitive name is also a clash.
    assert!(preview_game_rename(&lib.db, &lib.game, "taken", 1).is_err());

    // Created after the preview: apply refuses and nothing moves.
    let preview = preview_game_rename(&lib.db, &lib.game, "Later", 1).unwrap();
    let later = lib.root.join("nes").join("Later.nes");
    std::fs::write(&later, b"arrived late").unwrap();
    assert!(apply_game_rename(&preview, &lib.db, &lib.journal(), &no_cancel()).is_err());
    assert!(lib.game.exists());
    assert_eq!(std::fs::read(&later).unwrap(), b"arrived late");
    assert_eq!(std::fs::read(&taken).unwrap(), b"someone else");
    assert_eq!(lib.id_at(&lib.game), Some(lib.id));
}

#[test]
fn a_file_changed_after_preview_is_not_renamed() {
    let lib = Lib::new("Old.nes", Some("nes"));
    let preview = preview_game_rename(&lib.db, &lib.game, "New", 1).unwrap();
    std::fs::write(&lib.game, b"different content, different size").unwrap();
    assert!(apply_game_rename(&preview, &lib.db, &lib.journal(), &no_cancel()).is_err());
    assert!(lib.game.exists());
    assert!(!lib.root.join("nes").join("New.nes").exists());
}

#[test]
fn a_file_named_by_a_playlist_or_cue_sheet_is_refused() {
    for (sheet, text) in [
        ("Game.m3u", "Game (Disc 1).nes\nGame (Disc 2).nes\n"),
        ("Game.cue", "FILE \"Game (Disc 1).nes\" BINARY\n"),
    ] {
        let lib = Lib::new("Game (Disc 1).nes", Some("nes"));
        std::fs::write(lib.root.join("nes").join(sheet), text).unwrap();
        let error = preview_game_rename(&lib.db, &lib.game, "Renamed", 1).unwrap_err();
        assert!(error.contains(sheet), "{error}");
        assert!(lib.game.exists());
    }
}

#[test]
fn same_name_unknown_platform_and_missing_rows_are_refused() {
    let lib = Lib::new("Old.nes", Some("nes"));
    assert!(
        preview_game_rename(&lib.db, &lib.game, "Old", 1)
            .unwrap_err()
            .contains("already")
    );
    let unknown = Lib::new("Old.nes", None);
    assert!(preview_game_rename(&unknown.db, &unknown.game, "New", 1).is_err());
    assert!(unknown.game.exists());
    let stray = lib.root.join("nes").join("Stray.nes");
    std::fs::write(&stray, b"x").unwrap();
    assert!(
        preview_game_rename(&lib.db, &stray, "New", 1)
            .unwrap_err()
            .contains("not in the library")
    );
}
