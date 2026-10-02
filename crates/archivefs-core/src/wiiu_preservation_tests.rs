//! Synthetic containers only. No image or key material is checked in.
use super::*;
use crate::repair::execute::RepairExecutionOptions;
use crate::wiiu_conversion::*;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::tempdir;

fn wud_header() -> Vec<u8> {
    let mut h = vec![0; WUD_HEADER_SIZE as usize];
    h[..10].copy_from_slice(b"WUP-P-TEST");
    h[0x10000..0x10004].copy_from_slice(&DISC_MAGIC.to_be_bytes());
    h[0x10005] = 1;
    h[0x10006] = 2;
    h[0x10020..0x10029].copy_from_slice(b"synthetic");
    h[0x18000..].fill(0xa5); // opaque region; no claim that these bytes encrypt anything
    h
}
fn raw(path: &Path, size: u64) {
    let mut f = File::create(path).unwrap();
    f.write_all(&wud_header()).unwrap();
    f.set_len(size).unwrap();
}
fn header(sector: u32, logical: u64) -> [u8; 32] {
    let mut h = [0; 32];
    h[..4].copy_from_slice(b"WUX0");
    h[4..8].copy_from_slice(&MAGIC1.to_le_bytes());
    h[8..12].copy_from_slice(&sector.to_le_bytes());
    h[16..24].copy_from_slice(&logical.to_le_bytes());
    h
}
fn container(path: &Path, sector: u32, logical: u64, entries: &[u32], stored: &[Vec<u8>]) {
    let mut f = File::create(path).unwrap();
    f.write_all(&header(sector, logical)).unwrap();
    for e in entries {
        f.write_all(&e.to_le_bytes()).unwrap();
    }
    let offset = checked_align(32 + entries.len() as u64 * 4, sector as u64).unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    for bytes in stored {
        assert_eq!(bytes.len(), sector as usize);
        f.write_all(bytes).unwrap();
    }
}
/// Reordered physical blocks, repeated stored zero block and repeated content.
fn fixture(path: &Path, blocks: usize) -> Vec<u8> {
    assert!(blocks >= 8);
    let sector = WUD_SECTOR_SIZE as usize;
    let h = wud_header();
    let stored = vec![
        vec![0; sector],
        vec![0x5a; sector],
        h[2 * sector..3 * sector].to_vec(),
        h[..sector].to_vec(),
        h[3 * sector..4 * sector].to_vec(),
    ];
    let mut entries = vec![0; blocks];
    entries[..8].copy_from_slice(&[3, 0, 2, 4, 1, 0, 1, 0]);
    container(
        path,
        sector as u32,
        (blocks * sector) as u64,
        &entries,
        &stored,
    );
    // Expected output stays small even for the large streaming fixture.
    entries[..8]
        .iter()
        .flat_map(|i| stored[*i as usize].iter().copied())
        .collect()
}
fn request(source: &Path, destination: &Path) -> WiiUConversionRequest {
    WiiUConversionRequest {
        source: source.into(),
        destination: destination.into(),
        direction: WiiUConversionDirection::WuxToWud,
        source_identity: WiiUConversionIdentity::HashMissing,
        available_free_space: Some(MAX_LOGICAL_SIZE),
        tools: WiiUConversionToolInventory::default(),
    }
}
fn options(root: &Path) -> RepairExecutionOptions {
    RepairExecutionOptions {
        trusted: crate::safe_read::TrustedRoots::from_paths([root]),
        journal_dir: root.join("journal"),
        audit_cache: crate::dat::sources::audit_cache::AuditCacheConfig::Disabled,
    }
}
fn stages(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|x| x.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".emuwiz-wiiu-")
        })
        .collect()
}
fn edit(path: &Path, offset: u64, bytes: &[u8]) {
    let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    f.write_all(bytes).unwrap();
}

