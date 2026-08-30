//! Tests for the DOS boot-media evidence module.
//!
//! Every image is built byte-for-byte from a documented BPB and a real
//! 32-byte-entry root directory, so both a genuine DOS floppy and a hostile
//! case (a truncated root, an impossible geometry, an Atari-ST-geometry FAT
//! disk with no system files) can be constructed exactly rather than
//! approximated.

use super::*;
use crate::content_evidence::{ContentEvidenceConfidence, ContentEvidenceKind};
use crate::disk_format::{MAX_DISK_FORMAT_BYTES_READ, MAX_DISK_FORMAT_OFFSET};
use crate::safe_read::TrustedRoots;
use std::path::PathBuf;

// --- Image builders ----------------------------------------------------

const SECTOR: usize = 512;

/// One directory entry: 8.3 name padded with spaces, attribute byte.
fn dir_entry(base: &str, ext: &str, attr: u8) -> [u8; 32] {
    let mut entry = [0_u8; 32];
    entry[0..8].fill(b' ');
    entry[8..11].fill(b' ');
    let b = base.as_bytes();
    let e = ext.as_bytes();
    assert!(b.len() <= 8 && e.len() <= 3);
    entry[0..b.len()].copy_from_slice(b);
    entry[8..8 + e.len()].copy_from_slice(e);
    entry[11] = attr;
    entry
}

struct Bpb {
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    fat_count: u8,
    root_entries: u16,
    total_sectors: u32,
    media: u8,
    sectors_per_fat: u16,
    sectors_per_track: u16,
    heads: u16,
}

/// Builds a FAT image: a real BPB at the documented offsets, the boot
/// signature, `entries` written into the root directory, padded to
/// `total_sectors` (or to `image_sectors` when the caller wants slack or
/// truncation).
fn fat_image(bpb: &Bpb, entries: &[[u8; 32]], image_sectors: Option<u32>) -> Vec<u8> {
    let image_sectors = image_sectors.unwrap_or(bpb.total_sectors);
    let mut image = vec![0_u8; image_sectors as usize * SECTOR];

    image[0x00] = 0xEB; // short jump, as a real boot sector has
    image[0x01] = 0x3C;
    image[0x02] = 0x90;
    image[0x03..0x0B].copy_from_slice(b"ARCHVFS ");
    image[0x0B..0x0D].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    image[0x0D] = bpb.sectors_per_cluster;
    image[0x0E..0x10].copy_from_slice(&bpb.reserved_sectors.to_le_bytes());
    image[0x10] = bpb.fat_count;
    image[0x11..0x13].copy_from_slice(&bpb.root_entries.to_le_bytes());
    let total16 = u16::try_from(bpb.total_sectors).unwrap_or(0);
    image[0x13..0x15].copy_from_slice(&total16.to_le_bytes());
    image[0x15] = bpb.media;
    image[0x16..0x18].copy_from_slice(&bpb.sectors_per_fat.to_le_bytes());
    image[0x18..0x1A].copy_from_slice(&bpb.sectors_per_track.to_le_bytes());
    image[0x1A..0x1C].copy_from_slice(&bpb.heads.to_le_bytes());
    image[0x1C..0x20].copy_from_slice(&0_u32.to_le_bytes()); // hidden sectors
    let total32 = if total16 == 0 { bpb.total_sectors } else { 0 };
    image[0x20..0x24].copy_from_slice(&total32.to_le_bytes());
    image[BOOT_SIGNATURE_OFFSET] = 0x55;
    image[BOOT_SIGNATURE_OFFSET + 1] = 0xAA;

    let root_offset = (u64::from(bpb.reserved_sectors)
        + u64::from(bpb.fat_count) * u64::from(bpb.sectors_per_fat))
        * SECTOR as u64;
    let root_offset = root_offset as usize;
    for (index, entry) in entries.iter().enumerate() {
        let start = root_offset + index * 32;
        if start + 32 <= image.len() {
            image[start..start + 32].copy_from_slice(entry);
        }
    }
    image
}

