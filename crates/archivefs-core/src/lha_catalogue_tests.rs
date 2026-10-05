//! `.lha`/`.lzh` as catalogued library media (one `ArchiveKind::Lha`).
//!
//! A catalogue row is cheap registration only: no members are read to create
//! it, it is never a mount input, and it grants neither verified identity nor
//! launchability. Parent identity projection for LHA stays disabled (see
//! `dat::library_identity_projection`).

use std::path::{Path, PathBuf};

use crate::{ArchiveKind, Config, Database, archive_kind, media_registry, scan_and_persist};

fn config(source: &Path, root: &Path) -> Config {
    Config {
        source_folders: vec![source.to_path_buf()],
        mount_root: root.join("mount"),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    }
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn kinds(database: &Database) -> Vec<(PathBuf, String)> {
    let mut rows: Vec<_> = database
        .load_archives()
        .unwrap()
        .into_iter()
        .map(|row| (row.absolute_path, row.archive_kind))
        .collect();
    rows.sort();
    rows
}

#[test]
fn lha_and_lzh_are_one_catalogued_kind_with_case_insensitive_extensions() {
    for name in ["Game.lha", "Game.LHA", "Game.Lha", "Game.lzh", "Game.LZH"] {
        assert_eq!(
            archive_kind(Path::new(name)),
            Some(ArchiveKind::Lha),
            "{name}"
        );
    }
    assert_eq!(
        media_registry::kind_for_extension("lha"),
        Some(ArchiveKind::Lha)
    );
    assert_eq!(
        media_registry::kind_for_extension("lzh"),
        Some(ArchiveKind::Lha)
    );
    assert!(media_registry::is_watch_relevant_extension("lha"));
    assert!(media_registry::is_watch_relevant_extension("lzh"));
    assert_eq!(ArchiveKind::Lha.storage_name(), "lha");
    assert_eq!(ArchiveKind::from_storage("lha"), Some(ArchiveKind::Lha));
    // Deterministic, and no spelling gets its own kind.
    assert_eq!(ArchiveKind::from_storage("lzh"), None);
}

#[test]
fn unrelated_and_existing_extensions_are_unchanged() {
    for name in [
        "Game.lh",
        "Game.lhz",
        "Game.lha.bak",
        "Game.txt",
        "lha",
        "Game.l",
        "Game.arj",
    ] {
        assert_eq!(archive_kind(Path::new(name)), None, "{name}");
    }
    assert_eq!(archive_kind(Path::new("a.zip")), Some(ArchiveKind::Zip));
    assert_eq!(archive_kind(Path::new("a.7z")), Some(ArchiveKind::SevenZip));
    assert_eq!(archive_kind(Path::new("a.rar")), Some(ArchiveKind::Rar));
    assert_eq!(ArchiveKind::Zip.storage_name(), "zip");
    assert_eq!(ArchiveKind::SevenZip.storage_name(), "sevenzip");
    assert_eq!(ArchiveKind::Rar.storage_name(), "rar");
}

#[test]
fn an_lha_row_is_not_a_mount_input_and_is_not_directly_launchable_media() {
    assert!(!ArchiveKind::Lha.is_mount_input());
    assert!(ArchiveKind::Zip.is_mount_input());
    assert!(ArchiveKind::SevenZip.is_mount_input());
    assert!(ArchiveKind::Rar.is_mount_input());
    // Not a direct game image either: it must never take the loose-image paths.
    assert_ne!(ArchiveKind::Lha, ArchiveKind::DirectGameImage);
}

#[test]
fn scanning_creates_one_row_per_file_survives_reload_and_never_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("amiga");
    let lha = write(&source, "Game.lha", b"not even a real archive");
    let lzh = write(&source, "Other.lzh", b"also not parsed to create the row");
    let zip = write(&source, "Keep.zip", b"PK stand-in");
    write(&source, "notes.txt", b"ignored");
    let db_path = root.path().join("library.sqlite3");
    let cfg = config(&source, root.path());

    let mut database = Database::open_or_create(&db_path).unwrap();
    let first = scan_and_persist(&mut database, &cfg, "first").unwrap();
    assert_eq!(first.counts.archives_added, 3);
    let expected = vec![
        (lha.clone(), "lha".to_string()),
        (lzh.clone(), "lha".to_string()),
        (zip.clone(), "zip".to_string()),
    ];
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    assert_eq!(kinds(&database), expected_sorted);
    for path in [&lha, &lzh] {
        assert!(
            database
                .find_archive_id_by_absolute_path(path)
                .unwrap()
                .is_some()
        );
    }
    let ids_before: Vec<_> = database
        .load_archives()
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();

    // Reload from disk: rows survive with the same kind.
    drop(database);
    let mut database = Database::open_or_create(&db_path).unwrap();
    assert_eq!(kinds(&database), expected_sorted);

    // Rescan of unchanged files: nothing added, nothing duplicated, ids stable.
    let second = scan_and_persist(&mut database, &cfg, "second").unwrap();
    assert_eq!(second.counts.archives_added, 0);
    assert_eq!(second.counts.archives_changed, 0);
    assert_eq!(kinds(&database), expected_sorted);
    let ids_after: Vec<_> = database
        .load_archives()
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids_before, ids_after);
}

