use super::*;
#[path = "hdf_contents_fixtures.rs"]
mod fixture;

fn open(bytes: &[u8]) -> (tempfile::TempDir, PathBuf, Inspection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disposable.hdf");
    std::fs::write(&path, bytes).unwrap();
    let inspection = Inspection::open(&path).unwrap();
    (dir, path, inspection)
}

#[test]
fn hdf_contents_ofs_ffs_and_rdb_labels_list_without_changing_bytes() {
    for dos in [0, 1] {
        for wrapped in [false, true] {
            let flat = fixture::flat(dos, 2);
            let bytes = if wrapped { fixture::rdb(&flat) } else { flat };
            let (_dir, path, mut inspection) = open(&bytes);
            assert_eq!(
                inspection.disk().rdb.partitions[0].name.as_deref(),
                wrapped.then_some("DH0")
            );
            let root = inspection.root(0).unwrap();
            let page = inspection.list(root, 0).unwrap();
            assert_eq!(page.filesystem.volume_label.as_deref(), Some("Work"));
            assert_eq!(page.entries.len(), 3);
            assert_eq!(page.entries[1].size, Some(37));
            let directory = page.entries[0].directory.unwrap();
            assert!(inspection.list(directory, 0).unwrap().entries.is_empty());
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn hdf_contents_pages_are_lazy_and_remainder_limits_are_explicit() {
    let (_dir, _path, mut inspection) = open(&fixture::flat(1, 4200));
    let root = inspection.root(0).unwrap();
    let first = inspection.list(root, 0).unwrap();
    assert_eq!(first.entries.len(), PAGE_ENTRIES);
    assert_eq!(first.next_offset, Some(PAGE_ENTRIES));
    let second = inspection.list(root, PAGE_ENTRIES).unwrap();
    assert_ne!(first.entries[0].name, second.entries[0].name);
    let last = inspection
        .list(root, MAX_DIRECTORY_ENTRIES - PAGE_ENTRIES)
        .unwrap();
    assert_eq!(last.next_offset, None);
    assert!(last.limitation.unwrap().contains("4096"));
    assert!(inspection.list(root, MAX_DIRECTORY_ENTRIES).is_err());
}

#[test]
fn hdf_contents_does_not_traverse_unrequested_corrupt_subdirectories() {
    let mut bytes = fixture::flat(1, 1);
    let sub = &mut bytes[8 * 512..9 * 512];
    fixture::put(sub, 24, 999_999);
    fixture::checksum(sub, 20);
    let (_dir, _path, mut inspection) = open(&bytes);
    let root = inspection.root(0).unwrap();
    let page = inspection.list(root, 0).unwrap();
    assert_eq!(page.entries.len(), 2);
    assert!(
        inspection
            .list(page.entries[0].directory.unwrap(), 0)
            .is_err()
    );
}

#[test]
fn hdf_contents_rejects_cycle_bad_checksum_and_parent_conflicts() {
    for change in [0, 1, 2, 3] {
        let mut bytes = fixture::flat(1, 1);
        let block = &mut bytes[10 * 512..11 * 512];
        match change {
            0 => fixture::put(block, 496, 10),
            1 => fixture::put(block, 0, 42),
            2 => fixture::put(block, 500, 999),
            _ => fixture::put(block, 508, 99),
        }
        fixture::checksum(block, 20);
        let (_dir, _path, mut inspection) = open(&bytes);
        let root = inspection.root(0).unwrap();
        assert!(inspection.list(root, 0).is_err());
    }
}

#[test]
fn hdf_contents_unsupported_is_detection_only_and_links_are_not_followed() {
    let mut unsupported = fixture::flat(1, 0);
    unsupported[..4].copy_from_slice(b"PFS\x03");
    let (_dir, _path, mut inspection) = open(&unsupported);
    assert!(
        inspection
            .root(0)
            .unwrap_err()
            .contains("UnsupportedFilesystem")
    );
    let mut bytes = fixture::flat(1, 1);
    let link = &mut bytes[8 * 512..9 * 512];
    fixture::put(link, 508, 3);
    fixture::checksum(link, 20);
    let (_dir, _path, mut inspection) = open(&bytes);
    let root = inspection.root(0).unwrap();
    let page = inspection.list(root, 0).unwrap();
    assert!(page.entries[0].directory.is_none());
    assert!(page.entries[0].kind.contains("not followed"));
}

#[test]
fn hdf_contents_rejects_damaged_rdb_and_conflicting_partition_type() {
    for change in [0, 1] {
        let mut bytes = fixture::rdb(&fixture::flat(1, 0));
        if change == 0 {
            bytes[8] ^= 1;
        } else {
            fixture::put(&mut bytes, 512 + 192, 0x444f5300);
            fixture::checksum(&mut bytes[512..1024], 8);
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.hdf");
        std::fs::write(&path, bytes).unwrap();
        assert!(Inspection::open(&path).is_err());
    }
}

#[test]
fn hdf_contents_changes_and_replacements_invalidate_directory_tokens() {
    for replace in [false, true] {
        let (_dir, path, mut inspection) = open(&fixture::flat(1, 1));
        let root = inspection.root(0).unwrap();
        if replace {
            let replacement = path.with_extension("replacement");
            std::fs::write(&replacement, fixture::flat(1, 1)).unwrap();
            std::fs::rename(replacement, &path).unwrap();
        } else {
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            std::fs::write(&path, fixture::flat(1, 2)).unwrap();
            // Consecutive writes can share the host timestamp quantum. Make
            // this same-inode, same-size mutation observably different.
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(modified + std::time::Duration::from_secs(1)),
                )
                .unwrap();
        }
        assert!(!inspection.is_current());
        assert!(inspection.list(root, 0).is_err());
    }
}

#[test]
fn hdf_contents_rejects_nonregular_and_unsafe_sources() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Inspection::open(dir.path()).is_err());
    let missing = dir.path().join("missing.hdf");
    assert!(Inspection::open(&missing).is_err());
    let (_dir, path, _inspection) = open(&fixture::flat(1, 0));
    let link = dir.path().join("link.hdf");
    std::os::unix::fs::symlink(path, &link).unwrap();
    assert!(Inspection::open(&link).is_err());
}

#[test]
fn hdf_contents_case_ambiguous_names_are_refused() {
    let mut bytes = fixture::flat(1, 2);
    let second = &mut bytes[11 * 512..12 * 512];
    second[433..441].copy_from_slice(b"FILE0000");
    fixture::checksum(second, 20);
    let (_dir, _path, mut inspection) = open(&bytes);
    let root = inspection.root(0).unwrap();
    assert!(inspection.list(root, 0).unwrap_err().contains("Ambiguous"));
}

#[test]
fn hdf_contents_multiple_partitions_are_independent_and_overlap_is_refused() {
    let bytes = fixture::two_partitions();
    let (_dir, _path, mut inspection) = open(&bytes);
    assert_eq!(inspection.disk().rdb.partitions.len(), 2);
    for index in 0..2 {
        let root = inspection.root(index).unwrap();
        assert_eq!(inspection.list(root, 0).unwrap().entries.len(), 3);
    }
    let mut overlapping = bytes;
    let part = &mut overlapping[1024..1536];
    fixture::put(part, 164, 1);
    fixture::put(part, 168, 1);
    fixture::checksum(part, 8);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("overlap.hdf");
    std::fs::write(&path, overlapping).unwrap();
    assert!(Inspection::open(&path).is_err());
}