#[test]
fn wud_minimal_header_identity_and_extension_mismatch() {
    let d = tempdir().unwrap();
    for name in ["disc.wud", "disc.wux", "disc.bin", "disc.wua"] {
        let p = d.path().join(name);
        raw(&p, WUD_HEADER_SIZE);
        let before = std::fs::read(&p).unwrap();
        let r = inspect_wii_u_disc(&p);
        assert!(r.structural_complete, "{r:?}");
        assert_eq!(r.format, WiiUDiscFormat::Wud);
        let s = r.structure.unwrap();
        assert!(!s.retail_size_matches);
        let h = s.wud_header.unwrap();
        assert_eq!(h.manufacturer_disc_id.as_deref(), Some("WUP-P-TEST"));
        assert_eq!((h.major_version, h.minor_version), (1, 2));
        assert_eq!(h.footprint.as_deref(), Some("synthetic"));
        assert_eq!(h.partition_table, WiiUPartitionEvidence::EncryptedOrOpaque);
        assert_eq!(std::fs::read(&p).unwrap(), before);
    }
}
#[test]
fn wud_truncated_header_body_and_absurd_extent_fail_closed() {
    let d = tempdir().unwrap();
    let p = d.path().join("bad.wud");
    for size in [
        0,
        4,
        0x10000,
        0x10003,
        0x10008,
        WUD_HEADER_SIZE - 1,
        MAX_LOGICAL_SIZE + WUD_SECTOR_SIZE as u64,
    ] {
        raw(&p, size);
        let r = inspect_wii_u_disc(&p);
        assert!(!r.structural_complete, "accepted {size}: {r:?}");
    }
    std::fs::write(&p, vec![7; WUD_HEADER_SIZE as usize]).unwrap();
    assert_eq!(inspect_wii_u_disc(&p).format, WiiUDiscFormat::Unknown);
    assert!(
        inspect_wii_u_disc(&p)
            .issues
            .contains(&WiiUDiscIssue::InvalidMagic)
    );
}
#[test]
fn wud_retail_sparse_extent_inspection_is_bounded() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.data");
    raw(&p, RETAIL_WUD_SIZE);
    let r = inspect_wii_u_disc(&p);
    assert!(r.structural_complete);
    assert!(r.structure.unwrap().retail_size_matches);
    assert_eq!(r.source_evidence.unwrap().size_bytes, RETAIL_WUD_SIZE);
}
#[test]
fn wud_magic_does_not_require_a_manufacturer_string() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    raw(&p, WUD_HEADER_SIZE);
    edit(&p, 0, &[0; 32]);
    let r = inspect_wii_u_disc(&p);
    assert!(r.structural_complete);
    assert_eq!(r.format, WiiUDiscFormat::Wud);
    assert!(
        r.structure
            .unwrap()
            .wud_header
            .unwrap()
            .manufacturer_disc_id
            .is_none()
    );
}
#[test]
fn plaintext_partition_table_evidence_and_bounds() {
    let d = tempdir().unwrap();
    let p = d.path().join("plain.wud");
    raw(&p, WUD_HEADER_SIZE + WUD_SECTOR_SIZE as u64);
    edit(&p, 0x18000, &[0; 0x8000]);
    edit(&p, 0x18000, &CONTENTS_MAGIC.to_be_bytes());
    edit(&p, 0x18004, &WUD_SECTOR_SIZE.to_be_bytes());
    edit(&p, 0x1801c, &1u32.to_be_bytes());
    edit(&p, 0x18800, b"SI");
    edit(&p, 0x1881f, &[1]);
    edit(&p, 0x18820, &4u32.to_be_bytes());
    let r = inspect_wii_u_disc(&p);
    assert!(r.structural_complete, "{r:?}");
    assert_eq!(
        r.structure.unwrap().wud_header.unwrap().partition_table,
        WiiUPartitionEvidence::Plaintext {
            block_size: WUD_SECTOR_SIZE,
            partitions: vec![WiiUPartitionFact {
                volume_id: Some("SI".into()),
                volume_offsets: vec![WUD_HEADER_SIZE]
            }]
        }
    );
    for (offset, bytes) in [
        (0x1801c, u32::MAX.to_be_bytes().to_vec()),
        (0x1881f, vec![9]),
        (0x18820, u32::MAX.to_be_bytes().to_vec()),
    ] {
        edit(&p, offset, &bytes);
        assert!(!inspect_wii_u_disc(&p).structural_complete);
        edit(&p, 0x1801c, &1u32.to_be_bytes());
        edit(&p, 0x1881f, &[1]);
        edit(&p, 0x18820, &4u32.to_be_bytes());
    }
}
#[test]
fn wux_valid_repeated_zero_and_content_mappings() {
    let d = tempdir().unwrap();
    for name in ["disc.wux", "disc.wud", "disc.data", "disc.wua"] {
        let p = d.path().join(name);
        fixture(&p, 8);
        let r = inspect_wii_u_disc(&p);
        assert!(r.structural_complete, "{r:?}");
        assert_eq!(r.format, WiiUDiscFormat::Wux);
        let s = r.structure.unwrap();
        assert_eq!(s.block_count, Some(8));
        assert_eq!(s.referenced_block_count, Some(5));
        assert_eq!(s.repeated_block_count, Some(3));
        assert_eq!(
            s.wud_header.unwrap().manufacturer_disc_id.as_deref(),
            Some("WUP-P-TEST")
        );
    }
}
#[test]
fn wux_magic_table_and_body_truncation_fail_closed() {
    let d = tempdir().unwrap();
    let p = d.path().join("bad.wux");
    for size in [
        4,
        8,
        31,
        32,
        48,
        WUD_SECTOR_SIZE as u64 - 1,
        WUD_SECTOR_SIZE as u64 + 17,
    ] {
        fixture(&p, 8);
        File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(size)
            .unwrap();
        assert!(
            !inspect_wii_u_disc(&p).structural_complete,
            "accepted truncation at {size}"
        );
    }
    fixture(&p, 8);
    edit(&p, 4, &[0; 4]);
    assert!(
        inspect_wii_u_disc(&p)
            .issues
            .contains(&WiiUDiscIssue::InvalidMagic)
    );
    fixture(&p, 8);
    edit(&p, 0, b"NOPE");
    assert!(!inspect_wii_u_disc(&p).structural_complete);
}
#[test]
fn wux_invalid_entries_are_not_sparse_sentinels() {
    let d = tempdir().unwrap();
    let p = d.path().join("bad.wux");
    for index in [5, 8, u32::MAX] {
        fixture(&p, 8);
        edit(&p, WUX_HEADER + 7 * 4, &index.to_le_bytes());
        assert!(
            inspect_wii_u_disc(&p)
                .issues
                .iter()
                .any(|i| matches!(i, WiiUDiscIssue::BlockOutsideContainer { .. }))
        );
    }
    fixture(&p, 8);
    edit(&p, WUX_HEADER + 2 * 4, &0u32.to_le_bytes());
    // Index zero points at real stored data; a zero-filled disc ID is invalid.
    assert!(!inspect_wii_u_disc(&p).structural_complete);
}
#[test]
fn wux_impossible_sizes_overflow_table_limits_and_flags() {
    let d = tempdir().unwrap();
    let p = d.path().join("bad.wux");
    for logical in [
        0,
        WUD_HEADER_SIZE - 1,
        WUD_HEADER_SIZE + 1,
        MAX_LOGICAL_SIZE + WUD_SECTOR_SIZE as u64,
        u64::MAX,
    ] {
        std::fs::write(&p, header(WUD_SECTOR_SIZE, logical)).unwrap();
        assert!(!inspect_wii_u_disc(&p).structural_complete);
    }
    assert!(
        inspect_wii_u_disc(&p)
            .issues
            .contains(&WiiUDiscIssue::LogicalSizeOverflow)
    );
    std::fs::write(&p, header(0x100, MAX_LOGICAL_SIZE)).unwrap();
    assert!(
        inspect_wii_u_disc(&p)
            .issues
            .iter()
            .any(|i| matches!(i, WiiUDiscIssue::AbsurdBlockCount(_)))
    );
    for sector in [0, 1, 0xff, 0x1000_0000, u32::MAX] {
        std::fs::write(&p, header(sector, WUD_HEADER_SIZE)).unwrap();
        assert!(
            inspect_wii_u_disc(&p)
                .issues
                .contains(&WiiUDiscIssue::InvalidSectorSize(sector))
        );
    }
    fixture(&p, 8);
    edit(&p, 24, &1u32.to_le_bytes());
    assert!(
        inspect_wii_u_disc(&p)
            .issues
            .contains(&WiiUDiscIssue::UnsupportedFlags(1))
    );
    assert_eq!(checked_align(u64::MAX, 32), None);
    assert_eq!(checked_align(32, 0), None);
    assert_eq!(checked_align(35, 3), Some(36));
}
#[test]
fn split_missing_duplicate_and_hostile_numbers_are_bounded_refusals() {
    let d = tempdir().unwrap();
    let p = d.path().join("title_part1.wud");
    raw(&p, WUD_HEADER_SIZE);
    raw(&d.path().join("title_part3.wud"), WUD_HEADER_SIZE);
    let r = inspect_wii_u_disc(&p);
    assert!(!r.structural_complete);
    assert!(
        r.issues
            .contains(&WiiUDiscIssue::SplitMissingPart { index: 2 })
    );
    raw(&d.path().join("title.part1.wud"), WUD_HEADER_SIZE);
    assert!(
        inspect_wii_u_disc(&p)
            .issues
            .contains(&WiiUDiscIssue::SplitDuplicatePart { index: 1 })
    );
    let huge = d.path().join("title_part4294967295.wud");
    raw(&huge, WUD_HEADER_SIZE);
    assert!(!inspect_wii_u_disc(&huge).structural_complete);
}
#[test]
fn preview_is_read_only_native_and_no_clobber() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let before = std::fs::read(&p).unwrap();
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    assert_eq!(
        plan.readiness,
        WiiUConversionReadiness::ReadyToPreview,
        "{plan:?}"
    );
    assert!(plan.refusals.is_empty());
    assert!(plan.tool.is_none());
    assert!(plan.no_clobber);
    assert_eq!(
        plan.space.destination_exact_bytes,
        Some(8 * WUD_SECTOR_SIZE as u64)
    );
    assert_eq!(
        plan.space.temporary_bytes,
        plan.space.destination_exact_bytes
    );
    assert_eq!(plan.space.atomic_duplicate_bytes, Some(0));
    assert_eq!(plan.estimated_blocks, Some(8));
    assert!(plan.post_write_verification_available);
    assert!(!plan.keys_required);
    assert_eq!(std::fs::read(&p).unwrap(), before);
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
}
#[test]
fn preview_refuses_space_stale_hash_unsafe_path_and_wrong_direction() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let mut req = request(&p, &dest);
    req.available_free_space = Some(1);
    assert!(plan_wiiu_conversion(&req).refusals.iter().any(|r| matches!(
        r,
        WiiUConversionRefusal::InsufficientDestinationSpace { .. }
    )));
    req.available_free_space = Some(MAX_LOGICAL_SIZE);
    req.source_identity = WiiUConversionIdentity::HashStale;
    assert!(
        plan_wiiu_conversion(&req)
            .refusals
            .contains(&WiiUConversionRefusal::HashStale)
    );
    req.source_identity = WiiUConversionIdentity::HashMissing;
    req.destination = d.path().join("missing/out.wud");
    assert!(
        plan_wiiu_conversion(&req)
            .refusals
            .contains(&WiiUConversionRefusal::DestinationPathUnsafe)
    );
    req.destination = dest;
    req.direction = WiiUConversionDirection::WudToWux;
    assert!(
        plan_wiiu_conversion(&req)
            .refusals
            .iter()
            .any(|r| matches!(r, WiiUConversionRefusal::WrongSourceFormat { .. }))
    );
    assert_eq!(
        plan_wiiu_conversion(&req).readiness,
        WiiUConversionReadiness::NotReady
    );
}
#[test]
fn conversion_byte_exact_size_zero_regions_header_hash_and_provenance() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    let expected = fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let before = std::fs::read(&p).unwrap();
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    let (record, applied) = execute_wiiu_conversion(
        &plan,
        &options(d.path()),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), expected);
    assert_eq!(record.output_bytes, expected.len() as u64);
    assert!(
        expected[5 * WUD_SECTOR_SIZE as usize..6 * WUD_SECTOR_SIZE as usize]
            .iter()
            .all(|b| *b == 0)
    );
    assert_eq!(record.output_sha256, digest_hex(Sha256::digest(&expected)));
    assert_eq!(record.output_sha256, record.reconstructed_wud_sha256);
    assert!(inspect_wii_u_disc(&dest).structural_complete);
    assert_eq!(std::fs::read(&p).unwrap(), before);
    assert_eq!(applied.summary.applied, 1);
    assert_eq!(
        applied.transaction.unknown["wiiu_conversion"]["output_sha256"],
        record.output_sha256
    );
    let (journals, errors) =
        crate::dat::rename_apply::journal::list_journals(&d.path().join("journal"));
    assert!(errors.is_empty());
    assert_eq!(
        journals[0].unknown["wiiu_conversion"]["source_retained"],
        true
    );
    // Retained empty stage is the existing transaction's rollback destination.
    assert_eq!(stages(d.path()).len(), 1);
    assert_eq!(std::fs::read_dir(&stages(d.path())[0]).unwrap().count(), 0);
}
#[test]
fn stale_preview_refuses_changed_header_table_body_size_and_replacement() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    let dest = d.path().join("disc.wud");
    for offset in [
        32,
        WUD_SECTOR_SIZE as u64 * 3 + 5,
        WUD_SECTOR_SIZE as u64 * 2 + 2,
    ] {
        fixture(&p, 8);
        let plan = plan_wiiu_conversion(&request(&p, &dest));
        edit(&p, offset, &[0x42]);
        assert!(
            execute_wiiu_conversion(
                &plan,
                &options(d.path()),
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .is_err()
        );
        assert!(!dest.exists());
        assert!(stages(d.path()).is_empty());
    }
    fixture(&p, 8);
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(32)
        .unwrap();
    assert!(matches!(
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        ),
        Err(WiiUConversionError::StalePlan)
    ));
    fixture(&p, 8);
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    let same = std::fs::read(&p).unwrap();
    std::fs::rename(&p, d.path().join("retained")).unwrap();
    std::fs::write(&p, same).unwrap();
    assert!(matches!(
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        ),
        Err(WiiUConversionError::StalePlan)
    ));
}
#[cfg(unix)]
#[test]
fn restored_mtime_does_not_hide_a_stale_table() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    edit(&p, 32 + 7 * 4, &1u32.to_le_bytes());
    File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(
                plan.source_inspection
                    .source_evidence
                    .as_ref()
                    .unwrap()
                    .modified,
            ),
        )
        .unwrap();
    assert!(matches!(
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        ),
        Err(WiiUConversionError::StalePlan)
    ));
}
#[test]
fn existing_destination_before_and_after_preview_is_never_overwritten() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    std::fs::write(&dest, b"kept").unwrap();
    assert!(
        plan_wiiu_conversion(&request(&p, &dest))
            .refusals
            .contains(&WiiUConversionRefusal::DestinationExists)
    );
    assert!(
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"kept");
    assert!(stages(d.path()).is_empty());
}
#[cfg(unix)]
#[test]
fn dangling_destination_and_symlinked_source_or_parent_are_refused() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    std::os::unix::fs::symlink(d.path().join("missing"), &dest).unwrap();
    assert!(
        plan_wiiu_conversion(&request(&p, &dest))
            .refusals
            .contains(&WiiUConversionRefusal::DestinationExists)
    );
    let link = d.path().join("link.wux");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert!(!inspect_wii_u_disc(&link).structural_complete);
    let parent = d.path().join("linked-parent");
    std::os::unix::fs::symlink(d.path(), &parent).unwrap();
    assert!(
        plan_wiiu_conversion(&request(&p, &parent.join("out.wud")))
            .refusals
            .contains(&WiiUConversionRefusal::DestinationPathUnsafe)
    );
}
#[test]
fn interrupted_decode_cleans_stage_and_never_publishes() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    let cancel = AtomicBool::new(false);
    let mut chunks = 0;
    let error = execute_wiiu_conversion(&plan, &options(d.path()), &cancel, &mut |_| {
        chunks += 1;
        cancel.store(true, Ordering::Relaxed);
    })
    .unwrap_err();
    assert!(matches!(error, WiiUConversionError::Cancelled));
    assert_eq!(chunks, 1);
    assert!(!dest.exists());
    assert!(stages(d.path()).is_empty());
    assert!(!d.path().join("journal").exists());
}
#[test]
fn premature_eof_and_source_mutation_during_decode_never_publish() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    let dest = d.path().join("disc.wud");
    for truncate in [true, false] {
        fixture(&p, 8);
        let plan = plan_wiiu_conversion(&request(&p, &dest));
        let mut once = false;
        let r = execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {
                if !once {
                    once = true;
                    if truncate {
                        File::options()
                            .write(true)
                            .open(&p)
                            .unwrap()
                            .set_len(32)
                            .unwrap();
                    } else {
                        edit(&p, WUD_SECTOR_SIZE as u64 * 2 + 9, &[0x99]);
                    }
                }
            },
        );
        assert!(r.is_err());
        assert!(!dest.exists());
        assert!(stages(d.path()).is_empty());
    }
}
#[test]
fn corrupted_staged_output_and_late_destination_never_publish_success() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    let dest = d.path().join("disc.wud");
    for corrupt in [true, false] {
        fixture(&p, 8);
        let plan = plan_wiiu_conversion(&request(&p, &dest));
        let r = execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |progress| {
                if progress.written_bytes == progress.expected_bytes {
                    if corrupt {
                        edit(&stages(d.path())[0].join("output.wud"), 0x10000, &[0; 4]);
                    } else {
                        std::fs::write(&dest, b"appeared").unwrap();
                    }
                }
            },
        );
        assert!(r.is_err());
        assert!(stages(d.path()).is_empty());
        if corrupt {
            assert!(!dest.exists());
        } else {
            assert_eq!(std::fs::read(&dest).unwrap(), b"appeared");
        }
    }
}
#[test]
fn malformed_source_and_tampered_plan_never_publish() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    let dest = d.path().join("disc.wud");
    fixture(&p, 8);
    edit(&p, 32, &u32::MAX.to_le_bytes());
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    assert!(
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .is_err()
    );
    fixture(&p, 8);
    let mut plan = plan_wiiu_conversion(&request(&p, &dest));
    plan.space.destination_exact_bytes = Some(1);
    assert!(
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .is_err()
    );
    assert!(!dest.exists());
    assert!(stages(d.path()).is_empty());
}
#[test]
fn source_container_hash_is_verified_and_not_confused_with_wud_hash() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let mut req = request(&p, &dest);
    req.source_identity = WiiUConversionIdentity::HashAvailable {
        algorithm: "sha256".into(),
        value: "0".repeat(64),
    };
    let plan = plan_wiiu_conversion(&req);
    assert!(matches!(
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        ),
        Err(WiiUConversionError::StalePlan)
    ));
    req.source_identity = WiiUConversionIdentity::HashAvailable {
        algorithm: "sha256".into(),
        value: digest_hex(Sha256::digest(std::fs::read(&p).unwrap())),
    };
    execute_wiiu_conversion(
        &plan_wiiu_conversion(&req),
        &options(d.path()),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
}
#[test]
fn large_decode_progress_proves_chunked_processing_and_sparse_zero_reconstruction() {
    let d = tempdir().unwrap();
    let p = d.path().join("large.wux");
    let prefix = fixture(&p, 2048);
    let dest = d.path().join("large.wud");
    let before = crate::dat::rename_apply::identity::capture_identity(&p).unwrap();
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    let mut previous = 0;
    let mut chunks = 0;
    execute_wiiu_conversion(
        &plan,
        &options(d.path()),
        &AtomicBool::new(false),
        &mut |p| {
            assert!(p.written_bytes - previous <= WIIU_CONVERSION_CHUNK_BYTES as u64);
            assert!(p.written_bytes > previous);
            previous = p.written_bytes;
            chunks += 1;
        },
    )
    .unwrap();
    assert_eq!(chunks, 1024);
    assert_eq!(previous, 64 * 1024 * 1024);
    let mut out = File::open(&dest).unwrap();
    let mut h = vec![0; prefix.len()];
    out.read_exact(&mut h).unwrap();
    assert_eq!(h, prefix);
    let mut buffer = [0; WIIU_CONVERSION_CHUNK_BYTES];
    loop {
        let n = out.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        assert!(buffer[..n].iter().all(|b| *b == 0));
    }
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), previous);
    assert_eq!(
        crate::dat::rename_apply::identity::capture_identity(&p).unwrap(),
        before
    );
}
#[test]
fn production_paths_have_no_whole_image_buffering() {
    for source in [
        include_str!("wiiu_disc.rs"),
        include_str!("wiiu_conversion.rs"),
    ] {
        assert!(!source.contains("read_to_end"));
        assert!(!source.contains("fs::read("));
        assert!(!source.contains("vec![0; layout.logical"));
    }
}

