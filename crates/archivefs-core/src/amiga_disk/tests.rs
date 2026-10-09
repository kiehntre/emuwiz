use super::*;
use tempfile::tempdir;
fn p32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_be_bytes())
}
fn image(parts: &[(u32, u32, u32, u32)]) -> Vec<u8> {
    let mut b = vec![0; 512 * 400];
    b[..4].copy_from_slice(RDSK);
    p32(&mut b, 16, 512);
    p32(&mut b, 28, if parts.is_empty() { NONE } else { 1 });
    p32(&mut b, 64, 20);
    p32(&mut b, 68, 10);
    p32(&mut b, 72, 2);
    for (i, (low, high, next, dos)) in parts.iter().enumerate() {
        let o = (i + 1) * 512;
        b[o..o + 4].copy_from_slice(PART);
        p32(&mut b, o + 16, *next);
        b[o + 36] = 4;
        b[o + 37..o + 41].copy_from_slice(b"Work");
        p32(&mut b, o + 140, 2);
        p32(&mut b, o + 148, 10);
        p32(&mut b, o + 164, *low);
        p32(&mut b, o + 168, *high);
        p32(&mut b, o + 192, *dos);
        let off = *low as usize * 2 * 10 * 512;
        if off + 4 <= b.len() {
            b[off..off + 4].copy_from_slice(&dos.to_be_bytes());
        }
    }
    b
}
fn file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempdir().unwrap();
    let p = d.path().join("GoldenAxe_v1.4_0017.hdf");
    std::fs::write(&p, bytes).unwrap();
    (d, p)
}
#[test]
fn valid_rdb_and_part() {
    let (_d, p) = file(&image(&[(2, 3, NONE, 0x444f5301)]));
    let h = inspect_hdf(&p).unwrap();
    assert_eq!(h.rdb.partitions.len(), 1);
    let q = &h.rdb.partitions[0];
    assert_eq!(q.byte_offset, 20480);
    assert_eq!(q.filesystem, FileSystem::Dos(1));
}
#[test]
fn scans_initial_range() {
    let mut b = image(&[]);
    b[..4].fill(0);
    b[512 * 8..512 * 8 + 4].copy_from_slice(RDSK);
    p32(&mut b, 512 * 8 + 16, 512);
    p32(&mut b, 512 * 8 + 28, NONE);
    let (_d, p) = file(&b);
    assert_eq!(inspect_hdf(&p).unwrap().rdb.block_index, 8)
}
#[test]
fn no_rdb_truncated_and_invalid_block_fail() {
    let (_d, p) = file(&vec![0; 512]);
    assert!(matches!(inspect_hdf(&p), Err(DiskError::NoRdb)));
    let (_d, p) = file(&[1, 2]);
    assert!(inspect_hdf(&p).is_err());
    let mut b = image(&[]);
    p32(&mut b, 16, 1);
    let (_d, p) = file(&b);
    assert!(inspect_hdf(&p).is_err())
}
#[test]
fn multiple_cycle_and_bad_pointer_fail() {
    let (_d, p) = file(&image(&[(2, 2, 2, 0x444f5300), (3, 3, NONE, 0x444f5303)]));
    assert_eq!(inspect_hdf(&p).unwrap().rdb.partitions.len(), 2);
    let (_d, p) = file(&image(&[(2, 2, 1, 0x444f5300)]));
    assert!(matches!(inspect_hdf(&p), Err(DiskError::Cycle)));
    let (_d, p) = file(&image(&[(2, 2, 9999, 0x444f5300)]));
    assert!(inspect_hdf(&p).is_err())
}
#[test]
fn dos_and_unsupported_fs() {
    for n in 0..=7 {
        let (_d, p) = file(&image(&[(2, 2, NONE, 0x444f5300 + n)]));
        assert_eq!(
            inspect_hdf(&p).unwrap().rdb.partitions[0].filesystem,
            FileSystem::Dos(n as u8)
        )
    }
    for raw in [0x50465303, 0x53465300, 0x4d754653] {
        let (_d, p) = file(&image(&[(2, 2, NONE, raw)]));
        assert!(!matches!(
            inspect_hdf(&p).unwrap().rdb.partitions[0].filesystem,
            FileSystem::Dos(_)
        ))
    }
}
// A minimal flat (non-RDB) AmigaDOS image: a real-world WHDLoad CD32
// pack shape - the whole file is one filesystem starting at byte 0, no
// RDSK block anywhere. `1024` bytes is comfortably above the 512-byte
// minimum and stays a whole number of sectors.
fn flat_image(dos_type: u32) -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    p32(&mut b, 0, dos_type);
    b
}