#[test]
fn a_changed_lha_invalidates_its_scan_fingerprint_like_other_archives() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("amiga");
    let lha = write(&source, "Game.lha", b"version one");
    let zip = write(&source, "Same.zip", b"PK stand-in");
    let cfg = config(&source, root.path());
    let mut database = Database::open_or_create(&root.path().join("l.sqlite3")).unwrap();
    scan_and_persist(&mut database, &cfg, "first").unwrap();
    let id = database
        .find_archive_id_by_absolute_path(&lha)
        .unwrap()
        .unwrap();

    // Replace at the same path with a different size.
    std::fs::write(&lha, b"version two is longer").unwrap();
    let again = scan_and_persist(&mut database, &cfg, "changed").unwrap();
    assert_eq!(again.counts.archives_changed, 1, "only the LHA changed");
    assert_eq!(again.counts.archives_added, 0);
    assert_eq!(
        database.find_archive_id_by_absolute_path(&lha).unwrap(),
        Some(id)
    );
    let row = database
        .load_archives()
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(row.size_bytes, Some(b"version two is longer".len() as u64));
    // The untouched ZIP is unchanged.
    assert!(
        database
            .find_archive_id_by_absolute_path(&zip)
            .unwrap()
            .is_some()
    );
}

#[test]
fn an_existing_library_without_the_new_kind_upgrades_in_place() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("lib");
    let zip = write(&source, "Old.zip", b"PK stand-in");
    let cfg = config(&source, root.path());
    let db_path = root.path().join("old.sqlite3");
    let mut database = Database::open_or_create(&db_path).unwrap();
    scan_and_persist(&mut database, &cfg, "before").unwrap();
    let zip_id = database
        .find_archive_id_by_absolute_path(&zip)
        .unwrap()
        .unwrap();
    assert!(
        database
            .load_archives()
            .unwrap()
            .iter()
            .all(|r| r.archive_kind != "lha")
    );
    drop(database);

    // Same database, new LHA appears: the old row is untouched, the new one added.
    let lha = write(&source, "New.lha", b"x");
    let mut database = Database::open_or_create(&db_path).unwrap();
    let summary = scan_and_persist(&mut database, &cfg, "after").unwrap();
    assert_eq!(summary.counts.archives_added, 1);
    assert_eq!(
        database.find_archive_id_by_absolute_path(&zip).unwrap(),
        Some(zip_id)
    );
    assert!(
        database
            .find_archive_id_by_absolute_path(&lha)
            .unwrap()
            .is_some()
    );
    // An unknown storage name still fails safe.
    assert_eq!(ArchiveKind::from_storage("rar5"), None);
}

#[test]
fn an_lha_row_does_not_imply_any_dat_identity() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("amiga");
    let lha = write(&source, "Game.lha", b"bytes");
    let mut database = Database::open_or_create(&root.path().join("l.sqlite3")).unwrap();
    scan_and_persist(&mut database, &config(&source, root.path()), "scan").unwrap();
    let id = database
        .find_archive_id_by_absolute_path(&lha)
        .unwrap()
        .unwrap();
    assert!(
        database
            .library_dat_identity_for_item(id, "any-source")
            .unwrap()
            .is_none()
    );
}