#[test]
fn non_power_of_two_and_large_wux_blocks_trim_the_final_slot() {
    let d = tempdir().unwrap();
    let mut expected = wud_header();
    expected.extend(vec![0x42; WUD_HEADER_SIZE as usize]);
    for sector in [0x900_u32, 0x80000] {
        let p = d.path().join(format!("{sector}.wux"));
        let dest = d.path().join(format!("{sector}.wud"));
        let stored: Vec<_> = expected
            .chunks(sector as usize)
            .map(|chunk| {
                let mut full = vec![0xfb; sector as usize];
                full[..chunk.len()].copy_from_slice(chunk);
                full
            })
            .collect();
        let entries: Vec<_> = (0..stored.len() as u32).collect();
        container(&p, sector, expected.len() as u64, &entries, &stored);
        let plan = plan_wiiu_conversion(&request(&p, &dest));
        assert!(plan.refusals.is_empty(), "{plan:?}");
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), expected);
    }
}
#[test]
fn existing_repair_rollback_restores_verified_output_to_retained_stage() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    let expected = fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let before = std::fs::read(&p).unwrap();
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    let (_, mut result) = execute_wiiu_conversion(
        &plan,
        &options(d.path()),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    let stage = result.transaction.entries[0].source_path.clone();
    let rollback = crate::repair::execute::rollback_repair_transaction(
        &mut result.transaction,
        &d.path().join("journal"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        rollback,
        crate::dat::rename_apply::model::RollbackResult::FullyRolledBack
    );
    assert!(!dest.exists());
    assert_eq!(std::fs::read(stage).unwrap(), expected);
    assert_eq!(std::fs::read(&p).unwrap(), before);
}

#[test]
fn wux_index_zero_is_stored_data_even_when_nonzero() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    let expected = fixture(&p, 8);
    let sector = WUD_SECTOR_SIZE as usize;
    let h = wud_header();
    let stored = vec![
        vec![0x5a; sector],
        vec![0; sector],
        h[2 * sector..3 * sector].to_vec(),
        h[..sector].to_vec(),
        h[3 * sector..4 * sector].to_vec(),
    ];
    container(
        &p,
        sector as u32,
        8 * sector as u64,
        &[3, 1, 2, 4, 0, 1, 0, 1],
        &stored,
    );
    let dest = d.path().join("disc.wud");
    execute_wiiu_conversion(
        &plan_wiiu_conversion(&request(&p, &dest)),
        &options(d.path()),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(std::fs::read(dest).unwrap(), expected);
}

#[test]
fn unavailable_journal_prevents_publication_and_cleans_stage() {
    let d = tempdir().unwrap();
    let p = d.path().join("disc.wux");
    fixture(&p, 8);
    let dest = d.path().join("disc.wud");
    let plan = plan_wiiu_conversion(&request(&p, &dest));
    let opts = options(d.path());
    std::fs::write(&opts.journal_dir, b"unavailable").unwrap();
    assert!(execute_wiiu_conversion(&plan, &opts, &AtomicBool::new(false), &mut |_| {}).is_err());
    assert!(!dest.exists());
    assert!(stages(d.path()).is_empty());
    assert_eq!(std::fs::read(&opts.journal_dir).unwrap(), b"unavailable");
}

fn encoding_request(source: &Path, destination: &Path) -> WiiUConversionRequest {
    let mut req = request(source, destination);
    req.direction = WiiUConversionDirection::WudToWux;
    req
}
fn encoding_fixture(path: &Path, tail: &[u8]) -> Vec<u8> {
    let mut bytes = wud_header();
    for value in tail {
        bytes.extend(vec![*value; WUD_SECTOR_SIZE as usize]);
    }
    std::fs::write(path, &bytes).unwrap();
    bytes
}
fn encode_fixture(source: &Path, destination: &Path) -> WiiUConversionRecord {
    execute_wiiu_conversion(
        &plan_wiiu_conversion(&encoding_request(source, destination)),
        &options(destination.parent().unwrap()),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap()
    .0
}

#[test]
fn wud_to_wux_preview_is_bounded_read_only_and_binds_canonical_geometry() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.data");
    let destination = d.path().join("disc.wux");
    let before = encoding_fixture(&source, &[0, 0x5a]);
    let plan = plan_wiiu_conversion(&encoding_request(&source, &destination));
    assert_eq!(
        plan.readiness,
        WiiUConversionReadiness::ReadyToPreview,
        "{plan:?}"
    );
    assert!(plan.refusals.is_empty());
    assert!(plan.no_clobber && plan.source_immutable && plan.post_write_verification_available);
    assert!(plan.tool.is_none());
    assert!(plan.verification.required && plan.verification.exact_identity_provable);
    let layout = plan.wux_creation.unwrap();
    assert_eq!(layout.block_size_bytes, 0x8000);
    assert_eq!(layout.logical_size_bytes, before.len() as u64);
    assert_eq!(layout.logical_block_count, 6);
    assert_eq!(layout.lookup_table_bytes, 24);
    assert_eq!(layout.payload_offset, 0x8000);
    assert_eq!(layout.maximum_output_bytes, before.len() as u64 + 0x8000);
    assert_eq!(plan.space.source_bytes, before.len() as u64);
    assert_eq!(plan.space.destination_exact_bytes, None);
    assert_eq!(
        plan.space.destination_maximum_bytes,
        Some(layout.maximum_output_bytes)
    );
    assert_eq!(
        plan.space.temporary_bytes,
        Some(layout.maximum_output_bytes)
    );
    assert_eq!(plan.space.atomic_duplicate_bytes, Some(0));
    assert_eq!(
        plan.source_inspection
            .source_evidence
            .as_ref()
            .unwrap()
            .size_bytes,
        before.len() as u64
    );
    assert_eq!(std::fs::read(&source).unwrap(), before);
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
}

#[test]
fn wud_to_wux_tiny_header_has_independently_specified_canonical_bytes() {
    let d = tempdir().unwrap();
    let source = d.path().join("tiny.wud");
    let destination = d.path().join("tiny.wux");
    let original = encoding_fixture(&source, &[]);
    let record = encode_fixture(&source, &destination);
    let mut expected = vec![0; 0x8000];
    expected[..8].copy_from_slice(&[b'W', b'U', b'X', b'0', 0x2e, 0xd0, 0x99, 0x10]);
    expected[8..12].copy_from_slice(&0x8000u32.to_le_bytes());
    expected[16..24].copy_from_slice(&0x20000u64.to_le_bytes());
    expected[32..48].copy_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]);
    expected.extend(&original);
    assert_eq!(std::fs::read(&destination).unwrap(), expected);
    let inspected = inspect_wii_u_disc(&destination);
    assert!(inspected.structural_complete, "{inspected:?}");
    assert_eq!(inspected.format, WiiUDiscFormat::Wux);
    assert_eq!(record.wux_creation.unwrap().stored_blocks, 4);
    assert_eq!(
        record.reconstructed_wud_sha256,
        digest_hex(Sha256::digest(&original))
    );
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn wud_to_wux_mixed_zero_nonzero_and_repeated_bytes_have_exact_mappings() {
    let d = tempdir().unwrap();
    let source = d.path().join("mixed.wud");
    let destination = d.path().join("mixed.wux");
    let original = encoding_fixture(&source, &[0, 0x5a, 0, 0x5a, 0xff]);
    let record = encode_fixture(&source, &destination);
    let mut file = File::open(&destination).unwrap();
    let layout = load_wux(&mut file, record.output_bytes).unwrap();
    assert_eq!(layout.table, [0, 1, 2, 3, 1, 4, 1, 5, 6]);
    assert_eq!(layout.unique, 7);
    assert_eq!(
        record.wux_creation,
        Some(WiiUWuxCreationStatistics {
            zero_blocks: 3,
            reused_zero_blocks: 2,
            stored_blocks: 7,
        })
    );
    let mut restored = vec![0; original.len()];
    read_wux_at(&mut file, &layout, 0, &mut restored).unwrap();
    assert_eq!(restored, original);
    for offset in [0x7ff0, 0xfff9, 0x1fff1, 0x27ffb] {
        let mut cross_boundary = [0; 51];
        read_wux_at(&mut file, &layout, offset, &mut cross_boundary).unwrap();
        assert_eq!(
            &cross_boundary,
            &original[offset as usize..offset as usize + 51]
        );
    }
}