/// The standard 1.44 MB FAT12 floppy geometry.
fn floppy_1440() -> Bpb {
    Bpb {
        sectors_per_cluster: 1,
        reserved_sectors: 1,
        fat_count: 2,
        root_entries: 224,
        total_sectors: 2880,
        media: 0xF0,
        sectors_per_fat: 9,
        sectors_per_track: 18,
        heads: 2,
    }
}

/// A compact FAT16 geometry whose root directory still begins inside the
/// bounded inspection window.
fn small_fat16() -> Bpb {
    Bpb {
        sectors_per_cluster: 1,
        reserved_sectors: 1,
        fat_count: 2,
        root_entries: 512,
        total_sectors: 4200, // (4200 - 73) clusters => FAT16
        media: 0xF8,
        sectors_per_fat: 20,
        sectors_per_track: 63,
        heads: 16,
    }
}

/// The Atari ST double-sided 720 KB floppy geometry - a valid FAT12 BPB
/// that is not DOS media.
fn atari_st_720() -> Bpb {
    Bpb {
        sectors_per_cluster: 2,
        reserved_sectors: 1,
        fat_count: 2,
        root_entries: 112,
        total_sectors: 1440,
        media: 0xF9,
        sectors_per_fat: 5,
        sectors_per_track: 9,
        heads: 2,
    }
}

const SYS: u8 = 0x04 | 0x02 | 0x01; // system | hidden | read-only, as DOS sets

fn io_sys() -> [u8; 32] {
    dir_entry("IO", "SYS", SYS)
}
fn msdos_sys() -> [u8; 32] {
    dir_entry("MSDOS", "SYS", SYS)
}
fn ibmbio_com() -> [u8; 32] {
    dir_entry("IBMBIO", "COM", SYS)
}
fn ibmdos_com() -> [u8; 32] {
    dir_entry("IBMDOS", "COM", SYS)
}
fn command_com() -> [u8; 32] {
    dir_entry("COMMAND", "COM", 0x20)
}

fn inspect_bytes(label: &str, extension: &str, bytes: &[u8]) -> DosBootInspection {
    let dir = std::env::temp_dir().join(format!(
        "archivefs-dos-boot-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let path = dir.join(format!("{label}.{extension}"));
    std::fs::write(&path, bytes).expect("fixture write");
    let result = inspect_dos_boot_media(&path, &TrustedRoots::none(), None);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn parse(bytes: &[u8]) -> Result<FatVolume, DiskFormatRefusal> {
    parse_fat_bpb(bytes, bytes.len() as u64)
}

// --- Positive tests (section 7) --------------------------------------------

#[test]
fn fat12_floppy_with_io_and_msdos_is_msdos_family_boot_media() {
    let image = fat_image(&floppy_1440(), &[io_sys(), msdos_sys()], None);
    let inspection = inspect_bytes("msdos12", "img", &image);

    assert!(inspection.refusal.is_none());
    assert!(inspection.filesystem.is_some());
    assert_eq!(inspection.boot_families, vec![DosBootFamily::MsDos]);
    assert!(inspection.has_dos_boot_pair());

    let evidence = observe_dos_boot_evidence(&inspection);
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].kind, ContentEvidenceKind::BootStructure);
    assert_eq!(evidence[0].value, DOS_MSDOS_SYSTEM_FILES);
    assert_eq!(evidence[0].confidence, ContentEvidenceConfidence::Strong);
}

#[test]
fn fat12_floppy_with_ibmbio_and_ibmdos_is_pc_dos_family_boot_media() {
    let image = fat_image(&floppy_1440(), &[ibmbio_com(), ibmdos_com()], None);
    let inspection = inspect_bytes("pcdos12", "ima", &image);

    assert_eq!(inspection.boot_families, vec![DosBootFamily::PcDos]);
    let evidence = observe_dos_boot_evidence(&inspection);
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].value, DOS_PCDOS_SYSTEM_FILES);
    assert_eq!(evidence[0].confidence, ContentEvidenceConfidence::Strong);
}

