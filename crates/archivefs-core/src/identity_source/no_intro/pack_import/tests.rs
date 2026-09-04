use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::*;
use crate::identity_source::no_intro::load_no_intro_pack_snapshots_at;

const GB_DAT: &str = r#"<?xml version="1.0"?>
<datafile><header><name>Nintendo - Game Boy</name><version>20250101</version><author>No-Intro</author></header>
<game name="Test"><rom name="test.gb" size="1" crc="AAAAAAAA"/></game></datafile>"#;
const GBA_DAT: &str = r#"<?xml version="1.0"?>
<datafile><header><name>Nintendo - Game Boy Advance</name><version>20250102</version><author>No-Intro</author></header>
<game name="Test"><rom name="test.gba" size="1" crc="BBBBBBBB"/></game></datafile>"#;
const TOSEC_DAT: &str = r#"<?xml version="1.0"?>
<datafile><header><name>Atari ST</name><author>TOSEC</author></header>
<game name="Test"><rom name="test.st" size="1" crc="AAAAAAAA"/></game></datafile>"#;
const AFTERMARKET_DAT: &str = r#"<?xml version="1.0"?>
<datafile><header><name>Nintendo - Love Pack (Aftermarket)</name><version>20260827</version><author>No-Intro</author></header>
<game name="Test"><rom name="test.gb" size="1" crc="CCCCCCCC"/></game></datafile>"#;

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

