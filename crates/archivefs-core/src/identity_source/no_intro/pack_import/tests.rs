use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::*;

const GB_DAT: &str = r#"<?xml version="1.0"?>
<datafile><header><name>Nintendo - Game Boy</name><version>20250101</version><author>No-Intro</author></header>
<game name="Test"><rom name="test.gb" size="1" crc="AAAAAAAA"/></game></datafile>"#;
const GBA_DAT: &str = r#"<?xml version="1.0"?>
<datafile><header><name>Nintendo - Game Boy Advance</name><version>20250102</version><author>No-Intro</author></header>
<game name="Test"><rom name="test.gba" size="1" crc="BBBBBBBB"/></game></datafile>"#;
const TOSEC_DAT: &str = r#"<?xml version="1.0"?>
<datafile><header><name>Atari ST</name><author>TOSEC</author></header>
<game name="Test"><rom name="test.st" size="1" crc="AAAAAAAA"/></game></datafile>"#;

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let file = fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    for (name, bytes) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut zip, bytes).unwrap();
    }
    zip.finish().unwrap();
    path.to_path_buf()
}

#[test]
fn valid_pack_imports_multiple_dat_members_and_ignores_noise() {
    let dir = tempdir().unwrap();
    let pack = write_zip(
        &dir.path().join("pack.zip"),
        &[
            ("Nintendo/gb.dat", GB_DAT.as_bytes()),
            ("Nintendo/gba.dat", GBA_DAT.as_bytes()),
            ("README.txt", b"downloaded through the browser"),
        ],
    );
    let storage = dir.path().join("store");
    let report = import_no_intro_pack_at(&pack, &storage).unwrap();
    assert_eq!(report.status, NoIntroPackImportStatus::Updated);
    assert_eq!(report.accepted.len(), 2);
    assert!(report.rejected.is_empty());
    assert!(report.accepted.iter().all(|source| {
        source.dat.source.ecosystem == crate::dat::model::DatEcosystem::NoIntro
            && source.artifact_path.is_file()
    }));
    assert!(storage.join("state.json").is_file());
}

#[test]
fn same_pack_is_idempotent_and_survives_reload() {
    let dir = tempdir().unwrap();
    let pack = write_zip(
        &dir.path().join("pack.zip"),
        &[("gb.dat", GB_DAT.as_bytes())],
    );
    let storage = dir.path().join("store");
    let first = import_no_intro_pack_at(&pack, &storage).unwrap();
    let second = import_no_intro_pack_at(&pack, &storage).unwrap();
    assert_eq!(second.status, NoIntroPackImportStatus::Unchanged);
    assert_eq!(first.pack_sha256, second.pack_sha256);
    assert_eq!(second.accepted[0].system_name, "Nintendo - Game Boy");
    assert_eq!(second.accepted[0].artifact_name, "gb.dat");
    let loaded = load_current_no_intro_pack_at(&storage).unwrap().unwrap();
    assert_eq!(loaded[0].system_name, "Nintendo - Game Boy");
    assert_eq!(loaded[0].artifact_name, "gb.dat");
}

#[test]
fn a_changed_pack_publishes_a_new_snapshot_atomically() {
    let dir = tempdir().unwrap();
    let storage = dir.path().join("store");
    let first_pack = write_zip(
        &dir.path().join("first.zip"),
        &[("gb.dat", GB_DAT.as_bytes())],
    );
    let first = import_no_intro_pack_at(&first_pack, &storage).unwrap();
    let second_pack = write_zip(
        &dir.path().join("second.zip"),
        &[("gba.dat", GBA_DAT.as_bytes())],
    );
    let second = import_no_intro_pack_at(&second_pack, &storage).unwrap();
    assert_eq!(second.status, NoIntroPackImportStatus::Updated);
    assert_ne!(first.pack_sha256, second.pack_sha256);
    assert_ne!(first.snapshot_path, second.snapshot_path);
    assert!(second.accepted[0].artifact_path.is_file());
}

#[test]
fn non_no_intro_dat_is_rejected_by_content() {
    let dir = tempdir().unwrap();
    let pack = write_zip(
        &dir.path().join("pack.zip"),
        &[("wrong.dat", TOSEC_DAT.as_bytes())],
    );
    let report = import_no_intro_pack_at(&pack, &dir.path().join("store")).unwrap();
    assert!(report.accepted.is_empty());
    assert_eq!(report.rejected.len(), 1);
    assert!(report.rejected[0].reason.contains("not No-Intro"));
}

#[test]
fn corrupt_dat_prevents_publication_and_preserves_previous_snapshot() {
    let dir = tempdir().unwrap();
    let storage = dir.path().join("store");
    let good = write_zip(
        &dir.path().join("good.zip"),
        &[("gb.dat", GB_DAT.as_bytes())],
    );
    let first = import_no_intro_pack_at(&good, &storage).unwrap();
    let bad = write_zip(
        &dir.path().join("bad.zip"),
        &[(
            "broken.dat",
            b"<?xml version=\"1.0\"?><datafile><header><name>Nintendo - Game Boy</name><author>No-Intro</author>",
        )],
    );
    let error = import_no_intro_pack_at(&bad, &storage).unwrap_err();
    assert!(matches!(
        error,
        NoIntroPackImportError::IncompleteDat { .. }
    ));
    let state = fs::read_to_string(storage.join("state.json")).unwrap();
    assert!(state.contains(&first.pack_sha256));
    assert!(first.snapshot_path.join("dats/0.dat").is_file());
}

#[test]
fn malformed_zip_is_refused_without_state() {
    let dir = tempdir().unwrap();
    let pack = dir.path().join("broken.zip");
    fs::write(&pack, b"not a zip").unwrap();
    let storage = dir.path().join("store");
    assert!(matches!(
        import_no_intro_pack_at(&pack, &storage),
        Err(NoIntroPackImportError::InvalidArchive { .. })
    ));
    assert!(!storage.join("state.json").exists());
}

#[test]
fn traversal_member_is_refused() {
    let dir = tempdir().unwrap();
    let pack = write_zip(
        &dir.path().join("pack.zip"),
        &[("../escape.dat", GB_DAT.as_bytes())],
    );
    let error = import_no_intro_pack_at(&pack, &dir.path().join("store")).unwrap_err();
    assert!(matches!(error, NoIntroPackImportError::Traversal { .. }));
}

#[test]
fn excessive_member_count_is_refused_before_extraction() {
    let dir = tempdir().unwrap();
    let pack_path = dir.path().join("many.zip");
    let file = fs::File::create(&pack_path).unwrap();
    let mut zip = ZipWriter::new(file);
    for index in 0..=NO_INTRO_PACK_MAX_MEMBERS {
        zip.start_file(format!("noise/{index}.txt"), SimpleFileOptions::default())
            .unwrap();
    }
    zip.finish().unwrap();
    let error = import_no_intro_pack_at(&pack_path, &dir.path().join("store")).unwrap_err();
    assert!(matches!(
        error,
        NoIntroPackImportError::LimitExceeded { .. }
    ));
}
