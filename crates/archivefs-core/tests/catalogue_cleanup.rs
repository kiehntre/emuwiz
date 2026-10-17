use archivefs_core::{Config, Database, scan_and_persist};
use std::{fs, path::PathBuf};

fn fixture() -> (tempfile::TempDir, Database, Config, PathBuf, i64) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let path = source.join("game.zip");
    fs::write(&path, b"synthetic").unwrap();
    let config = Config {
        source_folders: vec![source],
        mount_root: root.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    let mut db = Database::open_or_create(root.path().join("library.sqlite3")).unwrap();
    scan_and_persist(&mut db, &config, "present").unwrap();
    let id = db.load_archives().unwrap()[0].id;
    fs::remove_file(&path).unwrap();
    scan_and_persist(&mut db, &config, "missing").unwrap();
    (root, db, config, path, id)
}

#[test]
fn historical_missing_does_not_authorise_cleanup_of_a_returned_file() {
    let (_root, mut db, _config, path, id) = fixture();
    let before = db.load_archives().unwrap();
    fs::write(&path, b"returned").unwrap();
    assert!(
        db.remove_missing_archives(&[id]).is_err(),
        "historical missing must not authorise current cleanup"
    );
    assert_eq!(db.load_archives().unwrap(), before);
    assert_eq!(fs::read(path).unwrap(), b"returned");
}
