archivefs_core::install_test_environment!();

use archivefs_core::synthetic_media as samples;
use archivefs_core::{Config, Database, scan_and_persist};
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;

fn zip(path: &Path, members: &[(&str, Vec<u8>)]) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    for (name, bytes) in members {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

fn tap() -> Vec<u8> {
    let mut bytes = vec![18, 0, 0xff];
    bytes.extend([0; 16]);
    bytes.push(0xff);
    bytes
}

fn scan(db: &mut Database, source: &Path) {
    let config = Config {
        source_folders: vec![source.to_path_buf()],
        mount_root: source.join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    let result = scan_and_persist(db, &config, "archive-member-platform-test").unwrap();
    assert!(
        result.folder_errors.is_empty(),
        "{:?}",
        result.folder_errors
    );
}

fn row(db: &Database, name: &str) -> archivefs_core::PersistedArchive {
    db.load_archives()
        .unwrap()
        .into_iter()
        .find(|row| row.absolute_path.file_name().is_some_and(|n| n == name))
        .unwrap()
}

#[test]
fn validated_members_assign_platform_without_game_or_dat_identity() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("unsorted");
    std::fs::create_dir(&source).unwrap();
    for (name, extension, bytes) in [
        ("tape", "tap", tap()),
        ("snapshot", "z80", samples::zx_z80_v1()),
        ("memory", "sna", samples::zx_sna_48k()),
        ("disk", "adf", samples::amiga_adf(1, b"TEST")),
    ] {
        let member_name = format!("game.{extension}");
        zip(
            &source.join(format!("{name}.zip")),
            &[(member_name.as_str(), bytes)],
        );
    }
    let mut db = Database::open_or_create(&dir.path().join("library.db")).unwrap();
    scan(&mut db, &source);
    for (name, platform) in [
        ("tape", "ZX Spectrum"),
        ("snapshot", "ZX Spectrum"),
        ("memory", "ZX Spectrum"),
        ("disk", "Amiga"),
    ] {
        let item = row(&db, &format!("{name}.zip"));
        assert_eq!(item.platform.as_deref(), Some(platform), "{name}");
        assert_eq!(
            item.platform_source.as_deref(),
            Some("archive_member_structure")
        );
    }
}

#[test]
fn shared_tzx_and_conflicting_members_remain_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("unsorted");
    std::fs::create_dir(&source).unwrap();
    zip(
        &source.join("shared.zip"),
        &[("tape.tzx", samples::zx_tzx())],
    );
    zip(
        &source.join("mixed.zip"),
        &[
            ("one.z80", samples::zx_z80_v1()),
            ("two.adf", samples::amiga_adf(1, b"TEST")),
        ],
    );
    zip(
        &source.join("shared-mixed.zip"),
        &[
            ("one.z80", samples::zx_z80_v1()),
            ("two.tzx", samples::zx_tzx()),
        ],
    );
    let mut db = Database::open_or_create(&dir.path().join("library.db")).unwrap();
    scan(&mut db, &source);
    assert!(row(&db, "shared.zip").platform.is_none());
    assert!(row(&db, "mixed.zip").platform.is_none());
    assert!(row(&db, "shared-mixed.zip").platform.is_none());
}

#[test]
fn completed_replacement_retires_stale_member_evidence_but_manual_stands() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("unsorted");
    std::fs::create_dir(&source).unwrap();
    let path = source.join("changing.zip");
    zip(&path, &[("one.z80", samples::zx_z80_v1())]);
    let mut db = Database::open_or_create(&dir.path().join("library.db")).unwrap();
    scan(&mut db, &source);
    assert_eq!(
        row(&db, "changing.zip").platform.as_deref(),
        Some("ZX Spectrum")
    );
    zip(&path, &[("other.adf", samples::amiga_adf(1, b"TEST"))]);
    scan(&mut db, &source);
    assert_eq!(row(&db, "changing.zip").platform.as_deref(), Some("Amiga"));
    zip(&path, &[("shared.tzx", samples::zx_tzx())]);
    scan(&mut db, &source);
    assert!(
        row(&db, "changing.zip").platform.is_none(),
        "{:?}",
        row(&db, "changing.zip")
    );
    let id = row(&db, "changing.zip").id;
    db.set_manual_platform(id, "Amiga").unwrap();
    zip(&path, &[("again.z80", samples::zx_z80_v1())]);
    scan(&mut db, &source);
    assert_eq!(
        row(&db, "changing.zip").platform_source.as_deref(),
        Some("manual")
    );
    zip(&path, &[("shared.tzx", samples::zx_tzx())]);
    scan(&mut db, &source);
    db.clear_manual_platform(id).unwrap();
    assert!(
        row(&db, "changing.zip").platform.is_none(),
        "{:?}",
        row(&db, "changing.zip")
    );
}

#[test]
fn interrupted_member_inspection_does_not_withdraw_old_assignment() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("unsorted");
    std::fs::create_dir(&source).unwrap();
    let path = source.join("changing.zip");
    zip(&path, &[("one.z80", samples::zx_z80_v1())]);
    let mut db = Database::open_or_create(&dir.path().join("library.db")).unwrap();
    scan(&mut db, &source);
    zip(&path, &[("too-large.z80", vec![0; 8 * 1024 * 1024 + 1])]);
    scan(&mut db, &source);
    assert_eq!(
        row(&db, "changing.zip").platform_source.as_deref(),
        Some("archive_member_structure")
    );
    assert_eq!(row(&db, "changing.zip").last_known_health, "RetryAvailable");
}

#[test]
fn folder_and_verified_dat_assignments_outweigh_archive_members() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("unsorted");
    let folder = source.join("Amiga");
    std::fs::create_dir_all(&folder).unwrap();
    zip(
        &folder.join("folder.zip"),
        &[("snap.z80", samples::zx_z80_v1())],
    );
    zip(
        &source.join("dat.zip"),
        &[("snap.z80", samples::zx_z80_v1())],
    );
    let mut db = Database::open_or_create(&dir.path().join("library.db")).unwrap();
    scan(&mut db, &source);
    assert_eq!(row(&db, "folder.zip").platform.as_deref(), Some("Amiga"));
    let dat_id = row(&db, "dat.zip").id;
    db.assign_platform(dat_id, Some("Amiga"), "verified_dat")
        .unwrap();
    scan(&mut db, &source);
    let dat = row(&db, "dat.zip");
    assert_eq!(dat.platform.as_deref(), Some("Amiga"));
    assert_eq!(dat.platform_source.as_deref(), Some("verified_dat"));
}