#[test]
fn command_com_alongside_a_pair_is_corroboration_only_not_an_extra_fact() {
    let image = fat_image(
        &floppy_1440(),
        &[io_sys(), msdos_sys(), command_com()],
        None,
    );
    let inspection = inspect_bytes("corrob", "img", &image);

    assert!(inspection.command_com_present);
    assert_eq!(inspection.boot_families, vec![DosBootFamily::MsDos]);
    // COMMAND.COM adds no second evidence fact.
    assert_eq!(observe_dos_boot_evidence(&inspection).len(), 1);
    assert!(
        inspection
            .observations
            .iter()
            .any(|line| line.contains("COMMAND.COM") && line.contains("corroboration"))
    );
}

#[test]
fn root_directory_location_is_bounded_and_computed_from_the_bpb() {
    let image = fat_image(&floppy_1440(), &[io_sys(), msdos_sys()], None);
    let volume = parse(&image).expect("valid BPB");

    // (1 reserved + 2 FATs * 9) * 512 = 9728.
    assert_eq!(volume.root_directory_offset, 9_728);
    assert_eq!(volume.root_directory_bytes, 224 * 32);
    assert_eq!(volume.fat_type, FatType::Fat12);
    assert!(volume.root_directory_offset <= MAX_DISK_FORMAT_OFFSET);

    let inspection = inspect_bytes("bounded", "img", &image);
    assert!(inspection.bytes_inspected <= MAX_DISK_FORMAT_BYTES_READ);
    // boot sector + exactly the root directory.
    assert_eq!(
        inspection.bytes_inspected,
        FAT_BOOT_SECTOR_BYTES as u64 + u64::from(volume.root_directory_bytes)
    );
}

#[test]
fn root_directory_names_are_matched_case_insensitively() {
    // FAT stores names upper-cased; a lower-cased entry must still match.
    let image = fat_image(
        &floppy_1440(),
        &[dir_entry("io", "sys", SYS), dir_entry("MsDoS", "SyS", SYS)],
        None,
    );
    let inspection = inspect_bytes("case", "img", &image);
    assert_eq!(inspection.boot_families, vec![DosBootFamily::MsDos]);
}

#[test]
fn extension_is_irrelevant_same_result_for_img_ima_and_anything_else() {
    let image = fat_image(&floppy_1440(), &[io_sys(), msdos_sys()], None);
    let a = inspect_bytes("ext-img", "img", &image);
    let b = inspect_bytes("ext-ima", "ima", &image);
    let c = inspect_bytes("ext-bin", "bin", &image);
    assert_eq!(a.boot_families, b.boot_families);
    assert_eq!(b.boot_families, c.boot_families);
    assert_eq!(a.boot_families, vec![DosBootFamily::MsDos]);
}

#[test]
fn fat16_image_with_io_and_msdos_is_msdos_family_boot_media() {
    let image = fat_image(&small_fat16(), &[io_sys(), msdos_sys()], None);
    let volume = parse(&image).expect("valid FAT16 BPB");
    assert_eq!(volume.fat_type, FatType::Fat16);
    assert!(volume.root_directory_offset <= MAX_DISK_FORMAT_OFFSET);

    let inspection = inspect_bytes("msdos16", "img", &image);
    assert_eq!(inspection.boot_families, vec![DosBootFamily::MsDos]);
    assert_eq!(observe_dos_boot_evidence(&inspection).len(), 1);
}

// --- Negative tests (section 8) ------------------------------------------

