use archivefs_core::ingestion::preview::{MediaDisposition, SpectrumCatalogue, preview_media};
use archivefs_core::{ArchiveKind, ArchiveScanner, Config, Database, ScanFingerprint};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

fn snapshot() -> Vec<u8> {
    let mut b = vec![0; 49182];
    b[7] = 0x80;
    b[9] = 0xc0;
    b[27] = 255;
    b[28] = 255;
    b[29] = 1;
    b
}
fn sna() -> Vec<u8> {
    let mut b = vec![0; 49179];
    b[24] = 0xc0;
    b[25] = 1;
    b[26] = 31;
    b
}
fn trd() -> Vec<u8> {
    let mut b = vec![0; 655360];
    b[0x8e1] = 0;
    b[0x8e2] = 1;
    b[0x8e3] = 0x16;
    b[0x8e5..0x8e7].copy_from_slice(&2544u16.to_le_bytes());
    b[0x8e7] = 0x10;
    b
}
fn scl() -> Vec<u8> {
    let mut b = b"SINCLAIR\x01".to_vec();
    b.extend_from_slice(&[0; 14]);
    b[22] = 1;
    b.extend_from_slice(&[0; 256]);
    b.extend_from_slice(&[0; 4]);
    b
}
fn config(root: &Path) -> Config {
    Config {
        source_folders: vec![root.into()],
        mount_root: root.join("mount"),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    }
}
fn fixture() -> (tempfile::TempDir, PathBuf, Config) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("zxs");
    std::fs::create_dir(&root).unwrap();
    let c = config(&root);
    (t, root, c)
}
fn dat(root: &Path, files: &[(&str, &[u8])]) -> SpectrumCatalogue {
    let trusted = archivefs_core::safe_read::TrustedRoots::from_paths([root]);
    let mut xml = String::from(
        "<datafile><header><name>Sinclair ZX Spectrum - Games - [Z80]</name></header>",
    );
    for (name, bytes) in files {
        let p = root.join(format!("hash-input-{}.tmp", xml.len()));
        std::fs::write(&p, bytes).unwrap();
        let h = archivefs_core::identity_source::hashing::hash_file(&p, &trusted, None).unwrap();
        std::fs::remove_file(p).unwrap();
        xml.push_str(&format!("<game name=\"{name}\"><rom name=\"unrelated\" size=\"{}\" crc=\"{}\" md5=\"{}\" sha1=\"{}\"/></game>",bytes.len(),h.crc32,h.md5,h.sha1));
    }
    xml.push_str("</datafile>");
    let path = root.join("catalogue.dat");
    std::fs::write(&path, xml).unwrap();
    let parsed = archivefs_core::dat::parsers::parse_dat_file(&path, Default::default()).unwrap();
    std::fs::remove_file(path).unwrap();
    SpectrumCatalogue::from_parsed(&parsed.dat).unwrap()
}
#[test]
fn spectrum_supported_families_and_invalid_exclusions_use_existing_inspectors() {
    let (_t, root, c) = fixture();
    let fixtures = [
        ("a.z80", snapshot()),
        ("b.sna", sna()),
        ("c.szx", b"ZXST\x01\x04\x01\0CRTR\0\0\0\0".to_vec()),
        ("d.trd", trd()),
        ("e.scl", scl()),
        ("f.tap", vec![2, 0, 0, 0]),
        ("g.tzx", b"ZXTape!\x1a\x01\x14".to_vec()),
    ];
    for (name, bytes) in &fixtures {
        std::fs::write(root.join(name), bytes).unwrap();
    }
    std::fs::write(root.join("bad.z80"), [255; 16]).unwrap();
    std::fs::write(root.join("unsupported.sp"), [0; 64]).unwrap();
    let scan = ArchiveScanner::new(&c)
        .scan_archives_with_summary()
        .unwrap();
    assert_eq!(scan.archives.len(), 7);
    assert_eq!(scan.total_skipped(), 2);
    assert!(
        scan.skipped_files
            .iter()
            .any(|f| f.path.ends_with("bad.z80")
                && f.reason == archivefs_core::SkipReason::InvalidContent)
    );
    let preview =
        preview_media(&c, &[], &[], &[], &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(preview.count(MediaDisposition::Eligible), 7);
    assert_eq!(preview.count(MediaDisposition::Excluded), 2);
    assert!(
        preview
            .items
            .iter()
            .find(|i| i.path.ends_with("bad.z80"))
            .unwrap()
            .explanation
            .contains("snapshot")
    );
    for (name, bytes) in fixtures {
        assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
    }
}
#[test]
fn spectrum_old_negative_and_positive_cache_do_not_bypass_structure() {
    let (_t, root, c) = fixture();
    std::fs::write(root.join("good.z80"), snapshot()).unwrap();
    std::fs::write(root.join("bad.sna"), [0; 32]).unwrap();
    let fingerprint = |name: &str, negative: bool| {
        let path = root.join(name);
        let m = path.metadata().unwrap();
        (
            1,
            ScanFingerprint {
                relative_path: name.into(),
                size_bytes: m.len(),
                modified_time_ns: m
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as i128,
                archive_kind: ArchiveKind::DirectGameImage,
                display_name: name.into(),
                normalized_name: name.into(),
                platform: Some("ZX Spectrum".into()),
                platform_provenance: None,
                scanner_version: archivefs_core::SCANNER_VERSION.into(),
                parser_version: archivefs_core::ARCHIVE_PARSER_VERSION.into(),
                cache_version: archivefs_core::SCAN_CACHE_VERSION,
                ingestion: None,
                non_archive_kind: negative.then(|| "unsupported".into()),
            },
        )
    };
    let scan = ArchiveScanner::new(&c)
        .scan_archives_with_cache(&[fingerprint("good.z80", true), fingerprint("bad.sna", false)])
        .unwrap();
    assert_eq!(scan.archives.len(), 1);
    assert!(scan.archives[0].path.ends_with("good.z80"));
    assert_eq!(scan.timings.reused, 0);
}
#[test]
fn spectrum_repeat_scan_preserves_ids_manual_platform_and_dat_blob() {
    let (t, root, c) = fixture();
    std::fs::write(root.join("sample.z80"), snapshot()).unwrap();
    std::fs::write(root.join("disk.scl"), scl()).unwrap();
    let path = t.path().join("disposable.sqlite3");
    let mut db = Database::open_or_create(&path).unwrap();
    archivefs_core::scan_and_persist(&mut db, &c, "synthetic-first").unwrap();
    let before = db.load_archives().unwrap();
    let id = before
        .iter()
        .find(|a| a.absolute_path.ends_with("sample.z80"))
        .unwrap()
        .id;
    db.set_manual_platform(id, "GameCube").unwrap();
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute("INSERT INTO library_dat_identities(archive_id,dat_source_id,verification_state,completeness,audited_at,facts_json,created_at,updated_at) VALUES(?1,'preservation-fixture','Verified','complete','fixture',?2,'fixture','fixture')",rusqlite::params![id,b"opaque-existing-evidence".as_slice()]).unwrap();
    archivefs_core::scan_and_persist(&mut db, &c, "synthetic-repeat").unwrap();
    let after = db.load_archives().unwrap();
    assert_eq!(
        before.iter().map(|a| a.id).collect::<Vec<_>>(),
        after.iter().map(|a| a.id).collect::<Vec<_>>()
    );
    assert_eq!(
        after
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .platform
            .as_deref(),
        Some("GameCube")
    );
    let blob: Vec<u8> = sql
        .query_row(
            "SELECT facts_json FROM library_dat_identities WHERE archive_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(blob, b"opaque-existing-evidence");
    let db_before = std::fs::read(&path).unwrap();
    preview_media(
        &c,
        &[],
        &after,
        &[],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        db_before,
        "preview never opens a writable database"
    );
}
#[test]
fn spectrum_preview_keeps_alternates_and_parts_separate_and_cancels() {
    let (t, root, c) = fixture();
    let first = snapshot();
    std::fs::write(root.join("existing.z80"), &first).unwrap();
    let mut db = Database::open_or_create(t.path().join("copy.sqlite3")).unwrap();
    archivefs_core::scan_and_persist(&mut db, &c, "seed").unwrap();
    let known = db.load_archives().unwrap();
    let mut alternate = sna();
    alternate[0] = 3;
    let mut part = snapshot();
    part[0] = 6;
    std::fs::write(root.join("alternate.sna"), &alternate).unwrap();
    std::fs::write(root.join("part.z80"), &part).unwrap();
    let catalogue = dat(
        &root,
        &[
            ("Example (1986)(Publisher)", &first),
            ("Example (1986)(Publisher)", &alternate),
            ("Adventure (1986)(Publisher)(Part 1 of 2)", &part),
        ],
    );
    let report = preview_media(
        &c,
        &[],
        &known,
        &[catalogue],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(report.count(MediaDisposition::Eligible), 2);
    assert!(
        report
            .items
            .iter()
            .find(|i| i.path.ends_with("alternate.sna"))
            .unwrap()
            .alternate
            .as_deref()
            .unwrap()
            .contains("Same exact")
    );
    assert!(
        report
            .items
            .iter()
            .find(|i| i.path.ends_with("part.z80"))
            .unwrap()
            .multipart
            .is_some()
    );
    let cancel = AtomicBool::new(false);
    let report = preview_media(&c, &[], &known, &[], &cancel, &mut |_, _| {
        cancel.store(true, Ordering::Relaxed)
    })
    .unwrap();
    assert!(report.cancelled);
}
#[test]
fn spectrum_catalogue_constructor_refuses_other_platform() {
    let (_t, root, _) = fixture();
    let p = root.join("other.dat");
    std::fs::write(&p,"<datafile><header><name>Nintendo Game Boy</name></header><game name=\"X\"><rom name=\"x\" size=\"2\"/></game></datafile>").unwrap();
    let parsed = archivefs_core::dat::parsers::parse_dat_file(&p, Default::default()).unwrap();
    assert!(SpectrumCatalogue::from_parsed(&parsed.dat).is_err());
}

#[test]
fn spectrum_exact_archive_member_is_excluded_but_corrupt_archive_never_proves_absence() {
    use std::io::Write;
    let (t, root, c) = fixture();
    let bytes = snapshot();
    let path = root.join("indexed.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file(
        "different-name.z80",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(&bytes).unwrap();
    zip.finish().unwrap();
    let mut db = Database::open_or_create(t.path().join("disposable.sqlite3")).unwrap();
    archivefs_core::scan_and_persist(&mut db, &c, "zip-seed").unwrap();
    let known = db.load_archives().unwrap();
    std::fs::write(root.join("loose.z80"), &bytes).unwrap();
    let report = preview_media(
        &c,
        &[],
        &known,
        &[],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(report.count(MediaDisposition::RepresentedArchive), 1);
    assert_eq!(report.count(MediaDisposition::Eligible), 0);
    assert!(
        report
            .items
            .iter()
            .find(|i| i.path.ends_with("loose.z80"))
            .unwrap()
            .explanation
            .contains("SHA-256")
    );
    std::fs::write(&path, b"corrupt archive").unwrap();
    let report = preview_media(
        &c,
        &[],
        &known,
        &[],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(report.count(MediaDisposition::RepresentedArchive), 0);
    assert!(
        !report
            .items
            .iter()
            .find(|i| i.path.ends_with("loose.z80"))
            .unwrap()
            .relationship_checks_complete
    );
    assert!(!report.problems.is_empty());
}
#[test]
fn spectrum_preview_comparison_limit_is_truthful_and_stale_content_is_refused() {
    let (_t, root, c) = fixture();
    let path = root.join("over-budget.tap");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(archivefs_core::ingestion::preview::MAX_COMPARISON_INPUT_BYTES + 1)
        .unwrap();
    let report = preview_media(&c, &[], &[], &[], &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(report.charged_input_bytes, 0);
    assert!(!report.items[0].relationship_checks_complete);
    std::fs::remove_file(path).unwrap();
    let path = root.join("changing.z80");
    std::fs::write(&path, snapshot()).unwrap();
    let outcome = preview_media(&c, &[], &[], &[], &AtomicBool::new(false), &mut |_, _| {
        std::fs::write(&path, vec![255; 49182]).unwrap();
    });
    assert!(outcome.is_err());
}
#[cfg(unix)]
#[test]
fn spectrum_snapshot_inspection_refuses_symlinks_and_nonregular_files() {
    let (_t, root, _) = fixture();
    std::fs::write(root.join("real.z80"), snapshot()).unwrap();
    std::os::unix::fs::symlink(root.join("real.z80"), root.join("alias.z80")).unwrap();
    assert!(
        archivefs_core::zx_spectrum_snapshot::inspect_spectrum_snapshot_file(
            &root.join("alias.z80")
        )
        .is_err()
    );
    assert!(archivefs_core::zx_spectrum_snapshot::inspect_spectrum_snapshot_file(&root).is_err());
}

#[cfg(unix)]
#[test]
fn spectrum_unreadable_disk_is_partial_and_never_marks_existing_record_missing() {
    use std::os::unix::fs::PermissionsExt;
    for (name, bytes) in [("protected.trd", trd()), ("protected.scl", scl())] {
        let (t, root, c) = fixture();
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        let mut db = Database::open_or_create(t.path().join("disposable.sqlite3")).unwrap();
        archivefs_core::scan_and_persist(&mut db, &c, "seed-readable").unwrap();
        let id = db.load_archives().unwrap()[0].id;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
        assert!(
            std::fs::File::open(&path).is_err(),
            "permission fixture must actually refuse reads"
        );
        let summary = archivefs_core::scan_and_persist(&mut db, &c, "permission-refused").unwrap();
        let rows = db.load_archives().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert!(rows[0].last_verified_missing_at.is_none());
        assert_eq!(summary.counts.archives_missing, 0);
        assert!(!summary.folder_errors.is_empty());
        let inspected =
            archivefs_core::ingestion::discovery::inspect_spectrum_media(&path, &root).unwrap();
        assert!(matches!(
            inspected.skip_reason,
            Some(archivefs_core::ingestion::SkipReason::InspectionUnavailable(_))
        ));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn spectrum_preview_discards_archive_changed_after_member_comparison() {
    use std::io::Write;
    let (t, root, c) = fixture();
    let bytes = snapshot();
    let path = root.join("indexed.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file("member.z80", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&bytes).unwrap();
    zip.finish().unwrap();
    let mut db = Database::open_or_create(t.path().join("disposable.sqlite3")).unwrap();
    archivefs_core::scan_and_persist(&mut db, &c, "seed").unwrap();
    let known = db.load_archives().unwrap();
    std::fs::write(root.join("member.z80"), bytes).unwrap();
    let outcome = preview_media(
        &c,
        &[],
        &known,
        &[],
        &AtomicBool::new(false),
        &mut |n, total| {
            if n == total {
                std::fs::write(&path, b"changed after decoded member proof").unwrap();
            }
        },
    );
    assert!(outcome.is_err());
}
