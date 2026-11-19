//! Blue rename card state: preview, confirm, refusal; disposable library only.
use super::State;
use std::path::{Path, PathBuf};

fn library(temp: &Path) -> (PathBuf, PathBuf, i64) {
    let root = temp.join("games");
    std::fs::create_dir_all(root.join("nes")).unwrap();
    let game = root.join("nes").join("Old.nes");
    std::fs::write(&game, b"NES\x1a synthetic").unwrap();
    let database = temp.join("library.sqlite3");
    let mut db = archivefs_core::Database::open_or_create(&database).unwrap();
    db.register_source_folders(std::slice::from_ref(&root))
        .unwrap();
    let folder = db.list_source_folders().unwrap()[0].id;
    let archive = archivefs_core::Archive::from_path_in_root(&game, &root).unwrap();
    let id = db
        .upsert_archive(folder, &root, &archive)
        .unwrap()
        .archive_id;
    db.set_manual_platform(id, "nes").unwrap();
    (database, game, id)
}

fn state(temp: &Path, database: PathBuf) -> State {
    State {
        paths: Some((database, temp.join("journal"))),
        saves: Some(Default::default()),
        ..State::default()
    }
}

#[test]
fn preview_then_confirm_renames_and_keeps_the_game() {
    let temp = tempfile::tempdir().unwrap();
    let (database, game, id) = library(temp.path());
    let mut card = state(temp.path(), database.clone());
    card.typed = "New Game (USA)".into();
    assert!(!card.confirm(), "nothing happens without a preview");
    assert!(game.exists());
    card.preview_now(&game);
    assert!(
        matches!(card.preview, Some(Ok(_))),
        "{:?}",
        card.preview.as_ref().map(|r| r.as_ref().err())
    );
    assert!(card.confirm());
    let renamed = game.with_file_name("New Game (USA).nes");
    assert!(renamed.exists() && !game.exists());
    let db = archivefs_core::Database::open_read_only(&database).unwrap();
    assert_eq!(
        db.find_archive_id_by_absolute_path(&renamed).unwrap(),
        Some(id)
    );
    assert!(matches!(&card.outcome, Some(Ok(message)) if message.contains("Undo is in History")));
}

#[test]
fn a_clash_is_shown_and_nothing_changes() {
    let temp = tempfile::tempdir().unwrap();
    let (database, game, _) = library(temp.path());
    std::fs::write(game.with_file_name("Taken.nes"), b"other").unwrap();
    let mut card = state(temp.path(), database);
    card.typed = "Taken".into();
    card.preview_now(&game);
    assert!(matches!(card.preview, Some(Err(_))));
    assert!(!card.confirm());
    assert!(game.exists());
    assert_eq!(
        std::fs::read(game.with_file_name("Taken.nes")).unwrap(),
        b"other"
    );
}
