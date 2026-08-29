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