#[test]
fn wud_to_wux_entirely_nonzero_blocks_are_all_stored() {
    let d = tempdir().unwrap();
    let source = d.path().join("nonzero.wud");
    let destination = d.path().join("nonzero.wux");
    let mut original = encoding_fixture(&source, &[0xff, 0xff, 1]);
    original[0x8000..0x10000].fill(0xff);
    std::fs::write(&source, &original).unwrap();
    let record = encode_fixture(&source, &destination);
    let stats = record.wux_creation.unwrap();
    assert_eq!(stats.zero_blocks, 0);
    assert_eq!(stats.reused_zero_blocks, 0);
    assert_eq!(stats.stored_blocks, 7);
    let layout = load_wux(&mut File::open(&destination).unwrap(), record.output_bytes).unwrap();
    assert_eq!(layout.table, [0, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn wud_to_wux_near_zero_block_is_not_shared_as_zero() {
    let d = tempdir().unwrap();
    let source = d.path().join("nearzero.wud");
    let destination = d.path().join("nearzero.wux");
    let mut original = encoding_fixture(&source, &[0, 0, 0]);
    *original.last_mut().unwrap() = 1;
    std::fs::write(&source, &original).unwrap();
    let record = encode_fixture(&source, &destination);
    assert_eq!(record.wux_creation.unwrap().stored_blocks, 5);
    let mut file = File::open(&destination).unwrap();
    let layout = load_wux(&mut file, record.output_bytes).unwrap();
    assert_eq!(layout.table, [0, 1, 2, 3, 1, 1, 4]);
    let mut last = [0; 1];
    read_wux_at(&mut file, &layout, layout.logical - 1, &mut last).unwrap();
    assert_eq!(last, [1]);
}

#[test]
fn wud_to_wux_then_existing_wud_decoder_is_byte_exact_and_hash_exact() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    let restored = d.path().join("restored.wud");
    let original = encoding_fixture(&source, &[0, 0x77, 0, 0xff]);
    let encoded = encode_fixture(&source, &destination);
    let (decoded, result) = execute_wiiu_conversion(
        &plan_wiiu_conversion(&request(&destination, &restored)),
        &options(d.path()),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(std::fs::read(&restored).unwrap(), original);
    let hash = digest_hex(Sha256::digest(&original));
    assert_eq!(encoded.source_container_sha256, hash);
    assert_eq!(encoded.reconstructed_wud_sha256, hash);
    assert_eq!(decoded.output_sha256, hash);
    assert_eq!(encoded.output_sha256, decoded.source_container_sha256);
    assert_eq!(encoded.source_header, decoded.source_header);
    assert_eq!(result.summary.applied, 1);
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn wud_to_wux_is_deterministic_across_two_transactions() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let first = d.path().join("first.wux");
    let second = d.path().join("second.wux");
    encoding_fixture(&source, &[0, 0x5a, 0x5a, 0, 0, 0xff]);
    let a = encode_fixture(&source, &first);
    let b = encode_fixture(&source, &second);
    assert_ne!(a.transaction_id, b.transaction_id);
    assert_eq!(a.output_sha256, b.output_sha256);
    assert_eq!(a.wux_creation, b.wux_creation);
    assert_eq!(
        std::fs::read(first).unwrap(),
        std::fs::read(second).unwrap()
    );
}

#[test]
fn wud_to_wux_partial_sector_and_malformed_wud_are_refused_before_staging() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    for size in [
        4,
        0x10003,
        WUD_HEADER_SIZE - 1,
        WUD_HEADER_SIZE + 1,
        MAX_LOGICAL_SIZE + WUD_SECTOR_SIZE as u64,
    ] {
        raw(&source, size);
        let plan = plan_wiiu_conversion(&encoding_request(&source, &destination));
        assert!(!plan.refusals.is_empty(), "accepted {size}");
        assert!(plan.wux_creation.is_none());
        assert!(
            execute_wiiu_conversion(
                &plan,
                &options(d.path()),
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert!(stages(d.path()).is_empty());
    }
    raw(&source, WUD_HEADER_SIZE);
    edit(&source, 0x10000, &[0; 4]);
    assert!(
        !plan_wiiu_conversion(&encoding_request(&source, &destination))
            .refusals
            .is_empty()
    );
}

#[test]
fn wud_to_wux_geometry_limits_overflow_and_table_alignment_are_checked() {
    for size in [
        0,
        1,
        WUD_HEADER_SIZE - 1,
        WUD_HEADER_SIZE + 1,
        MAX_LOGICAL_SIZE + 0x8000,
        u64::MAX,
    ] {
        assert!(WiiUWuxCreationLayout::for_size(size).is_err());
    }
    let largest = WiiUWuxCreationLayout::for_size(MAX_LOGICAL_SIZE).unwrap();
    assert_eq!(largest.logical_block_count, 2_097_152);
    assert_eq!(largest.lookup_table_bytes, 8_388_608);
    assert!(largest.lookup_table_bytes <= MAX_WUX_TABLE_BYTES);
    assert_eq!(largest.payload_offset, 8_421_376);
    assert_eq!(
        largest.maximum_output_bytes,
        MAX_LOGICAL_SIZE + largest.payload_offset
    );
    for (blocks, offset) in [
        (8184, 0x8000),
        (8185, 0x10000),
        (16376, 0x10000),
        (16377, 0x18000),
    ] {
        let layout = WiiUWuxCreationLayout::for_size(blocks * 0x8000).unwrap();
        assert_eq!(layout.payload_offset, offset);
        assert!(layout.lookup_table_bytes + 32 <= offset);
    }
}

#[test]
fn wud_to_wux_stale_header_body_size_and_replaced_source_never_stage() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    for change in 0..4 {
        let bytes = encoding_fixture(&source, &[0x5a, 0]);
        let plan = plan_wiiu_conversion(&encoding_request(&source, &destination));
        match change {
            0 => edit(&source, 0x10020, b"changed"),
            1 => edit(&source, WUD_HEADER_SIZE + 19, &[0x99]),
            2 => File::options()
                .write(true)
                .open(&source)
                .unwrap()
                .set_len(WUD_HEADER_SIZE)
                .unwrap(),
            _ => {
                std::fs::rename(&source, d.path().join("retained.wud")).unwrap();
                std::fs::write(&source, bytes).unwrap();
            }
        }
        assert!(matches!(
            execute_wiiu_conversion(
                &plan,
                &options(d.path()),
                &AtomicBool::new(false),
                &mut |_| {}
            ),
            Err(WiiUConversionError::StalePlan)
        ));
        assert!(!destination.exists());
        assert!(stages(d.path()).is_empty());
    }
}

#[cfg(unix)]
#[test]
fn wud_to_wux_restored_mtime_cannot_hide_a_body_edit() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    encoding_fixture(&source, &[0x5a]);
    let plan = plan_wiiu_conversion(&encoding_request(&source, &destination));
    edit(&source, WUD_HEADER_SIZE + 7, &[0x42]);
    File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(
                plan.source_inspection
                    .source_evidence
                    .as_ref()
                    .unwrap()
                    .modified,
            ),
        )
        .unwrap();
    assert!(matches!(
        execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        ),
        Err(WiiUConversionError::StalePlan)
    ));
    assert!(stages(d.path()).is_empty());
}