#[test]
fn valid_fat12_with_no_dos_boot_files_yields_no_dos_evidence() {
    let image = fat_image(
        &floppy_1440(),
        &[
            dir_entry("README", "TXT", 0x20),
            dir_entry("GAME", "EXE", 0x20),
        ],
        None,
    );
    let inspection = inspect_bytes("plainfat", "img", &image);

    assert!(inspection.refusal.is_none());
    assert!(
        inspection.filesystem.is_some(),
        "the FAT volume still parses"
    );
    assert!(inspection.boot_families.is_empty());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
    assert!(
        inspection
            .observations
            .iter()
            .any(|line| line.contains("FAT structure alone is not DOS evidence"))
    );
}

#[test]
fn command_com_alone_is_not_enough() {
    let image = fat_image(&floppy_1440(), &[command_com()], None);
    let inspection = inspect_bytes("cmdonly", "img", &image);
    assert!(inspection.command_com_present);
    assert!(inspection.boot_families.is_empty());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
}

#[test]
fn io_sys_alone_without_msdos_sys_is_not_enough() {
    let image = fat_image(&floppy_1440(), &[io_sys()], None);
    let inspection = inspect_bytes("ioonly", "img", &image);
    assert!(inspection.boot_families.is_empty());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
}

#[test]
fn one_file_from_each_pair_crossed_is_not_enough() {
    // IO.SYS + IBMDOS.COM: one file from each documented pair, neither pair complete.
    let image = fat_image(&floppy_1440(), &[io_sys(), ibmdos_com()], None);
    let inspection = inspect_bytes("crossed", "img", &image);
    assert!(inspection.boot_families.is_empty());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
}

#[test]
fn malformed_bpb_is_refused() {
    let mut image = fat_image(&floppy_1440(), &[io_sys(), msdos_sys()], None);
    image[0x0B..0x0D].copy_from_slice(&0_u16.to_le_bytes()); // zero bytes per sector
    let inspection = inspect_bytes("badbpb", "img", &image);
    assert!(inspection.filesystem.is_none());
    assert!(inspection.refusal.is_some());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
}

#[test]
fn missing_boot_signature_is_refused() {
    let mut image = fat_image(&floppy_1440(), &[io_sys(), msdos_sys()], None);
    image[BOOT_SIGNATURE_OFFSET] = 0x00;
    image[BOOT_SIGNATURE_OFFSET + 1] = 0x00;
    assert!(parse(&image).is_err());
}

#[test]
fn root_directory_past_the_inspection_window_is_refused() {
    let mut bpb = floppy_1440();
    bpb.reserved_sectors = 70; // pushes the root dir past MAX_DISK_FORMAT_OFFSET
    let image = fat_image(&bpb, &[], Some(2880));
    assert!(parse(&image).is_err());
}

#[test]
fn root_directory_beyond_end_of_file_is_refused() {
    let full = fat_image(&floppy_1440(), &[io_sys(), msdos_sys()], None);
    let truncated = &full[..5_000]; // ends before the root directory
    match parse_fat_bpb(truncated, truncated.len() as u64) {
        Err(DiskFormatRefusal::Truncated { .. }) => {}
        other => panic!("expected Truncated, got {other:?}"),
    }
}

#[test]
fn impossible_sector_geometry_is_refused() {
    let mut image = fat_image(&floppy_1440(), &[io_sys(), msdos_sys()], None);
    image[0x13..0x15].copy_from_slice(&10_u16.to_le_bytes()); // 10 total sectors
    assert!(parse(&image).is_err());
}

#[test]
fn random_bytes_named_img_yield_no_dos_evidence() {
    let noise: Vec<u8> = (0..4096).map(|i| (i * 37 + 11) as u8).collect();
    let inspection = inspect_bytes("noise", "img", &noise);
    assert!(inspection.filesystem.is_none());
    assert!(inspection.refusal.is_some());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
}

#[test]
fn atari_st_geometry_fat_disk_without_system_files_yields_no_dos_evidence() {
    let image = fat_image(&atari_st_720(), &[dir_entry("GAME", "PRG", 0x20)], None);
    let inspection = inspect_bytes("atarist", "img", &image);
    assert!(inspection.filesystem.is_some(), "a valid FAT12 BPB");
    assert!(inspection.boot_families.is_empty());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
}