#[test]
fn flat_amigados_image_is_recognised_as_one_whole_image_partition() {
    let (_d, p) = file(&flat_image(0x444f_5301));
    let disk = inspect_amiga_image(&p).unwrap();
    assert_eq!(disk.rdb.partitions.len(), 1);
    let partition = &disk.rdb.partitions[0];
    assert_eq!(partition.byte_offset, 0);
    assert_eq!(partition.byte_length, 1024);
    assert_eq!(partition.filesystem, FileSystem::Dos(1));
}

#[test]
fn flat_image_with_no_recognised_boot_signature_is_still_an_error() {
    let (_d, p) = file(&vec![0u8; 1024]);
    assert!(matches!(inspect_amiga_image(&p), Err(DiskError::NoRdb)));
}

#[test]
fn inspect_amiga_image_still_prefers_a_real_rdb_when_present() {
    let (_d, p) = file(&image(&[(2, 3, NONE, 0x444f5301)]));
    let disk = inspect_amiga_image(&p).unwrap();
    assert_eq!(disk.rdb.partitions.len(), 1);
    assert_eq!(disk.rdb.partitions[0].byte_offset, 20480);
}

#[test]
fn inspect_amiga_image_still_reports_real_errors_not_just_no_rdb() {
    let (_d, p) = file(&[0u8; 100]); // below the 512-byte minimum
    assert!(matches!(inspect_amiga_image(&p), Err(DiskError::TooSmall)));
}

#[test]
fn beyond_eof_and_evidence() {
    let (_d, p) = file(&image(&[(99, 199, NONE, 0x444f5300)]));
    assert!(inspect_hdf(&p).is_err());
    let (_d, p) = file(&image(&[(2, 2, NONE, 0x444f5300)]));
    let h = inspect_hdf(&p).unwrap();
    let e = structural_hdf_observation(&h);
    assert_eq!(e.provenance.representation, Representation::WholeHdf);
    assert_eq!(e.platform_candidate.as_deref(), Some("Amiga"));
    assert_ne!(e.claim, ClaimType::ExactSlaveMatch)
}

// --- .adf content-aware inspection -------------------------------------
//
// A minimal, structurally valid flat AmigaDOS floppy image (no RDB
// wrapper): a `DOS\x0N` boot block, a root-block pointer, and a valid
// `ST_ROOT` root block carrying a volume label - exactly the boot/root
// structures `inspect_amiga_filesystem` validates via the existing
// bounded `affs-read` reader. 128 sectors keeps the root block in the
// middle at block 64, matching that reader's default geometry.

fn fs_checksum(block: &mut [u8; 512]) {
    let mut sum = 0_u32;
    for offset in (0..512).step_by(4) {
        if offset != 20 {
            sum = sum.wrapping_add(u32::from_be_bytes(
                block[offset..offset + 4].try_into().unwrap(),
            ));
        }
    }
    p32(block, 20, (sum as i32).wrapping_neg() as u32);
}

