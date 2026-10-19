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

fn browse_generated(bytes: &[u8], extension: &str, expected_family: AmigaDosFamily) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(format!("disposable.{extension}"));
    std::fs::write(&path, bytes).unwrap();
    let mut inspection = Inspection::open(&path).unwrap();
    let root = inspection.root(0).unwrap();
    let first = inspection.list(root, 0).unwrap();
    assert_eq!(first.filesystem.family, expected_family);
    assert_eq!(first.filesystem.block_size, 512);
    assert_eq!(first.filesystem.volume_label.as_deref(), Some("Work"));
    assert_eq!(first.entries.len(), PAGE_ENTRIES);
    let child = first.entries.iter().find_map(|e| e.directory).unwrap();
    assert!(inspection.list(child, 0).unwrap().entries.is_empty());
    let next = inspection.list(root, first.next_offset.unwrap()).unwrap();
    assert!(!next.entries.is_empty());
    assert_ne!(first.entries[0].name, next.entries[0].name);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn amiga_contents_rdb_hdfx_and_existing_hdf_reuse_validated_ofs_ffs_ranges() {
    for dos in 0..=7 {
        let family = if dos % 2 == 0 {
            AmigaDosFamily::Ofs
        } else {
            AmigaDosFamily::Ffs
        };
        for extension in ["rdb", "hdfx", "hdf", "adf"] {
            for wrapped in [false, true] {
                let flat = fixture::flat(dos, 150);
                let bytes = if wrapped { fixture::rdb(&flat) } else { flat };
                // Extension offers inspection only. Actual RDB/flat evidence,
                // offsets and filesystem validation decide the contents.
                browse_generated(&bytes, extension, family);
            }
        }
    }
}

#[test]
fn amiga_contents_raw_adf_dd_hd_ofs_ffs_use_the_existing_filesystem_contract() {
    for dos in 0..=7 {
        for high_density in [false, true] {
            let bytes = fixture::floppy(dos, 150, high_density);
            let source = tempfile::tempdir().unwrap();
            let path = source.path().join("disposable.adf");
            std::fs::write(&path, &bytes).unwrap();
            let evidence = crate::amiga_disk::inspect_amiga_floppy(&path).unwrap();
            assert!(evidence.filesystem().is_some());
            browse_generated(
                &bytes,
                "adf",
                if dos % 2 == 0 {
                    AmigaDosFamily::Ofs
                } else {
                    AmigaDosFamily::Ffs
                },
            );
        }
    }
}

#[test]
fn amiga_contents_custom_adf_non_amiga_and_unsupported_filesystems_never_list() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("custom.adf");
    let mut custom = vec![0u8; 901_120];
    custom[..4].copy_from_slice(b"DOS\0");
    fixture::put(&mut custom, 8, 4);
    std::fs::write(&path, &custom).unwrap();
    assert!(
        crate::amiga_disk::inspect_amiga_floppy(&path)
            .unwrap()
            .filesystem()
            .is_none()
    );
    let mut inspection = Inspection::open(&path).unwrap();
    assert!(inspection.root(0).is_err());
    for magic in [b"ADFS", b"DMS!", b"CAPS", b"UAE-"] {
        let mut bytes = custom.clone();
        bytes[..4].copy_from_slice(magic);
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            Inspection::open(&path).is_err(),
            "unsupported signature {magic:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    for signature in [b"PFS\x03", b"SFS\0", b"MuFS"] {
        let mut bytes = fixture::flat(1, 0);
        bytes[..4].copy_from_slice(signature);
        std::fs::write(&path, &bytes).unwrap();
        let mut inspection = Inspection::open(&path).unwrap();
        assert!(
            inspection
                .root(0)
                .unwrap_err()
                .contains("UnsupportedFilesystem")
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::write(&path, fixture::ffs_1024()).unwrap();
    let mut inspection = Inspection::open(&path).unwrap();
    let error = inspection.root(0).unwrap_err();
    assert!(
        error.contains("UnsupportedBlockSize") && error.contains("1024"),
        "{error}"
    );
}

#[test]
fn amiga_contents_new_representations_reject_changed_or_replaced_sources() {
    for extension in ["rdb", "hdfx", "adf"] {
        for replace in [false, true] {
            let source = tempfile::tempdir().unwrap();
            let path = source.path().join(format!("disposable.{extension}"));
            let bytes = if extension == "adf" {
                fixture::floppy(1, 2, false)
            } else {
                fixture::rdb(&fixture::flat(1, 2))
            };
            std::fs::write(&path, &bytes).unwrap();
            let mut inspection = Inspection::open(&path).unwrap();
            let root = inspection.root(0).unwrap();
            if replace {
                let replacement = path.with_extension("replacement");
                std::fs::write(&replacement, &bytes).unwrap();
                std::fs::rename(&replacement, &path).unwrap();
            } else {
                let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
                File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_times(
                        std::fs::FileTimes::new()
                            .set_modified(modified + std::time::Duration::from_secs(2)),
                    )
                    .unwrap();
            }
            assert!(!inspection.is_current());
            assert!(inspection.list(root, 0).is_err());
        }
    }
}

#[test]
fn amiga_contents_trace_separates_bounded_container_probe_from_file_data_traversal() {
    for dos in [0, 1] {
        // Sector2 is inside the16-sector container-probe prefix; sector47 is outside.
        // Both are explicit file-data pointers, never directory metadata.
        for data_sector in [2u32, 47] {
            for wrapped in [false, true] {
                let mut flat = fixture::flat(dos, 1);
                fixture::file_data_at(&mut flat, data_sector);
                let offset = if wrapped { flat.len() as u64 } else { 0 };
                let bytes = if wrapped { fixture::rdb(&flat) } else { flat };
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join(if wrapped {
                    "disposable.hdfx"
                } else {
                    "disposable.adf"
                });
                std::fs::write(&path, &bytes).unwrap();
                let ((), trace) = read_audit::capture(|| {
                    let mut inspection = Inspection::open(&path).unwrap();
                    let root = inspection.root(0).unwrap();
                    let page = inspection.list(root, 0).unwrap();
                    assert_eq!(page.entries.len(), 2);
                    assert_eq!(page.entries[1].size, Some(37));
                });
                let payload_start = offset + u64::from(data_sector) * 512;
                let overlaps = |&(start, len): &(u64, usize)| {
                    start < payload_start + 512 && payload_start < start + len as u64
                };
                assert!(
                    !trace.filesystem.iter().any(overlaps),
                    "filesystem followed file-data pointer: {trace:?}"
                );
                assert_eq!(
                    trace.container.iter().any(overlaps),
                    !wrapped && data_sector == 2
                );
                assert!(
                    trace
                        .container
                        .iter()
                        .all(|&(start, len)| start + len as u64 <= 16 * 512
                            || wrapped && start == offset && len == 4),
                    "container metadata probe exceeded its existing ranges: {trace:?}"
                );
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
        }
    }
}