#[test]
fn generic_fat16_media_without_system_files_yields_no_dos_evidence() {
    // Stands in for PC-98 / X68000 style generic FAT media.
    let image = fat_image(&small_fat16(), &[dir_entry("DATA", "BIN", 0x20)], None);
    let inspection = inspect_bytes("genfat16", "img", &image);
    assert!(inspection.filesystem.is_some());
    assert!(inspection.boot_families.is_empty());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
}

#[test]
fn image_file_name_containing_dos_is_irrelevant() {
    let image = fat_image(&floppy_1440(), &[dir_entry("SETUP", "EXE", 0x20)], None);
    // The file itself is called "...DOS...": the module must not care.
    let inspection = inspect_bytes("MEGA-DOS-COLLECTION", "img", &image);
    assert!(inspection.boot_families.is_empty());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
}

#[test]
fn volume_label_reading_msdos_is_irrelevant() {
    let label = dir_entry("MSDOS", "", 0x08); // ATTR_VOLUME_ID
    let image = fat_image(
        &floppy_1440(),
        &[label, dir_entry("GAME", "EXE", 0x20)],
        None,
    );
    let inspection = inspect_bytes("vollabel", "img", &image);
    assert!(inspection.boot_families.is_empty());
    assert!(observe_dos_boot_evidence(&inspection).is_empty());
    // And short_name itself never returns the label.
    assert_eq!(short_name(&label), None);
}

#[test]
fn subdirectory_named_like_a_system_file_is_ignored() {
    let dir = dir_entry("IO", "SYS", 0x10); // ATTR_DIRECTORY
    let image = fat_image(&floppy_1440(), &[dir, msdos_sys()], None);
    let inspection = inspect_bytes("subdir", "img", &image);
    assert!(
        inspection.boot_families.is_empty(),
        "a directory entry must not satisfy the IO.SYS leg"
    );
}

#[test]
fn long_file_name_fragments_are_skipped() {
    let mut lfn = [0_u8; 32];
    lfn[0] = 0x41;
    lfn[11] = 0x0F; // long-name attribute
    assert_eq!(short_name(&lfn), None);
}

#[test]
fn deleted_and_end_marker_entries_are_skipped() {
    let mut deleted = io_sys();
    deleted[0] = 0xE5;
    assert_eq!(short_name(&deleted), None);
    assert_eq!(short_name(&[0_u8; 32]), None);
}

#[test]
fn inspection_is_deterministic() {
    let image = fat_image(
        &floppy_1440(),
        &[io_sys(), msdos_sys(), command_com()],
        None,
    );
    let first = inspect_bytes("det1", "img", &image);
    let second = inspect_bytes("det2", "img", &image);
    assert_eq!(first.boot_families, second.boot_families);
    assert_eq!(first.observations, second.observations);
    assert_eq!(
        observe_dos_boot_evidence(&first),
        observe_dos_boot_evidence(&second)
    );
}

#[test]
fn evidence_never_names_a_platform_or_product_code() {
    let image = fat_image(&floppy_1440(), &[io_sys(), msdos_sys()], None);
    let inspection = inspect_bytes("neutral", "img", &image);
    for fact in observe_dos_boot_evidence(&inspection) {
        assert_eq!(fact.kind, ContentEvidenceKind::BootStructure);
        assert_ne!(fact.kind, ContentEvidenceKind::ProductCode);
    }
}

#[test]
fn nonexistent_path_is_refused_not_panicked() {
    let path = PathBuf::from("/archivefs/does/not/exist.img");
    let inspection = inspect_dos_boot_media(&path, &TrustedRoots::none(), None);
    assert!(inspection.filesystem.is_none());
    assert!(inspection.refusal.is_some());
}