fn flat_adf(dos: u8, volume: &[u8]) -> Vec<u8> {
    const SECTORS: usize = 128;
    const ROOT: usize = SECTORS / 2;
    let mut img = vec![0_u8; SECTORS * 512];
    img[..3].copy_from_slice(b"DOS");
    img[3] = dos;
    p32(&mut img, 8, ROOT as u32); // affs-read reads the root block index here
    let mut root = [0_u8; 512];
    p32(&mut root, 0, 2); // T_HEADER
    p32(&mut root, 12, 72); // hash-table size
    let name_len = volume.len().min(30);
    root[0x1B0] = name_len as u8;
    root[0x1B1..0x1B1 + name_len].copy_from_slice(&volume[..name_len]);
    p32(&mut root, 508, 1); // ST_ROOT
    fs_checksum(&mut root);
    img[ROOT * 512..(ROOT + 1) * 512].copy_from_slice(&root);
    img
}

#[test]
fn flat_adf_ofs_dos0_is_structurally_confirmed_from_contents() {
    let (_d, p) = file(&flat_adf(0, b"WorkbenchDisk"));
    let inspection = inspect_amiga_floppy(&p).unwrap();
    assert_eq!(inspection.filesystem().unwrap().dos_type, 0);
    assert_eq!(inspection.filesystem().unwrap().family, AmigaDosFamily::Ofs);
    assert!(!inspection.filesystem().unwrap().international);
    assert!(!inspection.filesystem().unwrap().directory_cache);
    assert_eq!(inspection.filesystem().unwrap().block_size, 512);
    assert_eq!(
        inspection.filesystem().unwrap().volume_label.as_deref(),
        Some("WorkbenchDisk")
    );
    assert_eq!(inspection.disk.rdb.partitions.len(), 1);
}

#[test]
fn flat_adf_ffs_dos1_is_structurally_confirmed_from_contents() {
    let (_d, p) = file(&flat_adf(1, b"GameDisk1"));
    let inspection = inspect_amiga_floppy(&p).unwrap();
    assert_eq!(inspection.filesystem().unwrap().dos_type, 1);
    assert_eq!(inspection.filesystem().unwrap().family, AmigaDosFamily::Ffs);
    assert_eq!(
        inspection.filesystem().unwrap().volume_label.as_deref(),
        Some("GameDisk1")
    );
}

#[test]
fn flat_adf_dos2_through_dos7_variants_are_confirmed_at_512_byte_blocks() {
    for dos in 2..=7_u8 {
        let (_d, p) = file(&flat_adf(dos, b"Vol"));
        let inspection = inspect_amiga_floppy(&p)
            .unwrap_or_else(|error| panic!("DOS\\{dos} should be supported: {error:?}"));
        assert_eq!(inspection.filesystem().unwrap().dos_type, dos);
        assert_eq!(
            inspection.filesystem().unwrap().family,
            if dos & 1 == 0 {
                AmigaDosFamily::Ofs
            } else {
                AmigaDosFamily::Ffs
            }
        );
        assert_eq!(inspection.filesystem().unwrap().international, dos & 2 != 0);
        assert_eq!(
            inspection.filesystem().unwrap().directory_cache,
            dos & 4 != 0
        );
    }
}

#[test]
fn random_bytes_named_adf_are_refused_not_trusted_for_the_extension() {
    let (_d, p) = file(&vec![0xAB_u8; 4096]);
    assert!(matches!(
        inspect_amiga_floppy(&p),
        Err(AmigaFloppyError::Container(DiskError::NoRdb))
    ));
}