#[test]
fn wud_to_wux_existing_and_late_destinations_are_never_replaced() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    encoding_fixture(&source, &[0]);
    for late in [false, true] {
        let destination = d.path().join(format!("{late}.wux"));
        let plan = plan_wiiu_conversion(&encoding_request(&source, &destination));
        if !late {
            std::fs::write(&destination, b"kept").unwrap();
        }
        let result = execute_wiiu_conversion(
            &plan,
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |p| {
                if late && p.written_bytes == p.expected_bytes {
                    std::fs::write(&destination, b"kept").unwrap();
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"kept");
        assert!(
            plan_wiiu_conversion(&encoding_request(&source, &destination))
                .refusals
                .contains(&WiiUConversionRefusal::DestinationExists)
        );
        assert!(stages(d.path()).is_empty());
    }
}

#[test]
fn wud_to_wux_source_alias_and_hard_link_are_refused() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let original = encoding_fixture(&source, &[0x5a]);
    let alias = d.path().join("alias.wux");
    std::fs::hard_link(&source, &alias).unwrap();
    for destination in [&source, &alias] {
        let plan = plan_wiiu_conversion(&encoding_request(&source, destination));
        assert!(!plan.refusals.is_empty());
        assert!(
            execute_wiiu_conversion(
                &plan,
                &options(d.path()),
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .is_err()
        );
    }
    assert_eq!(std::fs::read(source).unwrap(), original);
    assert_eq!(std::fs::read(alias).unwrap(), original);
    assert!(stages(d.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn wud_to_wux_symlinks_and_special_sources_fail_closed() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    encoding_fixture(&source, &[]);
    let alias = d.path().join("alias.wud");
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    for unsafe_source in [&alias, d.path()] {
        let plan = plan_wiiu_conversion(&encoding_request(unsafe_source, &destination));
        assert!(!plan.refusals.is_empty());
        assert!(
            execute_wiiu_conversion(
                &plan,
                &options(d.path()),
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .is_err()
        );
    }
    std::os::unix::fs::symlink(d.path().join("absent"), &destination).unwrap();
    assert!(
        plan_wiiu_conversion(&encoding_request(&source, &destination))
            .refusals
            .contains(&WiiUConversionRefusal::DestinationExists)
    );
    let parent = d.path().join("linked-parent");
    std::os::unix::fs::symlink(d.path(), &parent).unwrap();
    assert!(
        plan_wiiu_conversion(&encoding_request(&source, &parent.join("out.wux")))
            .refusals
            .contains(&WiiUConversionRefusal::DestinationPathUnsafe)
    );
}

#[test]
fn wud_to_wux_cancelled_write_removes_stage_and_preserves_source() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    let original = encoding_fixture(&source, &[0, 0x5a, 0]);
    let cancel = AtomicBool::new(false);
    let mut callbacks = 0;
    let result = execute_wiiu_conversion(
        &plan_wiiu_conversion(&encoding_request(&source, &destination)),
        &options(d.path()),
        &cancel,
        &mut |_| {
            callbacks += 1;
            cancel.store(true, Ordering::Relaxed);
        },
    );
    assert!(matches!(result, Err(WiiUConversionError::Cancelled)));
    assert_eq!(callbacks, 1);
    assert!(!destination.exists());
    assert!(!d.path().join("journal").exists());
    assert!(stages(d.path()).is_empty());
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn wud_to_wux_source_changes_during_encoding_never_publish() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    for change in 0..3 {
        encoding_fixture(&source, &[0x5a, 0]);
        let mut once = false;
        let result = execute_wiiu_conversion(
            &plan_wiiu_conversion(&encoding_request(&source, &destination)),
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {
                if !once {
                    once = true;
                    match change {
                        0 => File::options()
                            .write(true)
                            .open(&source)
                            .unwrap()
                            .set_len(32)
                            .unwrap(),
                        1 => edit(&source, WUD_HEADER_SIZE + 17, &[0x99]),
                        _ => edit(&source, 17, &[0x99]),
                    }
                }
            },
        );
        assert!(result.is_err());
        assert!(!destination.exists());
        assert!(stages(d.path()).is_empty());
    }
}

#[test]
fn wud_to_wux_corrupted_stage_header_table_payload_and_extent_never_publish() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    let original = encoding_fixture(&source, &[0x5a, 0]);
    for corrupt in 0..4 {
        let result = execute_wiiu_conversion(
            &plan_wiiu_conversion(&encoding_request(&source, &destination)),
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |p| {
                if p.written_bytes == p.expected_bytes {
                    let staged = stages(d.path())[0].join("output.wux");
                    match corrupt {
                        0 => edit(&staged, 0, b"NOPE"),
                        1 => edit(&staged, 32 + 4 * 4, &u32::MAX.to_le_bytes()),
                        // Structure still parses: only full logical hashing catches this.
                        2 => edit(&staged, 0x8000 + 4 * 0x8000 + 19, &[0x99]),
                        _ => File::options()
                            .write(true)
                            .open(staged)
                            .unwrap()
                            .set_len(32)
                            .unwrap(),
                    }
                }
            },
        );
        assert!(
            matches!(result, Err(WiiUConversionError::VerificationFailed(_))),
            "{result:?}"
        );
        assert!(!destination.exists());
        assert!(stages(d.path()).is_empty());
        assert_eq!(std::fs::read(&source).unwrap(), original);
    }
}

#[test]
fn wud_to_wux_tampered_plan_geometry_and_direction_are_stale() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    encoding_fixture(&source, &[0]);
    for change in 0..5 {
        let mut plan = plan_wiiu_conversion(&encoding_request(&source, &destination));
        match change {
            0 => plan.wux_creation.as_mut().unwrap().block_size_bytes = 0x10000,
            1 => plan.wux_creation.as_mut().unwrap().lookup_table_bytes = u64::MAX,
            2 => plan.space.destination_maximum_bytes = Some(1),
            3 => plan.direction = WiiUConversionDirection::WuxToWud,
            _ => plan.wux_creation = None,
        }
        assert!(matches!(
            execute_wiiu_conversion(
                &plan,
                &options(d.path()),
                &AtomicBool::new(false),
                &mut |_| {}
            ),
            Err(WiiUConversionError::StalePlan)
        ));
        assert!(!destination.exists());
        assert!(stages(d.path()).is_empty());
    }
}

#[test]
fn wud_to_wux_hash_evidence_and_maximum_staging_space_are_required() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    let original = encoding_fixture(&source, &[0]);
    let mut req = encoding_request(&source, &destination);
    req.available_free_space = Some(original.len() as u64);
    assert!(plan_wiiu_conversion(&req).refusals.iter().any(|r| matches!(
        r,
        WiiUConversionRefusal::InsufficientDestinationSpace { .. }
    )));
    req.available_free_space = None;
    req.source_identity = WiiUConversionIdentity::HashAvailable {
        algorithm: "sha256".into(),
        value: "0".repeat(64),
    };
    assert!(matches!(
        execute_wiiu_conversion(
            &plan_wiiu_conversion(&req),
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        ),
        Err(WiiUConversionError::StalePlan)
    ));
    req.source_identity = WiiUConversionIdentity::HashAvailable {
        algorithm: "SHA256".into(),
        value: digest_hex(Sha256::digest(&original)),
    };
    assert!(
        execute_wiiu_conversion(
            &plan_wiiu_conversion(&req),
            &options(d.path()),
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .is_ok()
    );
}

#[test]
fn wud_to_wux_journal_records_round_trip_and_existing_rollback_works() {
    let d = tempdir().unwrap();
    let source = d.path().join("disc.wud");
    let destination = d.path().join("disc.wux");
    let original = encoding_fixture(&source, &[0x5a, 0]);
    let (record, mut result) = execute_wiiu_conversion(
        &plan_wiiu_conversion(&encoding_request(&source, &destination)),
        &options(d.path()),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    let encoded = std::fs::read(&destination).unwrap();
    let evidence = &result.transaction.unknown["wiiu_conversion"];
    assert_eq!(evidence["policy"], "native-wud-to-wux-zero-sharing-v1");
    assert_eq!(evidence["source_retained"], true);
    assert_eq!(
        evidence["reconstructed_wud_sha256"],
        record.source_container_sha256
    );
    assert_eq!(evidence["logical_wud_bytes"], original.len() as u64);
    assert_eq!(evidence["wux_creation"]["reused_zero_blocks"], 1);
    let staged = result.transaction.entries[0].source_path.clone();
    let rollback = crate::repair::execute::rollback_repair_transaction(
        &mut result.transaction,
        &d.path().join("journal"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        rollback,
        crate::dat::rename_apply::model::RollbackResult::FullyRolledBack
    );
    assert!(!destination.exists());
    assert_eq!(std::fs::read(staged).unwrap(), encoded);
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn wud_to_wux_large_streaming_and_measurement() {
    let bytes = std::env::var("EMUWIZ_WUD_WUX_PERF_BYTES")
        .map(|v| v.parse::<u64>().unwrap())
        .unwrap_or(128 * 1024 * 1024);
    let d = tempdir().unwrap();
    let source = d.path().join("large.wud");
    let destination = d.path().join("large.wux");
    raw(&source, bytes);
    let start = std::time::Instant::now();
    let original = crate::dat::rename_apply::identity::capture_identity(&source).unwrap();
    let plan = plan_wiiu_conversion(&encoding_request(&source, &destination));
    let mut previous = 0;
    let mut callbacks = 0;
    let (record, _) = execute_wiiu_conversion(
        &plan,
        &options(d.path()),
        &AtomicBool::new(false),
        &mut |p| {
            assert_eq!(
                p.written_bytes - previous,
                WIIU_WUX_CANONICAL_BLOCK_BYTES as u64
            );
            assert_eq!(p.completed_blocks, p.written_bytes / 0x8000);
            previous = p.written_bytes;
            callbacks += 1;
        },
    )
    .unwrap();
    let conversion_seconds = start.elapsed().as_secs_f64();
    let stats = record.wux_creation.unwrap();
    assert_eq!(previous, bytes);
    assert_eq!(callbacks, bytes / 0x8000);
    assert_eq!(stats.stored_blocks, 4);
    assert_eq!(stats.zero_blocks, bytes / 0x8000 - 3);
    assert_eq!(stats.reused_zero_blocks, stats.zero_blocks - 1);
    assert_eq!(
        record.reconstructed_wud_sha256,
        record.source_container_sha256
    );
    let inspection = inspect_wii_u_disc(&destination);
    assert!(inspection.structural_complete);
    assert_eq!(
        inspection.structure.unwrap().logical_disc_size_bytes,
        Some(bytes)
    );
    // Independently exercise the existing logical reader without a restored WUD
    // allocation or file. This also checks the full logical extent, not parsing alone.
    let mut output = File::open(&destination).unwrap();
    let layout = load_wux(&mut output, record.output_bytes).unwrap();
    assert_eq!(layout.logical, bytes);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; WIIU_CONVERSION_CHUNK_BYTES];
    let mut offset = 0;
    while offset < bytes {
        let length = (bytes - offset).min(buffer.len() as u64) as usize;
        read_wux_at(&mut output, &layout, offset, &mut buffer[..length]).unwrap();
        digest.update(&buffer[..length]);
        offset += length as u64;
    }
    assert_eq!(
        digest_hex(digest.finalize()),
        record.source_container_sha256
    );
    assert_eq!(
        record.source_container_sha256,
        digest_hex(original.freshness.as_ref().unwrap().sha256)
    );
    drop(layout);
    let second = encode_fixture(&source, &d.path().join("second.wux"));
    assert_eq!(record.output_bytes, second.output_bytes);
    assert_eq!(record.output_sha256, second.output_sha256);
    assert_eq!(record.wux_creation, second.wux_creation);
    assert!(crate::dat::rename_apply::identity::identity_matches(
        &original,
        &crate::dat::rename_apply::identity::capture_identity(&source).unwrap()
    ));
    println!(
        "WUD_WUX_PERF input_bytes={bytes} output_bytes={} zero_blocks={} reused_zero_blocks={} stored_blocks={} callbacks={callbacks} conversion_seconds={conversion_seconds:.3} elapsed_seconds={:.3} source_sha256={} logical_sha256={} output_sha256={} deterministic=true",
        record.output_bytes,
        stats.zero_blocks,
        stats.reused_zero_blocks,
        stats.stored_blocks,
        start.elapsed().as_secs_f64(),
        record.source_container_sha256,
        record.reconstructed_wud_sha256,
        record.output_sha256,
    );
}
