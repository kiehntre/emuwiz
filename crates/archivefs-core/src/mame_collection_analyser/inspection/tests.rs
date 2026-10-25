use super::*;
use std::io::Write;
use zip::write::SimpleFileOptions;

fn zip_set(path: &Path, members: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in members {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}
fn scan(root: &Path) -> CollectionInspection {
    inspect_collection(
        root,
        InspectionLimits::default(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap()
}

#[test]
fn zip_and_loose_members_hash_real_bytes_without_source_writes() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("zipset.zip");
    zip_set(&zip, &[("main.bin", b"hello world")]);
    fs::create_dir(dir.path().join("looseset")).unwrap();
    fs::write(dir.path().join("looseset/main.bin"), b"hello world").unwrap();
    let before = fs::read(&zip).unwrap();
    let report = scan(dir.path());
    assert!(report.inventory.inspected_completely);
    assert_eq!(report.inventory.sets.len(), 2);
    for set in &report.inventory.sets {
        assert_eq!(
            set.members[0].sha1.as_deref(),
            Some("2aae6c35c94fcfb415dbe95f408b9ce91ee846ed")
        );
        assert_eq!(set.members[0].size_bytes, Some(11));
    }
    assert_eq!(fs::read(zip).unwrap(), before);
    assert_eq!(
        fs::read(dir.path().join("looseset/main.bin")).unwrap(),
        b"hello world"
    );
}
#[test]
fn corrupt_zip_is_partial_and_never_supplies_hashes() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("g.zip"), b"not a zip").unwrap();
    let report = scan(dir.path());
    assert!(!report.inventory.inspected_completely);
    assert!(report.inventory.sets[0].members.is_empty());
    assert!(report.inventory.warnings[0].contains("Cannot verify ZIP"));
}
#[test]
fn chd_companion_is_presence_only_and_never_ordinary_file_verification() {
    let dir = tempfile::tempdir().unwrap();
    zip_set(&dir.path().join("g.zip"), &[("main.bin", b"hello world")]);
    fs::create_dir(dir.path().join("g")).unwrap();
    fs::write(dir.path().join("g/disk.chd"), b"misleading ordinary bytes").unwrap();
    let report = scan(dir.path());
    assert!(report.inventory.inspected_completely);
    assert_eq!(report.inventory.sets.len(), 1);
    let disk = report.inventory.sets[0]
        .members
        .iter()
        .find(|m| m.name == "disk.chd")
        .unwrap();
    assert!(disk.sha1.is_none() && disk.md5.is_none() && disk.crc32.is_none());
    assert_eq!(report.logical_bytes, 11);
}
#[test]
fn competing_rom_containers_are_retained_as_ambiguous_evidence() {
    let dir = tempfile::tempdir().unwrap();
    zip_set(&dir.path().join("g.zip"), &[("main.bin", b"hello world")]);
    fs::create_dir(dir.path().join("g")).unwrap();
    fs::write(dir.path().join("g/main.bin"), b"hello world").unwrap();
    let report = scan(dir.path());
    assert_eq!(
        report
            .inventory
            .sets
            .iter()
            .filter(|s| s.name == "g")
            .count(),
        2
    );
}
#[test]
fn pre_cancelled_and_mid_scan_cancellation_are_partial() {
    let dir = tempfile::tempdir().unwrap();
    zip_set(&dir.path().join("g.zip"), &[("main.bin", b"hello world")]);
    for pre_cancel in [true, false] {
        let cancel = AtomicBool::new(pre_cancel);
        let report = inspect_collection(
            dir.path(),
            InspectionLimits::default(),
            &cancel,
            &mut |_| {
                cancel.store(true, Ordering::Relaxed);
            },
        )
        .unwrap();
        assert!(report.cancelled);
        assert!(!report.inventory.inspected_completely);
        assert!(
            report
                .inventory
                .sets
                .iter()
                .flat_map(|s| &s.members)
                .all(|m| m.sha1.is_none())
        );
    }
}
#[test]
fn entry_and_logical_limits_refuse_complete_claims() {
    let dir = tempfile::tempdir().unwrap();
    zip_set(&dir.path().join("g.zip"), &[("main.bin", b"hello world")]);
    for limits in [
        InspectionLimits {
            max_entries: 1,
            ..Default::default()
        },
        InspectionLimits {
            max_logical_bytes: 1,
            ..Default::default()
        },
    ] {
        let report =
            inspect_collection(dir.path(), limits, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(!report.inventory.inspected_completely);
        assert!(
            report
                .inventory
                .sets
                .iter()
                .flat_map(|s| &s.members)
                .all(|m| m.sha1.is_none())
        );
    }
}
#[test]
fn source_replacement_after_hashing_invalidates_every_verified_member() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.zip");
    zip_set(&path, &[("main.bin", b"hello world")]);
    let report = inspect_collection(
        dir.path(),
        InspectionLimits::default(),
        &AtomicBool::new(false),
        &mut |_| {
            fs::write(&path, b"changed after inspection").unwrap();
        },
    )
    .unwrap();
    assert!(!report.inventory.inspected_completely);
    assert!(report.inventory.sets[0].members[0].sha1.is_none());
    assert!(
        report
            .inventory
            .warnings
            .iter()
            .any(|w| w.contains("source changed"))
    );
}
#[test]
fn empty_collection_is_a_complete_empty_inventory_not_fake_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let report = scan(dir.path());
    assert!(report.inventory.inspected_completely);
    assert!(report.inventory.sets.is_empty());
    assert_eq!(report.logical_bytes, 0);
}
#[test]
fn unsupported_root_files_and_nested_archives_are_visible_refusals() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("g.7z"), b"not opened").unwrap();
    fs::create_dir(dir.path().join("loose")).unwrap();
    fs::write(dir.path().join("loose/nested.zip"), b"not opened").unwrap();
    let report = scan(dir.path());
    assert!(!report.inventory.inspected_completely);
    assert_eq!(report.logical_bytes, 0);
    assert!(
        report
            .inventory
            .warnings
            .iter()
            .any(|w| w.contains("Unsupported file"))
    );
    assert!(
        report
            .inventory
            .warnings
            .iter()
            .any(|w| w.contains("Nested archives"))
    );
}
#[cfg(unix)]
#[test]
fn symlink_and_special_files_are_refused_without_following_targets() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("g.zip"), b"do not inspect").unwrap();
    std::os::unix::fs::symlink(outside.path().join("g.zip"), dir.path().join("g.zip")).unwrap();
    let report = scan(dir.path());
    assert!(!report.inventory.inspected_completely);
    assert!(report.inventory.sets.is_empty());
    assert_eq!(report.logical_bytes, 0);
    assert!(report.inventory.warnings[0].contains("Symbolic links"));
}