#[test]
fn zip_and_truncated_and_acorn_style_adf_are_all_refused() {
    // ZIP renamed .adf (local file header magic `PK\x03\x04`).
    let mut zip = vec![0_u8; 2048];
    zip[..4].copy_from_slice(b"PK\x03\x04");
    let (_d, p) = file(&zip);
    assert!(matches!(
        inspect_amiga_floppy(&p),
        Err(AmigaFloppyError::Container(DiskError::NoRdb))
    ));

    // Truncated below the 512-byte minimum.
    let (_d, p) = file(&[b'D', b'O', b'S', 0]);
    assert!(matches!(
        inspect_amiga_floppy(&p),
        Err(AmigaFloppyError::Container(DiskError::TooSmall))
    ));

    // Acorn ADFS / other non-Amiga "ADF": no `DOS` boot identifier.
    let mut acorn = vec![0_u8; 2048];
    acorn[..8].copy_from_slice(b"Hugo\0\0\0\0");
    let (_d, p) = file(&acorn);
    assert!(matches!(
        inspect_amiga_floppy(&p),
        Err(AmigaFloppyError::Container(DiskError::NoRdb))
    ));
}

#[test]
fn adf_with_dos_signature_but_malformed_root_block_fails_closed() {
    let mut img = flat_adf(1, b"Broken");
    // Corrupt the root block so its checksum/structure no longer validates.
    let root = 64 * 512;
    img[root..root + 4].copy_from_slice(&0xDEAD_BEEF_u32.to_be_bytes());
    let (_d, p) = file(&img);
    assert!(matches!(
        inspect_amiga_floppy(&p),
        Err(AmigaFloppyError::Filesystem(_))
    ));
}

#[test]
fn standard_floppy_with_dos_boot_and_missing_root_is_not_rejected_as_media() {
    let mut bytes = vec![0_u8; 880 * 1024];
    bytes[..4].copy_from_slice(b"DOS\x01");
    p32(&mut bytes, 8, 880);
    let (_dir, path) = file(&bytes);
    let inspection = inspect_amiga_floppy(&path).expect("plausible media must not require a root");
    assert!(matches!(
        inspection.evidence,
        AmigaFloppyEvidence::Plausible { .. }
    ));
    assert!(inspection.filesystem().is_none());
    let evidence = structural_amiga_floppy_observation(&inspection);
    assert_eq!(evidence.claim_strength, ClaimStrength::Weak);
    assert_eq!(evidence.release_candidate, None);
    assert_eq!(evidence.hash_or_value, None);
    assert_eq!(
        inspection.description(),
        "Structurally plausible Amiga floppy; no readable AmigaDOS filesystem was found."
    );
}