fn write_dat(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
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
fn absolute_and_windows_style_member_paths_are_refused() {
    for name in [
        "/absolute/evil.dat",
        "C:\\absolute\\evil.dat",
        "../../evil.dat",
    ] {
        let dir = tempdir().unwrap();
        let pack = write_zip(&dir.path().join("pack.zip"), &[(name, GB_DAT.as_bytes())]);
        let error = import_no_intro_pack_at(&pack, &dir.path().join("store")).unwrap_err();
        assert!(
            matches!(error, NoIntroPackImportError::Traversal { .. }),
            "{name}: {error}"
        );
    }
}

#[test]
fn oversized_zip_is_refused_before_archive_processing() {
    let dir = tempdir().unwrap();
    let pack = dir.path().join("oversized.zip");
    let file = fs::File::create(&pack).unwrap();
    file.set_len(NO_INTRO_PACK_MAX_BYTES + 1).unwrap();
    let error = import_no_intro_pack_at(&pack, &dir.path().join("store")).unwrap_err();
    assert!(matches!(
        error,
        NoIntroPackImportError::LimitExceeded { .. }
    ));
}

#[test]
fn oversized_member_name_is_refused() {
    let dir = tempdir().unwrap();
    let name = format!("{}.dat", "n".repeat(NO_INTRO_PACK_MAX_MEMBER_NAME_BYTES));
    let pack = write_zip(&dir.path().join("pack.zip"), &[(&name, GB_DAT.as_bytes())]);
    let error = import_no_intro_pack_at(&pack, &dir.path().join("store")).unwrap_err();
    assert!(matches!(error, NoIntroPackImportError::Traversal { .. }));
}

#[test]
fn oversized_dat_member_is_rejected_without_publication() {
    let dir = tempdir().unwrap();
    let bytes = vec![0_u8; (NO_INTRO_PACK_MAX_DAT_BYTES + 1) as usize];
    let pack = write_zip(&dir.path().join("pack.zip"), &[("oversized.dat", &bytes)]);
    let report = import_no_intro_pack_at(&pack, &dir.path().join("store")).unwrap();
    assert!(report.accepted.is_empty());
    assert_eq!(report.rejected.len(), 1);
}

#[test]
fn directories_are_ignored_without_becoming_dat_members() {
    let dir = tempdir().unwrap();
    let pack_path = dir.path().join("pack.zip");
    let file = fs::File::create(&pack_path).unwrap();
    let mut zip = ZipWriter::new(file);
    zip.add_directory("Nintendo/", SimpleFileOptions::default())
        .unwrap();
    zip.finish().unwrap();
    let report = import_no_intro_pack_at(&pack_path, &dir.path().join("store")).unwrap();
    assert!(report.accepted.is_empty());
    assert!(report.rejected.is_empty());
}

#[test]
fn identical_member_payload_reuses_content_addressed_snapshot() {
    let dir = tempdir().unwrap();
    let first_pack = write_zip(
        &dir.path().join("first.zip"),
        &[("first.dat", GB_DAT.as_bytes())],
    );
    let second_pack = write_zip(
        &dir.path().join("second.zip"),
        &[("second.dat", GB_DAT.as_bytes())],
    );
    let storage = dir.path().join("store");
    let first = import_no_intro_pack_at(&first_pack, &storage).unwrap();
    let second = import_no_intro_pack_at(&second_pack, &storage).unwrap();
    assert_eq!(first.snapshot_path, second.snapshot_path);
    let snapshots = fs::read_dir(storage.join("snapshots")).unwrap().count();
    assert_eq!(snapshots, 1);
    let lifecycle = load_no_intro_pack_snapshots_at(&storage).unwrap();
    assert_eq!(lifecycle.len(), 2);
    assert_eq!(
        lifecycle[0].coverage[0].dat_member_identity,
        lifecycle[1].coverage[0].dat_member_identity
    );
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

#[test]
fn inspection_is_non_mutating_and_classifies_from_dat_metadata() {
    let dir = tempdir().unwrap();
    let pack = write_zip(
        &dir.path().join("ordinary-name.zip"),
        &[("catalogue.dat", AFTERMARKET_DAT.as_bytes())],
    );
    let inspection = inspect_no_intro_pack_at(&pack).unwrap();
    assert_eq!(
        inspection.classification,
        NoIntroPackClassification::Aftermarket
    );
    assert_eq!(inspection.accepted.len(), 1);
    assert_eq!(
        inspection.accepted[0].system_name,
        "Nintendo - Love Pack (Aftermarket)"
    );
    assert!(!dir.path().join("state.json").exists());
}

#[test]
fn aggregate_dat_bound_has_explicit_edges() {
    assert!(validate_total_dat_bytes(NO_INTRO_PACK_MAX_TOTAL_DAT_BYTES).is_ok());
    assert!(matches!(
        validate_total_dat_bytes(NO_INTRO_PACK_MAX_TOTAL_DAT_BYTES + 1),
        Err(NoIntroPackImportError::LimitExceeded { .. })
    ));
}

#[test]
fn official_no_intro_header_shape_is_recognised_without_filename_authority() {
    let dir = tempdir().unwrap();
    let dat = write_dat(
        dir.path(),
        "unrelated-name.dat",
        r#"<?xml version="1.0"?><datafile><header><name>Nintendo - NES</name><version>2026</version><author>Contributor One, Contributor Two</author><homepage>No-Intro</homepage><url>https://www.no-intro.org</url></header><game name="Test"><rom name="test.nes" size="1" crc="AAAAAAAA"/></game></datafile>"#,
    );
    let source = import_no_intro_dat(&dat).unwrap();
    assert_eq!(
        source.dat.source.ecosystem,
        crate::dat::model::DatEcosystem::NoIntro
    );
}

#[test]
fn lookalike_no_intro_host_is_rejected() {
    let dir = tempdir().unwrap();
    let dat = write_dat(
        dir.path(),
        "not-no-intro.dat",
        r#"<?xml version="1.0"?><datafile><header><name>Nintendo - NES</name><homepage>No-Intro</homepage><url>https://www.no-intro.org.example</url></header><game name="Test"><rom name="test.nes" size="1" crc="AAAAAAAA"/></game></datafile>"#,
    );
    assert!(matches!(
        import_no_intro_dat(&dat),
        Err(NoIntroImportError::NotNoIntro { .. })
    ));
}

#[test]
#[ignore = "manual verification against real Downloads ZIPs; not portable to CI"]
fn manual_real_love_pack_verification() {
    let standard_zip =
        Path::new("/home/davedap/Downloads/No-Intro Love Pack (DAT) (2026-08-27).zip");
    let aftermarket_zip = Path::new(
        "/home/davedap/Downloads/No-Intro Love Pack (DAT) (Aftermarket) (2026-08-27).zip",
    );
    assert!(standard_zip.is_file(), "standard pack missing");
    assert!(aftermarket_zip.is_file(), "aftermarket pack missing");

    let storage = tempdir().unwrap();
    let root = storage.path().join("no_intro_pack");

    // ---- Inspect standard pack ----
    let inspection = inspect_no_intro_pack_at(standard_zip).unwrap();
    eprintln!(
        "STANDARD INSPECT: accepted={} rejected={} classification={:?}",
        inspection.accepted.len(),
        inspection.rejected.len(),
        inspection.classification
    );
    for r in &inspection.rejected {
        eprintln!("STANDARD REJECTED: member={} reason={}", r.member, r.reason);
    }

    // ---- Import standard pack ----
    let report1 = import_no_intro_pack_at(standard_zip, &root).unwrap();
    eprintln!(
        "STANDARD IMPORT: status={:?} accepted={} rejected={}",
        report1.status,
        report1.accepted.len(),
        report1.rejected.len()
    );
    for r in &report1.rejected {
        eprintln!(
            "STANDARD IMPORT REJECTED: member={} reason={}",
            r.member, r.reason
        );
    }

    // ---- Inspect aftermarket pack ----
    let inspection2 = inspect_no_intro_pack_at(aftermarket_zip).unwrap();
    eprintln!(
        "AFTERMARKET INSPECT: accepted={} rejected={} classification={:?}",
        inspection2.accepted.len(),
        inspection2.rejected.len(),
        inspection2.classification
    );
    for r in &inspection2.rejected {
        eprintln!(
            "AFTERMARKET REJECTED: member={} reason={}",
            r.member, r.reason
        );
    }

    // ---- Import/merge aftermarket pack ----
    let report2 = import_no_intro_pack_at(aftermarket_zip, &root).unwrap();
    eprintln!(
        "AFTERMARKET IMPORT: status={:?} accepted={} rejected={}",
        report2.status,
        report2.accepted.len(),
        report2.rejected.len()
    );
    for r in &report2.rejected {
        eprintln!(
            "AFTERMARKET IMPORT REJECTED: member={} reason={}",
            r.member, r.reason
        );
    }

    // ---- Verify all 18 standard-only DATs survived the merge ----
    let standard_names: std::collections::HashSet<_> = report1
        .accepted
        .iter()
        .map(|s| s.system_name.clone())
        .collect();
    let merged_names: std::collections::HashSet<_> = report2
        .accepted
        .iter()
        .map(|s| s.system_name.clone())
        .collect();
    let lost: Vec<_> = standard_names.difference(&merged_names).collect();
    eprintln!("LOST FROM STANDARD AFTER MERGE: {lost:?}");
    eprintln!("MERGED TOTAL: {}", report2.accepted.len());

    // ---- Reload from disk ----
    let reloaded = load_current_no_intro_pack_at(&root).unwrap().unwrap();
    eprintln!("RELOADED: accepted={}", reloaded.len());
    assert_eq!(reloaded.len(), report2.accepted.len());

    // ---- Reimport identical packs: expect Unchanged ----
    let report1_again = import_no_intro_pack_at(standard_zip, &root).unwrap();
    eprintln!("STANDARD REIMPORT status={:?}", report1_again.status);
    let report2_again = import_no_intro_pack_at(aftermarket_zip, &root).unwrap();
    eprintln!("AFTERMARKET REIMPORT status={:?}", report2_again.status);

    // ---- Confirm packing_policy is Standard for real No-Intro DATs ----
    let non_standard: Vec<_> = report2
        .accepted
        .iter()
        .filter(|s| s.dat.source.packing_policy != crate::dat::model::DatPackingPolicy::Standard)
        .map(|s| s.system_name.clone())
        .collect();
    eprintln!("NON-STANDARD PACKING POLICY MEMBERS: {non_standard:?}");

    // ---- Confirm ZIP hashes unchanged ----
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(fs::read(standard_zip).unwrap());
    let digest1 = hasher.finalize();
    let hex1: String = digest1.iter().map(|b| format!("{b:02x}")).collect();
    eprintln!("STANDARD ZIP SHA256 AFTER: {hex1}");
    let mut hasher2 = Sha256::new();
    hasher2.update(fs::read(aftermarket_zip).unwrap());
    let digest2 = hasher2.finalize();
    let hex2: String = digest2.iter().map(|b| format!("{b:02x}")).collect();
    eprintln!("AFTERMARKET ZIP SHA256 AFTER: {hex2}");
}