fn put16(v: &mut [u8], at: usize, n: u16) {
    v[at..at + 2].copy_from_slice(&n.to_be_bytes());
}
fn put32(v: &mut [u8], at: usize, n: u32) {
    v[at..at + 4].copy_from_slice(&n.to_be_bytes());
}

fn slave_bytes() -> Vec<u8> {
    let size: usize = 54;
    let mut code = vec![0; (size + 64).next_multiple_of(4)];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    put16(&mut code, 12, 20);
    put16(&mut code, 14, 3);
    put32(&mut code, 16, 524288);
    put32(&mut code, 20, 1);
    put32(&mut code, 24, 2);
    put16(&mut code, 28, 0);
    put16(&mut code, 34, size as u16);
    put16(&mut code, 36, (size + 8) as u16);
    put16(&mut code, 38, (size + 16) as u16);
    put16(&mut code, 40, (size + 24) as u16);
    put32(&mut code, 42, 512 * 1024);
    put16(&mut code, 46, 0x1234);
    put16(&mut code, 48, (size + 32) as u16);
    code[size..size + 5].copy_from_slice(b"Game\0");
    code[size + 8..size + 13].copy_from_slice(b"Copy\0");
    code[size + 16..size + 21].copy_from_slice(b"Info\0");
    code[size + 24..size + 29].copy_from_slice(b"Kick\0");
    code[size + 32..size + 39].copy_from_slice(b"Config\0");
    let mut out = Vec::new();
    for n in [
        0x3f3_u32,
        0,
        1,
        0,
        0,
        (code.len() / 4) as u32,
        0x3e9,
        (code.len() / 4) as u32,
    ] {
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend_from_slice(&code);
    out.extend_from_slice(&0x3f2_u32.to_be_bytes());
    out
}

#[test]
fn whdload_discovery_is_identical_with_and_without_lha_catalogue_rows() {
    use crate::dat::archive::lha_header::fixtures::{Entry, archive};
    use crate::ingestion::discovery::{discover_source, discover_source_with_fingerprints};
    if crate::dat::archive::lha::LhaProvider::discover(std::time::Duration::from_secs(10)).is_err()
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("amiga");
    // A real WHDLoad-shaped package, a plain LHA, and an LZH.
    write(
        &source,
        "Game.lha",
        &archive(&[Entry::unix("Game.slave", &slave_bytes(), 0o100644).level(1)]),
    );
    write(
        &source,
        "Plain.lha",
        &archive(&[Entry::unix("a.txt", b"hi", 0o100644).level(1)]),
    );
    write(
        &source,
        "Old.lzh",
        &archive(&[Entry::unix("b.txt", b"yo", 0o100644).level(1)]),
    );

    let before = discover_source(&source).unwrap();

    let mut database = Database::open_or_create(&root.path().join("l.sqlite3")).unwrap();
    scan_and_persist(&mut database, &config(&source, root.path()), "scan").unwrap();
    let rows = database.load_archives().unwrap();
    assert_eq!(
        rows.len(),
        3,
        "each LHA/LZH is one row, not one per discovery item"
    );
    let folder_id = rows[0].source_folder_id;
    let fingerprints: Vec<_> = database
        .load_scan_fingerprints(folder_id)
        .unwrap()
        .into_iter()
        .map(|fingerprint| (folder_id, fingerprint))
        .collect();
    assert_eq!(fingerprints.len(), 3);
    let after = discover_source_with_fingerprints(&source, &fingerprints).unwrap();

    // Identical candidates, explanations and counts: the rows change nothing.
    assert_eq!(format!("{:?}", before.items), format!("{:?}", after.items));
    assert_eq!(before.detail_fingerprint, after.detail_fingerprint);
    assert_eq!(before.reuse.cache_hits, after.reuse.cache_hits);
    let whdload: Vec<_> = before
        .items
        .iter()
        .filter(|item| format!("{item:?}").contains("WHDLoad archive"))
        .collect();
    assert_eq!(
        whdload.len(),
        1,
        "only the verified-slave package is WHDLoad"
    );
}