#[test]
fn standard_ofs_and_ffs_adfs_keep_validated_filesystem_evidence_without_writes() {
    for dos in [0, 1] {
        let tiny = flat_adf(dos, b"DescriptiveLabel");
        let mut bytes = vec![0_u8; 880 * 1024];
        bytes[..512].copy_from_slice(&tiny[..512]);
        p32(&mut bytes, 8, 880);
        bytes[880 * 512..881 * 512].copy_from_slice(&tiny[64 * 512..65 * 512]);
        let (_dir, path) = file(&bytes);
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let inspection = inspect_amiga_floppy(&path).unwrap();
        assert!(matches!(
            inspection.evidence,
            AmigaFloppyEvidence::AmigaDosValidated(_)
        ));
        assert_eq!(inspection.filesystem().unwrap().dos_type, dos);
        assert!(
            inspection
                .description()
                .starts_with("AmigaDOS filesystem validated")
        );
        assert_eq!(
            structural_amiga_floppy_observation(&inspection).claim_strength,
            ClaimStrength::Strong
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
    }
}

#[test]
fn standard_dd_and_hd_bad_roots_are_plausible_not_validated_or_declared_damaged() {
    for size in [880 * 1024, 1760 * 1024] {
        let mut bytes = vec![0_u8; size];
        bytes[..4].copy_from_slice(b"DOS\x01");
        p32(&mut bytes, 8, (size / 1024) as u32);
        p32(&mut bytes, size / 2, 0xDEAD_BEEF);
        let (_dir, path) = file(&bytes);
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let inspection = inspect_amiga_floppy(&path).unwrap();
        assert!(inspection.filesystem().is_none());
        assert!(matches!(
            inspection.evidence,
            AmigaFloppyEvidence::Plausible {
                filesystem_error: FilesystemError::InvalidFilesystem(_)
            }
        ));
        // The original filesystem validator still rejects the same structure.
        assert!(
            inspect_amiga_filesystem(&inspection.disk, &inspection.disk.rdb.partitions[0]).is_err()
        );
        for overclaim in ["corrupt", "verified", "good dump", "original", "complete"] {
            assert!(!inspection.description().to_lowercase().contains(overclaim));
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
    }
}

#[test]
fn random_zero_and_other_format_880_kib_files_have_insufficient_amiga_evidence() {
    let mut random = vec![0_u8; 880 * 1024];
    let mut seed = 0xC0FFEE_u32;
    for byte in &mut random {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        *byte = seed as u8;
    }
    let (_dir, path) = file(&random);
    assert!(inspect_amiga_floppy(&path).is_err());
    for header in [
        b"\0\0\0\0".as_slice(),
        b"Hugo",
        b"PK\x03\x04",
        b"DOS\x08",
        b"\xEB\x3C\x90M",
    ] {
        let mut bytes = vec![0_u8; 880 * 1024];
        bytes[..header.len()].copy_from_slice(header);
        let (_dir, path) = file(&bytes);
        assert!(inspect_amiga_floppy(&path).is_err(), "header {header:?}");
    }
}

#[test]
fn recognized_non_amigados_boot_identifiers_are_detection_only_at_floppy_sizes() {
    for header in [b"PFS\x03", b"SFS\0", b"MuFS"] {
        let mut bytes = vec![0_u8; 880 * 1024];
        bytes[..4].copy_from_slice(header);
        let (_dir, path) = file(&bytes);
        let inspection = inspect_amiga_floppy(&path).unwrap();
        assert!(matches!(
            inspection.evidence,
            AmigaFloppyEvidence::Plausible {
                filesystem_error: FilesystemError::UnsupportedFilesystem(_)
            }
        ));
        assert!(inspection.filesystem().is_none());
        let observation = structural_amiga_floppy_observation(&inspection);
        assert_eq!(observation.claim_strength, ClaimStrength::Weak);
        assert_eq!(observation.release_candidate, None);
    }
}

#[test]
fn boot_identifier_without_supported_floppy_size_cannot_get_weaker_evidence() {
    for size in [
        4,
        511,
        512,
        4096,
        880 * 1024 - 1,
        880 * 1024 - 512,
        880 * 1024 + 512,
    ] {
        let mut bytes = vec![0_u8; size];
        bytes[..4].copy_from_slice(b"DOS\x01");
        let (_dir, path) = file(&bytes);
        assert!(inspect_amiga_floppy(&path).is_err(), "size {size}");
    }
}

#[test]
fn structural_amiga_floppy_observation_is_platform_only_never_a_release() {
    let (_d, p) = file(&flat_adf(3, b"IntlFfsDisk"));
    let inspection = inspect_amiga_floppy(&p).unwrap();
    let observation = structural_amiga_floppy_observation(&inspection);
    assert_eq!(observation.platform_candidate.as_deref(), Some("Amiga"));
    assert_eq!(observation.release_candidate, None);
    assert_eq!(observation.hash_or_value, None);
    assert_eq!(observation.claim, ClaimType::PlatformCandidate);
    assert_eq!(observation.claim_strength, ClaimStrength::Strong);
    assert_eq!(
        observation.provenance.representation,
        Representation::StructuralMetadata
    );
    assert_eq!(
        observation.provenance.channel,
        EvidenceChannel::LocalStructural
    );
    assert_ne!(observation.claim, ClaimType::ExactSlaveMatch);
    let notes = observation.notes.unwrap();
    assert!(notes.contains("DOS\\3"));
    assert!(notes.contains("FFS"));
    assert!(notes.contains("international"));
    assert!(notes.contains("IntlFfsDisk"));
}
