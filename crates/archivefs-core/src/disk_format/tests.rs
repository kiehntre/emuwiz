//! Tests for the shared disk-format layer.
//!
//! Images are generated rather than committed: an `.st` is a raw sector dump and
//! a `.stx` is a header plus a track table, so both are built exactly, byte for
//! byte, from the structures the adapters document. That also means a hostile
//! case can be constructed precisely - a fuzzy mask that does not fit its record,
//! a record length that overflows a `u64` - rather than approximated.

use super::*;
use crate::platform::{
    DetectionConfidence, DetectionRequest, DetectionSource, detect_platform_report,
};
use std::path::PathBuf;

fn hdi_image(
    header_size: u32,
    sector_size: u32,
    sectors: u32,
    heads: u32,
    cylinders: u32,
) -> Vec<u8> {
    let payload = u64::from(sector_size)
        .checked_mul(u64::from(sectors))
        .and_then(|value| value.checked_mul(u64::from(heads)))
        .and_then(|value| value.checked_mul(u64::from(cylinders)))
        .unwrap() as usize;
    let mut image = vec![0; header_size as usize + payload];
    image[8..12].copy_from_slice(&header_size.to_le_bytes());
    image[12..16].copy_from_slice(&(payload as u32).to_le_bytes());
    image[16..20].copy_from_slice(&sector_size.to_le_bytes());
    image[20..24].copy_from_slice(&sectors.to_le_bytes());
    image[24..28].copy_from_slice(&heads.to_le_bytes());
    image[28..32].copy_from_slice(&cylinders.to_le_bytes());
    image
}

fn nhd_image(
    header_size: u32,
    sector_size: u16,
    sectors: u16,
    heads: u16,
    cylinders: u32,
) -> Vec<u8> {
    let payload = u64::from(sector_size)
        .checked_mul(u64::from(sectors))
        .and_then(|value| value.checked_mul(u64::from(heads)))
        .and_then(|value| value.checked_mul(u64::from(cylinders)))
        .unwrap() as usize;
    let mut image = vec![0; header_size as usize + payload];
    image[..15].copy_from_slice(b"T98HDDIMAGE.R0\0");
    image[0x110..0x114].copy_from_slice(&header_size.to_le_bytes());
    image[0x114..0x118].copy_from_slice(&cylinders.to_le_bytes());
    image[0x118..0x11a].copy_from_slice(&heads.to_le_bytes());
    image[0x11a..0x11c].copy_from_slice(&sectors.to_le_bytes());
    image[0x11c..0x11e].copy_from_slice(&sector_size.to_le_bytes());
    image[0x10..0x18].copy_from_slice(b"TEST NHD");
    image
}

/// A throwaway tree with a trusted library root and an untrusted one beside it.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "archivefs-disk-format-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for directory in ["library", "library/atari-st", "downloads", "elsewhere"] {
            std::fs::create_dir_all(root.join(directory)).expect("fixture");
        }
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture");
        }
        std::fs::write(&path, bytes).expect("fixture");
        path
    }

    #[cfg(unix)]
    fn link(&self, from: &str, to: &Path) -> PathBuf {
        let path = self.path(from);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture");
        }
        std::os::unix::fs::symlink(to, &path).expect("fixture");
        path
    }

    fn trusted(&self) -> TrustedRoots {
        TrustedRoots::from_paths([self.path("library"), self.path("downloads")])
    }

    fn inspect(&self, path: &Path) -> DiskFormatEvidence {
        inspect_disk_format(path, &self.trusted(), DiskFormatContext::default(), None)
    }

    fn inspect_in_folder(&self, path: &Path, folder_platform: &str) -> DiskFormatEvidence {
        inspect_disk_format(
            path,
            &self.trusted(),
            DiskFormatContext {
                folder_platform: Some(folder_platform),
            },
            None,
        )
    }

    /// Path, type, size, mtime and mode for everything in the tree.
    fn snapshot(&self) -> std::collections::BTreeMap<String, String> {
        let mut entries = std::collections::BTreeMap::new();
        let mut stack = vec![self.root.clone()];
        while let Some(current) = stack.pop() {
            let Ok(read_dir) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in read_dir.filter_map(Result::ok) {
                let path = entry.path();
                let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    stack.push(path.clone());
                }
                #[cfg(unix)]
                let mode = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode()
                };
                #[cfg(not(unix))]
                let mode = 0_u32;
                entries.insert(
                    path.to_string_lossy().into_owned(),
                    format!(
                        "{:?}|{}|{mode:o}|{:?}",
                        metadata.file_type(),
                        metadata.len(),
                        metadata.modified().ok()
                    ),
                );
            }
        }
        entries
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn dfs_side(total_sectors: u16, title: &str, filename: &[u8; 7], start: u16) -> Vec<u8> {
    let mut image = vec![0_u8; usize::from(total_sectors) * 256];
    let title = title.as_bytes();
    image[0..8].fill(b' ');
    image[0..title.len().min(8)].copy_from_slice(&title[..title.len().min(8)]);
    image[256..260].fill(b' ');
    if title.len() > 8 {
        image[256..256 + (title.len() - 8).min(4)].copy_from_slice(&title[8..title.len().min(12)]);
    }
    image[260] = 0x12; // valid BCD catalogue cycle number
    image[261] = 8; // one eight-byte entry
    image[262] = 1; // 400 sectors: high two sector bits = 1, boot option 0
    image[263] = 0x90;
    image[8..15].copy_from_slice(filename);
    image[15] = b'$' | 0x80; // root directory, locked
    let details = 256 + 8;
    image[details..details + 2].copy_from_slice(&0x1900_u16.to_le_bytes());
    image[details + 2..details + 4].copy_from_slice(&0x1900_u16.to_le_bytes());
    image[details + 4..details + 6].copy_from_slice(&3_u16.to_le_bytes());
    image[details + 6] = ((start >> 8) & 3) as u8;
    image[details + 7] = start as u8;
    image
}

fn valid_dfs_ssd() -> Vec<u8> {
    dfs_side(400, "EXAMPLE DFS", b"!BOOT  ", 2)
}

fn valid_dfs_dsd() -> Vec<u8> {
    let mut image = vec![0_u8; 800 * 256];
    let side0 = dfs_side(400, "SIDE ZERO", b"ONE    ", 2);
    let side1 = dfs_side(400, "SIDE ONE", b"TWO    ", 2);
    image[..512].copy_from_slice(&side0[..512]);
    image[0x0a00..0x0a00 + 512].copy_from_slice(&side1[..512]);
    image
}

#[test]
fn standard_dfs_ssd_catalogue_is_valid_and_exposes_title_and_entry_metadata() {
    let fixture = Fixture::new("dfs-ssd");
    let path = fixture.write("library/example.ssd", &valid_dfs_ssd());
    let evidence = fixture.inspect(&path);
    assert_eq!(
        evidence.format,
        Some(DiskFormat::AcornDfsDisk),
        "{evidence:?}"
    );
    assert!(!evidence.conclusive);
    let Some(DiskFormatMetadata::Dfs(layout)) = evidence.metadata else {
        panic!("expected DFS metadata: {:?}", evidence.metadata);
    };
    assert_eq!(layout.sides.len(), 1);
    assert_eq!(layout.sides[0].title, "EXAMPLE DFS");
    assert_eq!(layout.sides[0].files[0].filename, "!BOOT");
    assert!(layout.sides[0].files[0].locked);
    assert_eq!(layout.sides[0].files[0].start_sector, 2);
    assert_eq!(layout.sides[0].files[0].load_address, 0x1900);
}

#[test]
fn standard_dfs_dsd_catalogues_validate_on_both_interleaved_sides() {
    let fixture = Fixture::new("dfs-dsd");
    let path = fixture.write("library/example.dsd", &valid_dfs_dsd());
    let evidence = fixture.inspect(&path);
    assert_eq!(
        evidence.format,
        Some(DiskFormat::AcornDfsDisk),
        "{evidence:?}"
    );
    let Some(DiskFormatMetadata::Dfs(layout)) = evidence.metadata else {
        panic!("expected DFS metadata: {:?}", evidence.metadata);
    };
    assert!(layout.double_sided);
    assert_eq!(layout.sides.len(), 2);
    assert_eq!(layout.sides[0].title, "SIDE ZERO");
    assert_eq!(layout.sides[1].title, "SIDE ONE");
}

#[test]
fn dfs_random_truncated_and_impossible_entries_fail_closed() {
    let fixture = Fixture::new("dfs-negative");
    for (extension, size) in [("ssd", 100 * 1024), ("dsd", 200 * 1024)] {
        let random = fixture.write(&format!("library/random.{extension}"), &vec![0xA5; size]);
        assert!(!fixture.inspect(&random).is_recognised());
    }
    let short = fixture.write("library/short.ssd", &vec![0; 511]);
    assert!(!fixture.inspect(&short).is_recognised());
    let mut impossible = valid_dfs_ssd();
    impossible[256 + 8 + 6] = 1; // start sector 0x190, past a 400-sector side once length is applied
    impossible[256 + 8 + 7] = 0x90;
    impossible[256 + 8 + 4] = 0xff;
    impossible[256 + 8 + 5] = 0xff;
    let impossible = fixture.write("library/impossible.ssd", &impossible);
    assert!(!fixture.inspect(&impossible).is_recognised());
    let mut malformed = valid_dfs_ssd();
    malformed[8] = 0x01; // control byte in a declared filename
    let malformed = fixture.write("library/malformed.ssd", &malformed);
    assert!(!fixture.inspect(&malformed).is_recognised());
}

#[test]
fn dfs_inspection_reads_only_bounded_catalogue_data_and_not_adfs_extensions() {
    let fixture = Fixture::new("dfs-bounds");
    let path = fixture.write("library/example.ssd", &valid_dfs_ssd());
    let evidence = fixture.inspect(&path);
    assert!(evidence.bytes_inspected <= 2048);
    for extension in ["adl", "adf"] {
        let path = fixture.write(&format!("library/not-dfs.{extension}"), &valid_dfs_ssd());
        assert_eq!(
            fixture
                .inspect(&path)
                .refusal
                .as_ref()
                .map(DiskFormatRefusal::code),
            Some("no_adapter")
        );
    }
}

#[test]
fn dfs_structure_is_ambiguous_but_folder_evidence_resolves_the_canonical_platform() {
    let fixture = Fixture::new("dfs-platform");
    let bare = fixture.write("library/unsorted/example.ssd", &valid_dfs_ssd());
    let bare_report = detect_platform_report(
        &DetectionRequest::new(&bare, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    assert_eq!(bare_report.platform, None);
    assert_eq!(bare_report.confidence, DetectionConfidence::Ambiguous);
    assert!(
        bare_report
            .candidates
            .iter()
            .any(|candidate| candidate.platform == "BBC Micro")
    );
    assert!(
        bare_report
            .candidates
            .iter()
            .any(|candidate| candidate.platform == "Acorn Electron")
    );

    for (folder, expected) in [
        ("bbcmicro", "BBC Micro"),
        ("bbcmaster", "BBC Micro"),
        ("electron", "Acorn Electron"),
    ] {
        let path = fixture.write(&format!("library/{folder}/example.ssd"), &valid_dfs_ssd());
        let report = detect_platform_report(
            &DetectionRequest::new(&path, &fixture.path("library"))
                .inspecting_content()
                .with_trusted_roots(fixture.trusted()),
        );
        assert_eq!(report.platform, Some(expected), "folder {folder}");
    }
}

// --- Image builders -------------------------------------------------------

/// One Atari ST floppy image, built exactly from a geometry.
///
/// Writes a real FAT12 BPB at the documented offsets and pads to the full sector
/// count, so the resulting bytes are what a TOS format would have produced as far
/// as the adapter looks.
fn st_image(sides: u16, sectors_per_track: u16, tracks: u16) -> Vec<u8> {
    let total_sectors = u32::from(sectors_per_track) * u32::from(sides) * u32::from(tracks);
    let mut image = vec![0_u8; (total_sectors * 512) as usize];
    // 0x00: a branch instruction, as a real boot sector has.
    image[0x00] = 0x60;
    image[0x01] = 0x1c;
    image[0x02..0x08].copy_from_slice(b"Loader");
    image[0x0B..0x0D].copy_from_slice(&512_u16.to_le_bytes()); // bytes per sector
    image[0x0D] = 2; // sectors per cluster
    image[0x0E..0x10].copy_from_slice(&1_u16.to_le_bytes()); // reserved sectors
    image[0x10] = 2; // FATs
    // 112 root entries is the TOS default for both single- and double-sided.
    image[0x11..0x13].copy_from_slice(&112_u16.to_le_bytes());
    image[0x13..0x15].copy_from_slice(&u16::try_from(total_sectors).expect("fits").to_le_bytes());
    image[0x15] = 0xf9; // media descriptor
    image[0x16..0x18].copy_from_slice(&5_u16.to_le_bytes()); // sectors per FAT
    image[0x18..0x1A].copy_from_slice(&sectors_per_track.to_le_bytes());
    image[0x1A..0x1C].copy_from_slice(&sides.to_le_bytes());
    image
}

/// The standard double-sided 720 KB ST disk.
fn st_720k() -> Vec<u8> {
    st_image(2, 9, 80)
}

/// One Pasti container with `tracks` minimal track records.
fn stx_image(version: u16, track_count: u8, record_length: u32) -> Vec<u8> {
    let mut image = Vec::new();
    image.extend_from_slice(b"RSY\0");
    image.extend_from_slice(&version.to_le_bytes());
    image.extend_from_slice(&1_u16.to_le_bytes()); // tool
    image.extend_from_slice(&0_u16.to_le_bytes()); // reserved
    image.push(track_count);
    image.push(0); // revision
    image.extend_from_slice(&0_u32.to_le_bytes()); // reserved
    assert_eq!(image.len(), 16, "the file header is 16 bytes");
    for index in 0..track_count {
        let start = image.len();
        image.extend_from_slice(&record_length.to_le_bytes()); // record length
        image.extend_from_slice(&0_u32.to_le_bytes()); // fuzzy length
        image.extend_from_slice(&9_u16.to_le_bytes()); // sector count
        image.extend_from_slice(&0_u16.to_le_bytes()); // flags
        image.extend_from_slice(&6250_u16.to_le_bytes()); // MFM track length
        image.push(index); // track number, side 0
        image.push(0); // record type
        assert_eq!(image.len() - start, 16, "a track header is 16 bytes");
        image.resize(start + record_length as usize, 0);
    }
    image
}

fn valid_stx() -> Vec<u8> {
    stx_image(3, 10, 64)
}

/// A minimal one-track, one-sector D88 container.
fn d88_image(name: &str) -> Vec<u8> {
    let track_offset = D88_HEADER_BYTES as u32;
    let track_bytes = 16 + 128;
    let mut image = vec![0u8; D88_HEADER_BYTES + track_bytes];
    let name_bytes = name.as_bytes();
    image[..name_bytes.len().min(17)].copy_from_slice(&name_bytes[..name_bytes.len().min(17)]);
    image[0x1a] = 0; // write enabled
    image[0x1b] = 0; // 2D media
    image[0x1c..0x20].copy_from_slice(&track_offset.to_le_bytes());
    let track = D88_HEADER_BYTES;
    image[track] = 0; // C
    image[track + 1] = 0; // H
    image[track + 2] = 1; // R
    image[track + 3] = 0; // N = 128 bytes
    image[track + 4..track + 6].copy_from_slice(&1u16.to_le_bytes());
    image[track + 6] = 0; // density
    image[track + 7] = 0; // deleted-data flag
    image[track + 8] = 0; // status/CRC
    image[track + 14..track + 16].copy_from_slice(&128u16.to_le_bytes());
    image[track + 16..].fill(0xe5);
    image
}

// --- Limits ---------------------------------------------------------------

/// Test 1
#[test]
fn limits_are_internally_consistent() {
    // Compile-time, because these are all constants: a change that breaks the
    // relationship should fail the build, not one test run.
    const _: () = assert!(MAX_DISK_FORMAT_READ_CHUNK as u64 <= MAX_DISK_FORMAT_BYTES_READ);
    const _: () = assert!(MAX_DISK_FORMAT_OFFSET <= MAX_PASTI_BYTES);
    const _: () = assert!(MAX_RAW_FLOPPY_BYTES < MAX_PASTI_BYTES);
    // A boot sector must be readable in one chunk.
    const _: () = assert!(512 <= MAX_DISK_FORMAT_READ_CHUNK);
    // The whole Pasti track table must fit in the budget: one 16-byte header per
    // record, plus the file header.
    let table = 16 * MAX_PASTI_TRACK_RECORDS as u64 + 16;
    assert!(
        table <= MAX_DISK_FORMAT_BYTES_READ,
        "the read budget must admit a full track table"
    );
}

// --- Atari ST: valid ------------------------------------------------------

/// Test 2
#[test]
fn a_standard_720k_st_image_is_recognised() {
    let fixture = Fixture::new("st-720k");
    let image = fixture.write("library/game.st", &st_720k());
    let evidence = fixture.inspect(&image);

    assert_eq!(evidence.format, Some(DiskFormat::AtariStRawFloppy));
    assert_eq!(evidence.platform, Some("AtariST"));
    assert_eq!(
        evidence.confidence,
        DetectionConfidence::Probable,
        "a FAT12 boot sector is shared with PC DOS floppies, so alone it is not proof"
    );
    assert!(!evidence.conclusive);
    let Some(DiskFormatMetadata::Floppy(geometry)) = evidence.metadata else {
        panic!("expected floppy geometry: {:?}", evidence.metadata);
    };
    assert_eq!(geometry.sides, 2);
    assert_eq!(geometry.sectors_per_track, 9);
    assert_eq!(geometry.tracks, 80);
    assert_eq!(geometry.total_sectors, 1440);
    assert_eq!(
        evidence.bytes_inspected, 512,
        "one boot sector, nothing more"
    );
    assert!(
        evidence
            .evidence
            .iter()
            .any(|item| item.contains("same FAT12 structure a PC DOS floppy")),
        "the limit of what this proves must be stated: {:?}",
        evidence.evidence
    );
}

#[test]
fn a_minimal_d88_container_is_recognised_without_platform_identity() {
    let fixture = Fixture::new("d88-minimal");
    let image = fixture.write("library/disk.d88", &d88_image("PC88 TEST DISK"));
    let evidence = fixture.inspect(&image);

    assert_eq!(evidence.format, Some(DiskFormat::D88Container));
    assert_eq!(evidence.platform, Some("NEC PC-8801"));
    assert_eq!(evidence.confidence, DetectionConfidence::Probable);
    assert!(!evidence.conclusive);
    assert!(
        evidence
            .evidence
            .iter()
            .any(|item| item.contains("PC88 TEST DISK"))
    );
    assert!(
        evidence
            .evidence
            .iter()
            .any(|item| item.contains("write-protect"))
    );
    assert!(evidence.evidence.iter().any(|item| item.contains("0x00")));
    match evidence.metadata {
        Some(DiskFormatMetadata::D88(layout)) => {
            assert_eq!(layout.disk_name[..14], *b"PC88 TEST DISK");
            assert!(!layout.write_protected);
            assert_eq!(layout.media_type, 0);
            assert_eq!(layout.declared_track_entries, 1);
            assert_eq!(layout.validated_track_entries, 1);
            assert_eq!(layout.declared_sectors, 1);
            assert_eq!(layout.declared_data_bytes, 128);
        }
        metadata => panic!("expected D88 metadata, got {metadata:?}"),
    }
}

#[test]
fn d88_folder_context_does_not_turn_shared_structure_into_machine_proof() {
    let fixture = Fixture::new("d88-context");
    let image = fixture.write("library/disk.d88", &d88_image("SHARED"));
    let evidence = fixture.inspect_in_folder(&image, "PC-98");
    assert_eq!(evidence.format, Some(DiskFormat::D88Container));
    assert_eq!(evidence.confidence, DetectionConfidence::Ambiguous);
    assert!(!evidence.conclusive);
}

#[test]
fn malformed_d88_structures_fail_closed() {
    let fixture = Fixture::new("d88-invalid");
    let cases = [
        ("random", vec![0x5a; D88_HEADER_BYTES + 144]),
        (
            "truncated",
            d88_image("TRUNCATED")[..D88_HEADER_BYTES - 1].to_vec(),
        ),
        ("past-eof", {
            let mut bytes = d88_image("PAST EOF");
            bytes[0x1c..0x20].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
            bytes
        }),
        ("bad-sector-header", {
            let mut bytes = d88_image("BAD HEADER");
            bytes[D88_HEADER_BYTES + 3] = 7;
            bytes
        }),
        ("bad-sector-size", {
            let mut bytes = d88_image("BAD SIZE");
            bytes[D88_HEADER_BYTES + 14..D88_HEADER_BYTES + 16]
                .copy_from_slice(&64u16.to_le_bytes());
            bytes
        }),
    ];
    for (label, bytes) in cases {
        let image = fixture.write(&format!("library/{label}.d88"), &bytes);
        let evidence = fixture.inspect(&image);
        assert_eq!(evidence.format, None, "{label} was accepted: {evidence:?}");
        assert!(evidence.refusal.is_some(), "{label} had no refusal");
    }
}

/// Test 3
#[test]
fn a_second_valid_geometry_is_recognised() {
    let fixture = Fixture::new("st-880k");
    // Single-sided 400 KB and the 11-sector 880 KB extended format.
    for (sides, spt, tracks, expected_sectors) in
        [(1_u16, 10_u16, 80_u16, 800_u32), (2, 11, 80, 1760)]
    {
        let image = fixture.write(
            &format!("library/g{sides}{spt}.st"),
            &st_image(sides, spt, tracks),
        );
        let evidence = fixture.inspect(&image);
        assert_eq!(
            evidence.format,
            Some(DiskFormat::AtariStRawFloppy),
            "{sides} sides x {spt} sectors should validate: {:?}",
            evidence.refusal
        );
        let Some(DiskFormatMetadata::Floppy(geometry)) = evidence.metadata else {
            panic!("expected geometry");
        };
        assert_eq!(geometry.total_sectors, expected_sectors);
    }
}

/// Test 4
#[test]
fn an_st_image_in_an_atari_st_folder_is_confirmed() {
    let fixture = Fixture::new("st-folder");
    let image = fixture.write("library/atari-st/game.st", &st_720k());
    let evidence = fixture.inspect_in_folder(&image, "AtariST");
    assert_eq!(evidence.confidence, DetectionConfidence::Confirmed);
    assert!(evidence.conclusive);
    assert!(
        evidence
            .evidence
            .iter()
            .any(|item| item.contains("raises this to confirmed"))
    );
}

/// Test 5
#[test]
fn an_st_image_under_a_conflicting_folder_is_ambiguous() {
    let fixture = Fixture::new("st-conflict");
    let image = fixture.write("library/megadrive/game.st", &st_720k());
    let evidence = fixture.inspect_in_folder(&image, "MegaDrive");
    assert_eq!(
        evidence.confidence,
        DetectionConfidence::Ambiguous,
        "structure and folder disagree, so the structure claims nothing on its own"
    );
    assert!(!evidence.conclusive);
    assert!(
        evidence
            .evidence
            .iter()
            .any(|item| item.contains("names MegaDrive instead"))
    );
}

// --- Atari ST: refused ----------------------------------------------------

/// Test 6
#[test]
fn a_truncated_st_image_is_refused() {
    let fixture = Fixture::new("st-truncated");
    let mut image = st_720k();
    image.truncate(512 * 700); // fewer sectors than the boot sector declares
    let path = fixture.write("library/game.st", &image);
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, None);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("geometry_mismatch"),
        "{:?}",
        evidence.refusal
    );
}

/// Test 7
#[test]
fn an_st_image_shorter_than_a_sector_is_refused() {
    let fixture = Fixture::new("st-tiny");
    let path = fixture.write("library/game.st", &[0_u8; 64]);
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("too_small")
    );
}

/// Test 8
#[test]
fn an_st_image_that_is_not_sector_aligned_is_refused() {
    let fixture = Fixture::new("st-unaligned");
    let mut image = st_720k();
    image.push(0); // one byte past a whole sector
    let path = fixture.write("library/game.st", &image);
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("not_sector_aligned")
    );
}

/// Test 9
#[test]
fn impossible_st_geometry_is_refused() {
    let fixture = Fixture::new("st-impossible");
    // Each case keeps the file length valid but corrupts one BPB field, so the
    // refusal can only come from the geometry check it targets.
    let cases: &[(&str, usize, &[u8])] = &[
        ("40 sectors per track", 0x18, &40_u16.to_le_bytes()),
        ("5 sides", 0x1A, &5_u16.to_le_bytes()),
        ("1024-byte sectors", 0x0B, &1024_u16.to_le_bytes()),
        ("zero sides", 0x1A, &0_u16.to_le_bytes()),
        ("zero sectors per track", 0x18, &0_u16.to_le_bytes()),
        ("7 FATs", 0x10, &[7]),
        ("3 sectors per cluster", 0x0D, &[3]),
        ("100 root entries", 0x11, &100_u16.to_le_bytes()),
        ("zero total sectors", 0x13, &0_u16.to_le_bytes()),
    ];
    for (label, offset, bytes) in cases {
        let mut image = st_720k();
        image[*offset..*offset + bytes.len()].copy_from_slice(bytes);
        let path = fixture.write("library/broken.st", &image);
        let evidence = fixture.inspect(&path);
        assert_eq!(
            evidence.format, None,
            "{label} must not be accepted as an Atari ST image"
        );
        assert!(
            evidence.refusal.is_some(),
            "{label} must be refused with a reason"
        );
    }
}

/// Test 10
#[test]
fn a_geometry_that_does_not_match_the_file_length_is_refused() {
    let fixture = Fixture::new("st-mismatch");
    // A perfectly valid 80-track, 2-side, 9-sector boot sector - 1440 sectors,
    // 737,280 bytes - in a file padded out to 1600 sectors. The geometry passes
    // every plausibility check, so the only thing wrong is that it does not
    // account for the file's length.
    let mut image = st_720k();
    image.resize(1600 * 512, 0);
    let path = fixture.write("library/game.st", &image);
    let evidence = fixture.inspect(&path);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("geometry_mismatch"),
        "{:?}",
        evidence.refusal
    );
    let detail = evidence.refusal.as_ref().expect("refused").detail();
    assert!(
        detail.contains("737280") && detail.contains("819200"),
        "the refusal should name both sizes: {detail}"
    );
}

/// Test 11
#[test]
fn random_data_with_an_st_extension_is_refused() {
    let fixture = Fixture::new("st-random");
    // Sector-aligned, plausible length, but no coherent boot sector. This is the
    // case that must not be waved through on size alone.
    let mut image = vec![0_u8; 737_280];
    for (index, byte) in image.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(31).wrapping_add(7);
    }
    let path = fixture.write("library/noise.st", &image);
    let evidence = fixture.inspect(&path);
    assert_eq!(
        evidence.format, None,
        "a sector-aligned file of the right size is not enough: {:?}",
        evidence.evidence
    );
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("malformed")
    );
}

/// Test 12
#[test]
fn an_all_zero_st_image_is_refused() {
    let fixture = Fixture::new("st-zero");
    let path = fixture.write("library/blank.st", &vec![0_u8; 737_280]);
    assert_eq!(fixture.inspect(&path).format, None);
}

/// Test 13
#[test]
fn an_oversized_st_file_is_refused_before_it_is_read() {
    let fixture = Fixture::new("st-huge");
    // Sparse: only the length matters, and the adapter must refuse on length
    // alone without reading a byte.
    let path = fixture.path("library/huge.st");
    let file = std::fs::File::create(&path).expect("fixture");
    file.set_len(MAX_RAW_FLOPPY_BYTES + 512).expect("fixture");
    drop(file);
    let evidence = fixture.inspect(&path);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("too_large")
    );
    assert_eq!(evidence.bytes_inspected, 0, "refused before any read");
}

// --- Pasti (.stx) ---------------------------------------------------------

/// Test 14
#[test]
fn a_minimal_valid_pasti_container_is_recognised_and_conclusive() {
    let fixture = Fixture::new("stx-valid");
    let path = fixture.write("library/game.stx", &valid_stx());
    let evidence = fixture.inspect(&path);

    assert_eq!(evidence.format, Some(DiskFormat::AtariStPasti));
    assert_eq!(evidence.platform, Some("AtariST"));
    assert_eq!(
        evidence.confidence,
        DetectionConfidence::Confirmed,
        "Pasti exists only for Atari ST media, so a valid container settles it"
    );
    assert!(evidence.conclusive);
    let Some(DiskFormatMetadata::Pasti(layout)) = evidence.metadata else {
        panic!("expected Pasti layout");
    };
    assert_eq!(layout.version, 3);
    assert_eq!(layout.declared_track_records, 10);
    assert_eq!(layout.validated_track_records, 10);
    assert_eq!(layout.declared_sectors, 90);
    assert!(
        evidence
            .evidence
            .iter()
            .any(|item| item.contains("never reconstructed")),
        "the report must state that the disk was not decoded"
    );
}

/// Test 15
#[test]
fn an_unsupported_pasti_version_is_refused_rather_than_guessed_at() {
    let fixture = Fixture::new("stx-version");
    for version in [0_u16, 1, 2, 4, 99, u16::MAX] {
        let path = fixture.write("library/game.stx", &stx_image(version, 4, 64));
        let evidence = fixture.inspect(&path);
        assert_eq!(
            evidence.format, None,
            "version {version} must not be claimed"
        );
        assert!(
            evidence
                .refusal
                .as_ref()
                .expect("refused")
                .detail()
                .contains("not one this build understands")
        );
    }
}

/// Test 16
#[test]
fn a_file_without_the_pasti_signature_is_refused() {
    let fixture = Fixture::new("stx-signature");
    let mut image = valid_stx();
    image[0..4].copy_from_slice(b"RSX\0");
    let path = fixture.write("library/game.stx", &image);
    assert!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .expect("refused")
            .detail()
            .contains("Pasti `RSY\\0` signature")
    );
}

/// Test 17
#[test]
fn a_truncated_pasti_header_is_refused() {
    let fixture = Fixture::new("stx-truncated");
    for length in [0_usize, 4, 15, 16, 20, 31] {
        let mut image = valid_stx();
        image.truncate(length);
        let path = fixture.write("library/game.stx", &image);
        let evidence = fixture.inspect(&path);
        assert_eq!(
            evidence.format, None,
            "a {length}-byte file cannot be valid"
        );
        assert!(evidence.refusal.is_some());
    }
}

/// Test 18
#[test]
fn a_pasti_record_shorter_than_its_own_header_is_refused() {
    let fixture = Fixture::new("stx-short-record");
    let mut image = valid_stx();
    // Track 0 claims an 8-byte record; its own header is 16.
    image[16..20].copy_from_slice(&8_u32.to_le_bytes());
    let path = fixture.write("library/game.stx", &image);
    assert!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .expect("refused")
            .detail()
            .contains("shorter than its own")
    );
}

/// Test 19
#[test]
fn a_pasti_record_reaching_past_the_file_is_refused() {
    let fixture = Fixture::new("stx-past-end");
    let mut image = valid_stx();
    image[16..20].copy_from_slice(&100_000_u32.to_le_bytes());
    let path = fixture.write("library/game.stx", &image);
    assert!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .expect("refused")
            .detail()
            .contains("past the file's")
    );
}

/// Test 20
#[test]
fn a_pasti_fuzzy_mask_that_does_not_fit_its_record_is_refused() {
    let fixture = Fixture::new("stx-fuzzy");
    let mut image = valid_stx();
    // A 64-byte record claiming a 4096-byte fuzzy mask.
    image[20..24].copy_from_slice(&4096_u32.to_le_bytes());
    let path = fixture.write("library/game.stx", &image);
    assert!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .expect("refused")
            .detail()
            .contains("does not fit in its")
    );
}

/// Test 21
#[test]
fn an_excessive_pasti_track_count_is_refused_without_allocating() {
    let fixture = Fixture::new("stx-track-count");
    // A tiny file claiming the maximum possible track count. The count must never
    // size an allocation, so this has to be cheap and refused.
    let mut image = valid_stx();
    image[0x0A] = u8::MAX;
    let path = fixture.write("library/game.stx", &image);
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, None);
    assert!(
        evidence
            .refusal
            .as_ref()
            .expect("refused")
            .detail()
            .contains("record limit")
    );
    assert!(
        evidence.bytes_inspected <= MAX_DISK_FORMAT_BYTES_READ,
        "the budget must hold whatever the header claims"
    );
}

/// Test 22
#[test]
fn a_pasti_header_declaring_no_tracks_is_refused() {
    let fixture = Fixture::new("stx-zero-tracks");
    let mut image = valid_stx();
    image[0x0A] = 0;
    let path = fixture.write("library/game.stx", &image);
    assert!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .expect("refused")
            .detail()
            .contains("no track records")
    );
}

/// Test 23
#[test]
fn pasti_record_lengths_that_try_to_overflow_are_refused() {
    let fixture = Fixture::new("stx-overflow");
    // `u32::MAX` record length: adding it to the offset must be checked, not
    // wrapped into something that looks in-range.
    for hostile in [u32::MAX, u32::MAX - 15, 0xffff_0000] {
        let mut image = valid_stx();
        image[16..20].copy_from_slice(&hostile.to_le_bytes());
        let path = fixture.write("library/game.stx", &image);
        let evidence = fixture.inspect(&path);
        assert_eq!(
            evidence.format, None,
            "record length {hostile} must be refused"
        );
    }
    // And a fuzzy length that would overflow when added to the header size.
    let mut image = valid_stx();
    image[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    let path = fixture.write("library/game.stx", &image);
    assert_eq!(fixture.inspect(&path).format, None);
}

/// Test 24
#[test]
fn an_invalid_pasti_revision_is_refused() {
    let fixture = Fixture::new("stx-revision");
    let mut image = valid_stx();
    image[0x0B] = 7;
    let path = fixture.write("library/game.stx", &image);
    assert!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .expect("refused")
            .detail()
            .contains("revision 7")
    );
}

/// Test 25
#[test]
fn random_data_with_an_stx_extension_is_refused() {
    let fixture = Fixture::new("stx-random");
    let mut image = vec![0_u8; 8192];
    for (index, byte) in image.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(97).wrapping_add(13);
    }
    let path = fixture.write("library/noise.stx", &image);
    assert_eq!(fixture.inspect(&path).format, None);
}

/// Test 26
#[test]
fn a_pasti_track_table_longer_than_the_budget_stops_at_the_bound() {
    let fixture = Fixture::new("stx-long-table");
    // Records long enough that walking them all would run past the inspection
    // window. The walk must stop at the bound and report what it proved, not
    // claim the rest was checked and not exceed the budget.
    let image = stx_image(3, 160, 512);
    let path = fixture.write("library/big.stx", &image);
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, Some(DiskFormat::AtariStPasti));
    let Some(DiskFormatMetadata::Pasti(layout)) = evidence.metadata else {
        panic!("expected layout");
    };
    assert_eq!(layout.declared_track_records, 160);
    assert!(
        layout.validated_track_records < 160,
        "the walk should have stopped at the inspection bound"
    );
    assert!(layout.validated_track_records > 0);
    assert!(evidence.bytes_inspected <= MAX_DISK_FORMAT_BYTES_READ);
}

// --- Dispatch, symlinks, cancellation, purity -----------------------------

/// Test 27
#[test]
fn an_unhandled_extension_is_refused_without_opening_the_file() {
    let fixture = Fixture::new("dispatch");
    for name in ["game.bin", "game.iso", "RESOURCE.GEN", "game.msa"] {
        let path = fixture.write(&format!("library/{name}"), &st_720k());
        let evidence = fixture.inspect(&path);
        assert_eq!(evidence.format, None, "{name} has no adapter");
        assert_eq!(
            evidence.refusal.as_ref().map(DiskFormatRefusal::code),
            Some("no_adapter")
        );
        assert_eq!(evidence.bytes_inspected, 0);
    }
    // `.dsk` now has an adapter, so a non-DSK payload under it is a
    // *malformed* refusal, not a missing adapter - but still no format claim.
    let path = fixture.write("library/not-really.dsk", &st_720k());
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, None);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("malformed")
    );
    // And a file with no extension at all.
    let path = fixture.write("library/plain", &st_720k());
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("no_extension")
    );
}

/// Test 28
#[cfg(unix)]
#[test]
fn a_symlink_inside_the_trusted_roots_is_inspected() {
    let fixture = Fixture::new("symlink-ok");
    let target = fixture.write("downloads/real.st", &st_720k());
    let link = fixture.link("library/game.st", &target);
    let evidence = fixture.inspect(&link);
    assert_eq!(evidence.format, Some(DiskFormat::AtariStRawFloppy));
    assert!(evidence.read_via_symlink);
    assert!(
        evidence
            .evidence
            .iter()
            .any(|item| item.contains("validated symlink target"))
    );
}

/// Test 29
#[cfg(unix)]
#[test]
fn a_symlink_escaping_the_trusted_roots_is_refused_by_the_shared_policy() {
    let fixture = Fixture::new("symlink-escape");
    let target = fixture.write("elsewhere/secret.st", &st_720k());
    let link = fixture.link("library/game.st", &target);
    let evidence = fixture.inspect(&link);
    assert_eq!(evidence.format, None);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("target_outside_trusted_roots"),
        "the refusal must come from safe_read, not from a policy invented here"
    );
}

/// Test 30
#[cfg(unix)]
#[test]
fn a_broken_or_looping_symlink_is_refused() {
    let fixture = Fixture::new("symlink-broken");
    let broken = fixture.link("library/broken.st", &fixture.path("downloads/absent.st"));
    let first = fixture.path("library/loop-a.st");
    let second = fixture.path("library/loop-b.st");
    std::os::unix::fs::symlink(&second, &first).expect("fixture");
    std::os::unix::fs::symlink(&first, &second).expect("fixture");
    for path in [broken, first] {
        let evidence = fixture.inspect(&path);
        assert_eq!(evidence.format, None, "{} must be refused", path.display());
        assert_eq!(
            evidence.refusal.as_ref().map(DiskFormatRefusal::code),
            Some("unresolvable_target")
        );
    }
}

/// Test 31
#[cfg(unix)]
#[test]
fn a_symlink_is_refused_when_no_trusted_root_is_configured() {
    let fixture = Fixture::new("symlink-fail-closed");
    let target = fixture.write("downloads/real.stx", &valid_stx());
    let link = fixture.link("library/game.stx", &target);
    let evidence = inspect_disk_format(
        &link,
        &TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("no_trusted_roots")
    );
}

/// Test 32
#[test]
fn cancellation_is_honoured_between_bounded_steps() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fixture = Fixture::new("cancel");
    let path = fixture.write("library/game.stx", &stx_image(3, 100, 64));
    let cancel = AtomicBool::new(true);
    let evidence = inspect_disk_format(
        &path,
        &fixture.trusted(),
        DiskFormatContext::default(),
        Some(&cancel),
    );
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("cancelled")
    );
    // Not cancelled, the same file validates - so the refusal above really was
    // the flag and not a malformed fixture.
    cancel.store(false, Ordering::Relaxed);
    let evidence = inspect_disk_format(
        &path,
        &fixture.trusted(),
        DiskFormatContext::default(),
        Some(&cancel),
    );
    assert_eq!(evidence.format, Some(DiskFormat::AtariStPasti));
}

/// Test 33
#[test]
fn every_inspection_stays_inside_the_documented_read_budget() {
    let fixture = Fixture::new("budget");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("library/a.st", st_720k()),
        ("library/b.st", st_image(1, 10, 80)),
        ("library/c.st", vec![0_u8; 737_280]),
        ("library/d.stx", valid_stx()),
        ("library/e.stx", stx_image(3, 160, 512)),
        ("library/f.stx", stx_image(3, 168, 64)),
    ];
    for (name, image) in cases {
        let path = fixture.write(name, &image);
        let evidence = fixture.inspect(&path);
        assert!(
            evidence.bytes_inspected <= MAX_DISK_FORMAT_BYTES_READ,
            "{name} read {} bytes, over the {MAX_DISK_FORMAT_BYTES_READ} budget",
            evidence.bytes_inspected
        );
    }
}

/// Test 34
#[test]
fn inspection_is_deterministic_and_changes_nothing_on_disk() {
    let fixture = Fixture::new("read-only");
    fixture.write("library/valid.st", &st_720k());
    fixture.write("library/valid.stx", &valid_stx());
    fixture.write("library/noise.st", &vec![7_u8; 737_280]);
    fixture.write("library/short.stx", &valid_stx()[..8]);
    let before = fixture.snapshot();

    let mut first: Vec<String> = Vec::new();
    for name in [
        "valid.st",
        "valid.stx",
        "noise.st",
        "short.stx",
        "absent.st",
    ] {
        let evidence = fixture.inspect(&fixture.path(&format!("library/{name}")));
        first.push(evidence.summary());
    }
    for _ in 0..5 {
        let mut again: Vec<String> = Vec::new();
        for name in [
            "valid.st",
            "valid.stx",
            "noise.st",
            "short.stx",
            "absent.st",
        ] {
            again.push(
                fixture
                    .inspect(&fixture.path(&format!("library/{name}")))
                    .summary(),
            );
        }
        assert_eq!(first, again, "the same inputs must give the same answer");
    }

    assert_eq!(
        fixture.snapshot(),
        before,
        "inspection must not create, remove, modify or re-timestamp anything"
    );
}

/// Test 35
#[test]
fn no_module_here_can_write_extract_mount_or_reach_a_network() {
    for (name, source) in [
        ("mod.rs", include_str!("mod.rs")),
        ("atari_st.rs", include_str!("atari_st.rs")),
        ("atari_stx.rs", include_str!("atari_stx.rs")),
    ] {
        // Comments are stripped first: this module's documentation legitimately
        // uses these words while explaining that it does none of them.
        let code: String = source
            .split("#[cfg(test)]")
            .next()
            .expect("production half")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "fs::write",
            "fs::create_dir",
            "fs::remove_",
            "fs::rename",
            "fs::set_permissions",
            "File::create",
            "OpenOptions",
            "ureq",
            "reqwest",
            "TcpStream",
            "Command",
            "std::process",
            "ZipArchive",
            "mount",
        ] {
            assert!(
                !code.contains(forbidden),
                "`{forbidden}` must never appear in {name}"
            );
        }
    }
}

// --- Through the platform detector ---------------------------------------

/// Test 36
#[test]
fn the_detector_confirms_an_st_image_in_an_atari_st_folder() {
    let fixture = Fixture::new("detect-st-folder");
    let image = fixture.write("library/atari-st/Game.st", &st_720k());
    let report = detect_platform_report(
        &DetectionRequest::new(&image, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    assert_eq!(report.platform, Some("AtariST"));
    assert_eq!(
        report.confidence,
        DetectionConfidence::Confirmed,
        "structure plus folder is the case the milestone says is Confirmed: {:?}",
        report.evidence
    );
}

/// Test 37
#[test]
fn the_detector_keeps_an_unlabelled_st_image_at_probable() {
    let fixture = Fixture::new("detect-st-alone");
    let image = fixture.write("library/unsorted/Game.st", &st_720k());
    let report = detect_platform_report(
        &DetectionRequest::new(&image, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    assert_eq!(report.platform, Some("AtariST"));
    assert_eq!(
        report.confidence,
        DetectionConfidence::Probable,
        "a FAT12 boot sector is not proof of the platform on its own"
    );
    assert!(
        report.evidence.iter().any(|item| {
            item.source == DetectionSource::Signature && item.detail.contains("raw floppy")
        }),
        "the structural match should still be reported: {:?}",
        report.evidence
    );
}

/// Test 38
#[test]
fn the_detector_confirms_a_pasti_image_with_no_folder_evidence_at_all() {
    let fixture = Fixture::new("detect-stx");
    let image = fixture.write("library/unsorted/Game.stx", &valid_stx());
    let report = detect_platform_report(
        &DetectionRequest::new(&image, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    assert_eq!(report.platform, Some("AtariST"));
    assert_eq!(
        report.confidence,
        DetectionConfidence::Confirmed,
        "Pasti is Atari ST specific, so the container alone settles it"
    );
}

/// Test 39
#[test]
fn a_malformed_st_image_falls_back_to_extension_evidence_only() {
    let fixture = Fixture::new("detect-st-malformed");
    let image = fixture.write("library/unsorted/Game.st", &vec![0_u8; 737_280]);
    let report = detect_platform_report(
        &DetectionRequest::new(&image, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    // `.st` is still a strong extension, so Atari ST remains the best guess -
    // but no structural claim is made for it.
    assert_eq!(report.platform, Some("AtariST"));
    assert_eq!(report.confidence, DetectionConfidence::Probable);
    assert!(
        !report
            .evidence
            .iter()
            .any(|item| item.source == DetectionSource::Signature),
        "a malformed image must claim no signature: {:?}",
        report.evidence
    );
}

/// Test 40
#[test]
fn a_valid_st_image_under_a_contradicting_folder_keeps_the_folders_platform() {
    let fixture = Fixture::new("detect-st-conflict");
    let image = fixture.write("library/megadrive/Game.st", &st_720k());
    let report = detect_platform_report(
        &DetectionRequest::new(&image, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    // The established precedence rule: the folder outranks a contradicting
    // structural match, and the result is Probable with the conflict visible.
    assert_eq!(report.platform, Some("MegaDrive"));
    assert_eq!(report.confidence, DetectionConfidence::Probable);
    assert!(
        report
            .candidates
            .iter()
            .any(|candidate| candidate.platform == "AtariST"),
        "the contradicting structure must stay visible as a candidate"
    );
}

/// Test 41
#[test]
fn a_manual_assignment_still_overrides_a_valid_structure() {
    let fixture = Fixture::new("detect-st-manual");
    let image = fixture.write("library/atari-st/Game.stx", &valid_stx());
    let report = detect_platform_report(
        &DetectionRequest::new(&image, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted())
            .with_manual_platform(Some("Amiga")),
    );
    assert_eq!(report.platform, Some("Amiga"));
    assert!(report.manually_assigned);
}

// --- Regression: nothing else moved ---------------------------------------

/// Test 42: the ScummVM case this project fixed earlier stays fixed - adding a
/// structural adapter must not disturb it.
#[test]
fn scummvm_resource_gen_is_still_scummvm() {
    let fixture = Fixture::new("regression-scummvm");
    fixture.write("library/scummvm/laurabow2/RESOURCE.MAP", b"map");
    let target = fixture.write("library/scummvm/laurabow2/RESOURCE.GEN", b"not a rom");
    let report = detect_platform_report(
        &DetectionRequest::new(&target, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    assert_eq!(report.platform, Some("ScummVM"));
    assert_ne!(report.platform, Some("MegaDrive"));
    assert_ne!(report.platform, Some("AtariST"));
}

/// Test 43: shared extensions stay ambiguous. A `.dsk` is one of the extensions
/// Atari ST shares, so this is exactly where an over-eager adapter would leak.
#[test]
fn generic_shared_extensions_stay_ambiguous() {
    let fixture = Fixture::new("regression-shared");
    for name in ["game.dsk", "game.bin", "game.iso"] {
        // Give each one bytes that would satisfy the ST boot-sector check, so the
        // only thing stopping an Atari ST claim is that no adapter handles the
        // extension.
        let path = fixture.write(&format!("library/unsorted/{name}"), &st_720k());
        let report = detect_platform_report(
            &DetectionRequest::new(&path, &fixture.path("library"))
                .inspecting_content()
                .with_trusted_roots(fixture.trusted()),
        );
        assert_eq!(report.platform, None, "{name} must stay unresolved");
        assert_eq!(
            report.confidence,
            DetectionConfidence::Ambiguous,
            "{name} should list its candidates rather than pick one"
        );
        assert!(
            !report
                .evidence
                .iter()
                .any(|item| item.source == DetectionSource::Signature),
            "{name} must produce no structural claim"
        );
    }
}

/// Test 44: a Mega Drive ROM is still identified by its own header, and an
/// Atari ST adapter has not shadowed it.
#[test]
fn a_mega_drive_rom_is_still_confirmed_from_its_header() {
    let fixture = Fixture::new("regression-megadrive");
    let mut rom = vec![0_u8; 0x200];
    rom[0x100..0x104].copy_from_slice(b"SEGA");
    let path = fixture.write("library/unsorted/mystery.bin", &rom);
    let report = detect_platform_report(
        &DetectionRequest::new(&path, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    assert_eq!(report.platform, Some("MegaDrive"));
    assert_eq!(report.confidence, DetectionConfidence::Confirmed);
}

/// Test 45: this milestone (Atari ST/STX detection) adds no migration of
/// its own. The migration list is still pinned exactly, so an *unrelated*
/// future change that quietly adds one here is still caught; migrations
/// 0007 (`migrations/0007_discovery_details.sql`, Collection Discovery
/// paging), 0008 (`migrations/0008_library_dat_identities.sql`, per-item DAT
/// identity persistence), and 0009 (`migrations/0009_set_audit_verdicts.sql`,
/// set-audit persistence) are already accounted for as legitimately accepted,
/// unrelated additions - see `database.rs` - not something this milestone introduced.
#[test]
fn the_database_schema_and_migrations_are_unchanged() {
    let versions = crate::database::migration_versions_for_tests();
    assert_eq!(
        versions,
        vec![1_i64, 2, 3, 4, 5, 6, 7, 8, 9],
        "migrations must remain exactly 0001 through 0009"
    );
}

// --- CPCEMU DSK / ZX Spectrum +3 ---------------------------------------

const DSK_INFO: usize = 256;

/// One CPCEMU `.dsk`, built exactly from a geometry. When `plus3dos` is set,
/// track 0's first sector carries a valid +3DOS/PCW disk-specification block
/// of the given `disk_type`; when `bootable` is also set the whole first
/// sector is tuned so its byte sum mod 256 is 3.
fn dsk_image(
    extended: bool,
    tracks: u8,
    sides: u8,
    sectors_per_track: u8,
    plus3dos: Option<(u8 /*disk_type*/, bool /*bootable*/)>,
) -> Vec<u8> {
    let sector_bytes = 512usize;
    let track_size = DSK_INFO + usize::from(sectors_per_track) * sector_bytes;
    let entries = usize::from(tracks) * usize::from(sides);

    let mut info = vec![0u8; DSK_INFO];
    if extended {
        info[..0x22].copy_from_slice(b"EXTENDED CPC DSK File\r\nDisk-Info\r\n");
    } else {
        info[..0x22].copy_from_slice(b"MV - CPCEMU Disk-File\r\nDisk-Info\r\n");
    }
    info[0x22..0x30].copy_from_slice(b"archivefs test");
    info[0x30] = tracks;
    info[0x31] = sides;
    if extended {
        let hi = u8::try_from(track_size / 256).expect("track fits in a byte*256");
        for entry in 0..entries {
            info[0x34 + entry] = hi;
        }
    } else {
        info[0x32..0x34].copy_from_slice(&u16::try_from(track_size).unwrap().to_le_bytes());
    }

    let mut image = info;
    for track_index in 0..entries {
        let track = u8::try_from(track_index / usize::from(sides)).unwrap();
        let side = u8::try_from(track_index % usize::from(sides)).unwrap();
        let mut header = vec![0u8; DSK_INFO];
        header[..0x0C].copy_from_slice(b"Track-Info\r\n");
        header[0x10] = track;
        header[0x11] = side;
        header[0x14] = 2; // sector size code -> 512
        header[0x15] = sectors_per_track;
        header[0x16] = 0x4E; // GAP#3
        header[0x17] = 0xE5; // filler
        for sector in 0..usize::from(sectors_per_track) {
            let base = 0x18 + sector * 8;
            header[base] = track;
            header[base + 1] = side;
            header[base + 2] = 0xC1 + u8::try_from(sector).unwrap(); // sector ID
            header[base + 3] = 2; // N
            if extended {
                header[base + 6..base + 8].copy_from_slice(&512u16.to_le_bytes());
            }
        }
        image.extend_from_slice(&header);
        let mut data = vec![0xE5u8; usize::from(sectors_per_track) * sector_bytes];

        if track_index == 0
            && let Some((disk_type, bootable)) = plus3dos
        {
            // +3DOS / PCW disk specification in the first 16 bytes of the
            // first sector (which starts at image offset 0x200).
            data[0] = disk_type;
            data[1] = 0; // sidedness: single
            data[2] = tracks;
            data[3] = sectors_per_track;
            data[4] = 2; // log2(512) - 7
            data[5] = 1; // reserved tracks
            data[6] = 3; // block shift
            data[7] = 2; // directory blocks
            data[8] = 0x2A; // gap r/w
            data[9] = 0x52; // gap format
            for byte in data.iter_mut().take(15).skip(10) {
                *byte = 0; // reserved
            }
            data[15] = 0; // spec-block checksum slot (not validated here)
            if bootable {
                let sum: u32 = data[..512].iter().map(|b| u32::from(*b)).sum();
                let want = 3u32;
                let have = sum % 256;
                // Nudge a scratch byte well outside the spec block.
                data[64] = data[64].wrapping_add(((want + 256 - have) % 256) as u8);
            }
        }
        image.extend_from_slice(&data);
    }
    image
}

/// A standard 40-track single-sided +3 "CF2" disk with a bootable +3DOS spec.
fn plus3_cf2_disk() -> Vec<u8> {
    dsk_image(false, 40, 1, 9, Some((0, true)))
}

/// A plain double-sided 80-track CPC data disk: valid container, no +3DOS.
fn cpc_data_disk() -> Vec<u8> {
    dsk_image(false, 80, 2, 9, None)
}

#[test]
fn dsk_standard_container_is_recognised_but_not_a_platform() {
    let fixture = Fixture::new("dsk-standard");
    let path = fixture.write("library/game.dsk", &cpc_data_disk());
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, Some(DiskFormat::CpcEmuDsk));
    assert!(!evidence.conclusive, "a bare .dsk never settles a platform");
    assert_eq!(evidence.confidence, DetectionConfidence::Probable);
    let Some(DiskFormatMetadata::Dsk(layout)) = evidence.metadata else {
        panic!("expected dsk layout");
    };
    assert!(!layout.extended);
    assert!(!layout.plus3dos_disk_spec);
    assert_eq!(layout.declared_tracks, 80);
    assert!(evidence.bytes_inspected <= MAX_DISK_FORMAT_BYTES_READ);
}

// --- ZX Spectrum SCL ("SINCLAIR") archive -----------------------------

/// Build an `.scl` from a list of per-file sector counts.
fn scl_archive(files: &[u8], with_checksum: bool) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"SINCLAIR");
    v.push(u8::try_from(files.len()).expect("<=128 files"));
    for &sectors in files {
        let mut entry = [0u8; 14];
        entry[..8].copy_from_slice(b"GAME    ");
        entry[8] = b'C';
        entry[13] = sectors; // length in 256-byte sectors
        v.extend_from_slice(&entry);
    }
    let payload: usize = files.iter().map(|&s| usize::from(s) * 256).sum();
    v.extend(std::iter::repeat_n(0u8, payload));
    if with_checksum {
        let sum = v
            .iter()
            .fold(0u32, |acc, &b| acc.wrapping_add(u32::from(b)));
        v.extend_from_slice(&sum.to_le_bytes());
    }
    v
}

#[test]
fn scl_empty_archive_is_valid_zx_spectrum_evidence() {
    let fixture = Fixture::new("scl-empty");
    let path = fixture.write("library/empty.scl", &scl_archive(&[], false));
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, Some(DiskFormat::SpectrumSclArchive));
    assert_eq!(evidence.platform, Some("ZX Spectrum"));
    assert!(evidence.conclusive);
    assert_eq!(evidence.confidence, DetectionConfidence::Confirmed);
    let Some(DiskFormatMetadata::Scl(layout)) = evidence.metadata else {
        panic!("expected scl layout");
    };
    assert_eq!(layout.file_count, 0);
    assert_eq!(layout.declared_sectors, 0);
    assert!(evidence.bytes_inspected <= MAX_DISK_FORMAT_BYTES_READ);
}

#[test]
fn dsk_extended_container_is_recognised() {
    let fixture = Fixture::new("dsk-extended");
    let image = dsk_image(true, 42, 1, 9, None);
    let path = fixture.write("library/game.dsk", &image);
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, Some(DiskFormat::CpcEmuDsk));
    let Some(DiskFormatMetadata::Dsk(layout)) = evidence.metadata else {
        panic!("expected dsk layout");
    };
    assert!(layout.extended);
    assert!(!layout.plus3dos_disk_spec);
}

#[test]
fn dsk_with_plus3dos_disk_spec_is_zx_spectrum() {
    let fixture = Fixture::new("dsk-plus3");
    let path = fixture.write("library/Game (1987).dsk", &plus3_cf2_disk());
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, Some(DiskFormat::SpectrumPlus3Disk));
    assert_eq!(evidence.platform, Some("ZX Spectrum"));
    assert!(evidence.conclusive);
    assert_eq!(evidence.confidence, DetectionConfidence::Confirmed);
    let Some(DiskFormatMetadata::Dsk(layout)) = evidence.metadata else {
        panic!("expected dsk layout");
    };
    assert!(layout.plus3dos_disk_spec);
    assert_eq!(layout.plus3dos_disk_type, Some(0));
    assert!(layout.plus3_bootable);
}

#[test]
fn cpc_disk_type_in_the_spec_block_is_never_spectrum() {
    let fixture = Fixture::new("dsk-cpc-spec");
    // disk type 2 = CPC data - the spec block validates structurally but must
    // not be read as ZX Spectrum.
    let path = fixture.write(
        "library/game.dsk",
        &dsk_image(false, 40, 1, 9, Some((2, false))),
    );
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, Some(DiskFormat::CpcEmuDsk));
    assert_ne!(evidence.platform, Some("ZX Spectrum"));
    let Some(DiskFormatMetadata::Dsk(layout)) = evidence.metadata else {
        panic!("expected dsk layout");
    };
    assert_eq!(layout.plus3dos_disk_type, Some(2));
}

#[test]
fn a_plus3_disk_in_an_amstrad_cpc_folder_is_reported_ambiguous_not_forced() {
    let fixture = Fixture::new("dsk-plus3-cpc-folder");
    let path = fixture.write("library/game.dsk", &plus3_cf2_disk());
    let evidence = fixture.inspect_in_folder(&path, "Amstrad CPC");
    assert_eq!(evidence.format, Some(DiskFormat::SpectrumPlus3Disk));
    assert_eq!(evidence.confidence, DetectionConfidence::Ambiguous);
    assert!(!evidence.conclusive);
}

#[test]
fn dsk_truncated_into_its_track_data_is_a_geometry_mismatch() {
    let fixture = Fixture::new("dsk-truncated");
    let mut image = cpc_data_disk();
    image.truncate(image.len() - 4096);
    let path = fixture.write("library/game.dsk", &image);
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, None);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("geometry_mismatch")
    );
}

#[test]
fn scl_one_and_many_file_archives_validate_with_and_without_checksum() {
    let fixture = Fixture::new("scl-files");
    let one = fixture.write("library/one.scl", &scl_archive(&[4], false));
    let ev = fixture.inspect(&one);
    assert_eq!(ev.format, Some(DiskFormat::SpectrumSclArchive));
    let Some(DiskFormatMetadata::Scl(layout)) = ev.metadata else {
        panic!()
    };
    assert_eq!(layout.file_count, 1);
    assert_eq!(layout.declared_sectors, 4);
    assert!(!layout.has_trailing_checksum);

    let many = fixture.write("library/many.scl", &scl_archive(&[1, 2, 3, 9], true));
    let ev = fixture.inspect(&many);
    let Some(DiskFormatMetadata::Scl(layout)) = ev.metadata else {
        panic!()
    };
    assert_eq!(layout.file_count, 4);
    assert_eq!(layout.declared_sectors, 15);
    assert!(layout.has_trailing_checksum);
}

#[test]
fn scl_wrong_magic_is_refused() {
    let fixture = Fixture::new("scl-magic");
    let mut bytes = scl_archive(&[1], false);
    bytes[3] = b'X';
    let path = fixture.write("library/x.scl", &bytes);
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, None);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("malformed")
    );
}

#[test]
fn scl_truncated_header_is_refused() {
    let fixture = Fixture::new("scl-short-header");
    let path = fixture.write("library/short.scl", b"SINCL");
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("too_small")
    );
}

#[test]
fn scl_directory_table_beyond_eof_is_refused() {
    let fixture = Fixture::new("scl-table-oob");
    // Claims 50 files but carries no directory at all.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"SINCLAIR");
    bytes.push(50);
    let path = fixture.write("library/liar.scl", &bytes);
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, None);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("malformed")
    );
}

#[test]
fn scl_payload_shorter_or_longer_than_declared_is_refused() {
    let fixture = Fixture::new("scl-payload");
    let mut short = scl_archive(&[4], false);
    short.truncate(short.len() - 300); // lose part of the declared payload
    let path = fixture.write("library/short.scl", &short);
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("geometry_mismatch")
    );

    let mut long = scl_archive(&[2], false);
    long.extend_from_slice(&[0u8; 64]); // trailing bytes the entries do not account for
    let path = fixture.write("library/long.scl", &long);
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("geometry_mismatch")
    );
}

#[test]
fn dsk_random_bytes_and_zip_are_refused() {
    let fixture = Fixture::new("dsk-random");
    let path = fixture.write("library/game.dsk", &vec![0x5Au8; 8192]);
    assert_eq!(fixture.inspect(&path).format, None);

    let mut zip = vec![0u8; 8192];
    zip[..4].copy_from_slice(b"PK\x03\x04");
    let path = fixture.write("library/z.dsk", &zip);
    assert_eq!(fixture.inspect(&path).format, None);
}

#[test]
fn scl_absurd_file_count_is_refused() {
    let fixture = Fixture::new("scl-absurd");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"SINCLAIR");
    bytes.push(200); // over the 128-entry TR-DOS maximum
    bytes.extend(std::iter::repeat_n(0u8, 200 * 14));
    let path = fixture.write("library/big.scl", &bytes);
    assert_eq!(fixture.inspect(&path).format, None);
}

#[test]
fn dsk_absurd_geometry_is_refused_without_huge_allocation() {
    let fixture = Fixture::new("dsk-absurd");
    let mut info = vec![0u8; DSK_INFO];
    info[..8].copy_from_slice(b"MV - CPC");
    info[0x30] = 255; // 255 tracks
    info[0x31] = 2;
    info[0x32..0x34].copy_from_slice(&0xFFFFu16.to_le_bytes()); // 64KB tracks
    // File is tiny; the declared geometry cannot possibly fit.
    let path = fixture.write("library/evil.dsk", &info);
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, None);
    assert!(evidence.refusal.is_some());
}

#[test]
fn scl_random_bytes_named_scl_are_refused() {
    let fixture = Fixture::new("scl-random");
    let path = fixture.write("library/game.scl", &vec![0x5Au8; 4096]);
    assert_eq!(fixture.inspect(&path).format, None);
}

// --- ZX Spectrum TR-DOS disk image ----------------------------------

fn trd_geometry(disk_type: u8) -> (u16, u8) {
    match disk_type {
        0x16 => (80, 2),
        0x17 => (40, 2),
        0x18 => (80, 1),
        0x19 => (40, 1),
        _ => panic!("unsupported test disk type"),
    }
}

/// Build a structurally valid TR-DOS image, then let `tweak` corrupt it.
fn trd_disk(
    disk_type: u8,
    file_count: u8,
    label: &[u8; 8],
    tweak: impl FnOnce(&mut [u8]),
) -> Vec<u8> {
    let (tps, sides) = trd_geometry(disk_type);
    let total_tracks = u64::from(tps) * u64::from(sides);
    let total_sectors = total_tracks * 16;
    let mut v = vec![0u8; usize::try_from(total_sectors * 256).unwrap()];

    if file_count > 0 {
        v[0..8].copy_from_slice(b"BOOT    ");
        v[8] = b'B';
        v[13] = 1;
    }

    let d = 0x800usize;
    v[d] = 0; // catalogue end marker
    let first_free_track = 1u16;
    let first_free_sector = 0u8;
    let free_sectors = u16::try_from(total_sectors - 16).unwrap(); // whole disk bar track 0
    v[d + 0xE1] = first_free_sector;
    v[d + 0xE2] = u8::try_from(first_free_track).unwrap();
    v[d + 0xE3] = disk_type;
    v[d + 0xE4] = file_count;
    v[d + 0xE5..d + 0xE7].copy_from_slice(&free_sectors.to_le_bytes());
    v[d + 0xE7] = 0x10;
    v[d + 0xF5..d + 0xF5 + 8].copy_from_slice(label);

    tweak(&mut v);
    v
}

#[test]
fn trd_standard_double_sided_image_is_valid_zx_spectrum_evidence() {
    let fixture = Fixture::new("trd-80ds");
    let path = fixture.write(
        "library/Game (1989).trd",
        &trd_disk(0x16, 5, b"MYDISK  ", |_| {}),
    );
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, Some(DiskFormat::SpectrumTrDosDisk));
    assert_eq!(evidence.platform, Some("ZX Spectrum"));
    assert!(evidence.conclusive);
    assert_eq!(evidence.confidence, DetectionConfidence::Confirmed);
    let Some(DiskFormatMetadata::TrDos(descriptor)) = evidence.metadata else {
        panic!("expected trdos descriptor");
    };
    assert_eq!(descriptor.disk_type, 0x16);
    assert_eq!(descriptor.tracks_per_side, 80);
    assert_eq!(descriptor.sides, 2);
    assert_eq!(descriptor.file_count, 5);
    assert_eq!(descriptor.label, Some(*b"MYDISK  "));
    assert!(evidence.bytes_inspected <= MAX_DISK_FORMAT_BYTES_READ);
}

#[test]
fn dsk_detection_reaches_platform_detection_for_a_plus3_disk_only() {
    let fixture = Fixture::new("dsk-platform");
    // A +3 disk resolves to ZX Spectrum through the shared structural layer.
    let plus3 = fixture.write("library/unsorted/game.dsk", &plus3_cf2_disk());
    let report = detect_platform_report(
        &DetectionRequest::new(&plus3, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    assert_eq!(report.platform, Some("ZX Spectrum"));
    assert_eq!(report.confidence, DetectionConfidence::Confirmed);

    // A generic CPC data disk does not force any platform.
    let cpc = fixture.write("library/unsorted/data.dsk", &cpc_data_disk());
    let report = detect_platform_report(
        &DetectionRequest::new(&cpc, &fixture.path("library"))
            .inspecting_content()
            .with_trusted_roots(fixture.trusted()),
    );
    assert_ne!(report.platform, Some("ZX Spectrum"));
}

#[test]
fn trd_all_four_documented_geometries_validate() {
    let fixture = Fixture::new("trd-geometries");
    for (disk_type, tps, sides) in [(0x16, 80, 2), (0x17, 40, 2), (0x18, 80, 1), (0x19, 40, 1)] {
        let path = fixture.write(
            &format!("library/type{disk_type:02x}.trd"),
            &trd_disk(disk_type, 0, b"        ", |_| {}),
        );
        let evidence = fixture.inspect(&path);
        assert_eq!(
            evidence.format,
            Some(DiskFormat::SpectrumTrDosDisk),
            "disk type 0x{disk_type:02X}"
        );
        let Some(DiskFormatMetadata::TrDos(descriptor)) = evidence.metadata else {
            panic!()
        };
        assert_eq!(descriptor.tracks_per_side, tps);
        assert_eq!(descriptor.sides, sides);
        assert_eq!(descriptor.label, None); // all spaces -> no label
    }
}

#[test]
fn trd_truncated_to_used_sectors_is_still_accepted() {
    // Archives commonly trim trailing free sectors; the descriptor is intact.
    let fixture = Fixture::new("trd-trimmed");
    let mut image = trd_disk(0x19, 3, b"TRIMMED ", |_| {});
    image.truncate(image.len() - 40 * 256); // drop 40 trailing sectors
    let path = fixture.write("library/trimmed.trd", &image);
    assert_eq!(
        fixture.inspect(&path).format,
        Some(DiskFormat::SpectrumTrDosDisk)
    );
}

#[test]
fn trd_missing_or_wrong_id_byte_is_refused() {
    let fixture = Fixture::new("trd-id");
    let path = fixture.write(
        "library/noid.trd",
        &trd_disk(0x16, 1, b"X       ", |v| v[0x800 + 0xE7] = 0x00),
    );
    let evidence = fixture.inspect(&path);
    assert_eq!(evidence.format, None);
    assert_eq!(
        evidence.refusal.as_ref().map(DiskFormatRefusal::code),
        Some("malformed")
    );
}

#[test]
fn trd_undocumented_disk_type_is_refused() {
    let fixture = Fixture::new("trd-type");
    let path = fixture.write(
        "library/badtype.trd",
        &trd_disk(0x16, 1, b"X       ", |v| v[0x800 + 0xE3] = 0x99),
    );
    assert_eq!(fixture.inspect(&path).format, None);
}

#[test]
fn trd_impossible_file_count_or_free_cursor_is_refused() {
    let fixture = Fixture::new("trd-impossible");
    let count = fixture.write(
        "library/count.trd",
        &trd_disk(0x16, 1, b"X       ", |v| v[0x800 + 0xE4] = 200),
    );
    assert_eq!(fixture.inspect(&count).format, None);

    let sector = fixture.write(
        "library/sector.trd",
        &trd_disk(0x16, 1, b"X       ", |v| v[0x800 + 0xE1] = 40),
    );
    assert_eq!(fixture.inspect(&sector).format, None);

    let track = fixture.write(
        "library/track.trd",
        &trd_disk(0x16, 1, b"X       ", |v| v[0x800 + 0xE2] = 250),
    );
    assert_eq!(fixture.inspect(&track).format, None);
}

#[test]
fn trd_inconsistent_free_space_is_refused() {
    let fixture = Fixture::new("trd-freespace");
    // Free-sector count that does not reconcile with the free cursor.
    let path = fixture.write(
        "library/free.trd",
        &trd_disk(0x16, 1, b"X       ", |v| {
            v[0x800 + 0xE5] = 0x00;
            v[0x800 + 0xE6] = 0x00; // claims zero free but the cursor says otherwise
        }),
    );
    assert_eq!(fixture.inspect(&path).format, None);
}

#[test]
fn trd_geometry_larger_than_the_disk_type_allows_is_refused() {
    let fixture = Fixture::new("trd-toobig");
    let mut image = trd_disk(0x19, 0, b"        ", |_| {}); // 40 SS -> 163840
    image.extend(std::iter::repeat_n(0u8, 4096)); // one track too many
    let path = fixture.write("library/toobig.trd", &image);
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("geometry_mismatch")
    );
}

#[test]
fn trd_truncated_below_the_descriptor_and_unaligned_lengths_are_refused() {
    let fixture = Fixture::new("trd-short");
    let mut image = trd_disk(0x19, 0, b"        ", |_| {});
    let full = image.clone();

    image.truncate(0x880); // descriptor sector no longer present
    let path = fixture.write("library/tiny.trd", &image);
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("too_small")
    );

    let mut unaligned = full;
    unaligned.truncate(unaligned.len() - 100); // not a whole number of sectors
    let path = fixture.write("library/unaligned.trd", &unaligned);
    assert_eq!(
        fixture
            .inspect(&path)
            .refusal
            .as_ref()
            .map(DiskFormatRefusal::code),
        Some("not_sector_aligned")
    );
}

#[test]
fn trd_random_correctly_sized_image_is_not_spectrum_evidence() {
    let fixture = Fixture::new("trd-random");
    // Exactly the size of an 80-track DS TR-DOS disk, but random content.
    let bytes: Vec<u8> = (0..655360).map(|i| (i * 131 + 7) as u8).collect();
    let path = fixture.write("library/game.trd", &bytes);
    assert_eq!(fixture.inspect(&path).format, None);
}

#[test]
fn trd_recognition_does_not_swallow_an_mgt_sam_coupe_image() {
    let fixture = Fixture::new("trd-mgt");
    // A common MGT/SAM Coupe size (80 x 2 x 10 x 512), 256-aligned, but with
    // no TR-DOS descriptor - must not be read as ZX Spectrum TR-DOS media.
    let bytes = vec![0xE5u8; 819200];
    let path = fixture.write("library/sam.trd", &bytes);
    assert_eq!(fixture.inspect(&path).format, None);
}

#[test]
fn trd_and_scl_in_an_amstrad_cpc_folder_report_ambiguous_not_forced() {
    let fixture = Fixture::new("trdos-cpc-folder");
    let trd = fixture.write("library/game.trd", &trd_disk(0x16, 1, b"X       ", |_| {}));
    let ev = fixture.inspect_in_folder(&trd, "Amstrad CPC");
    assert_eq!(ev.format, Some(DiskFormat::SpectrumTrDosDisk));
    assert_eq!(ev.confidence, DetectionConfidence::Ambiguous);
    assert!(!ev.conclusive);

    let scl = fixture.write("library/game.scl", &scl_archive(&[2], false));
    let ev = fixture.inspect_in_folder(&scl, "Amstrad CPC");
    assert_eq!(ev.confidence, DetectionConfidence::Ambiguous);
}

#[test]
fn trdos_media_reaches_platform_detection_as_confirmed_zx_spectrum() {
    let fixture = Fixture::new("trdos-platform");
    for (name, bytes) in [
        (
            "library/unsorted/a.trd",
            trd_disk(0x16, 2, b"DISK    ", |_| {}),
        ),
        ("library/unsorted/b.scl", scl_archive(&[1, 1], false)),
    ] {
        let path = fixture.write(name, &bytes);
        let report = detect_platform_report(
            &DetectionRequest::new(&path, &fixture.path("library"))
                .inspecting_content()
                .with_trusted_roots(fixture.trusted()),
        );
        assert_eq!(report.platform, Some("ZX Spectrum"), "{name}");
        assert_eq!(report.confidence, DetectionConfidence::Confirmed, "{name}");
    }
}

#[test]
fn hdi_and_nhd_validate_geometry_without_proving_pc98() {
    let fixture = Fixture::new("hdi-nhd-positive");
    let hdi = fixture.write("library/disk.hdi", &hdi_image(0x20, 512, 1, 1, 1));
    let nhd = fixture.write("library/disk.nhd", &nhd_image(0x200, 512, 1, 1, 1));

    let hdi_evidence = fixture.inspect(&hdi);
    assert_eq!(hdi_evidence.format, Some(DiskFormat::HdiContainer));
    assert_eq!(hdi_evidence.platform, Some("PC-98"));
    assert_eq!(hdi_evidence.confidence, DetectionConfidence::Probable);
    assert!(!hdi_evidence.conclusive);
    assert!(
        hdi_evidence
            .evidence
            .iter()
            .any(|item| item.contains("0x00000000"))
    );
    assert!(matches!(
        hdi_evidence.metadata,
        Some(DiskFormatMetadata::Hdi(_))
    ));

    let nhd_evidence = fixture.inspect(&nhd);
    assert_eq!(nhd_evidence.format, Some(DiskFormat::NhdContainer));
    assert_eq!(nhd_evidence.confidence, DetectionConfidence::Probable);
    assert!(!nhd_evidence.conclusive);
    assert!(
        nhd_evidence
            .evidence
            .iter()
            .any(|item| item.contains("TEST NHD"))
    );
    assert!(matches!(
        nhd_evidence.metadata,
        Some(DiskFormatMetadata::Nhd(_))
    ));
}

#[test]
fn hdi_and_nhd_folder_context_confirms_existing_pc98_equivalence() {
    let fixture = Fixture::new("hdi-nhd-context");
    for folder in ["PC-98", "NEC PC-9801"] {
        let hdi = fixture.write(
            &format!("library/{folder}/disk.hdi"),
            &hdi_image(0x20, 512, 1, 1, 1),
        );
        let nhd = fixture.write(
            &format!("library/{folder}/disk.nhd"),
            &nhd_image(0x200, 512, 1, 1, 1),
        );
        for path in [hdi, nhd] {
            let evidence = fixture.inspect_in_folder(&path, folder);
            assert_eq!(evidence.confidence, DetectionConfidence::Confirmed);
            assert!(evidence.conclusive);
            assert_eq!(evidence.platform, Some("PC-98"));
        }
    }
}

#[test]
fn malformed_hdi_and_nhd_structures_fail_closed() {
    let fixture = Fixture::new("hdi-nhd-negative");
    let cases = [
        ("truncated.hdi", vec![0; 12]),
        ("bad-signature.nhd", vec![0; 512]),
        ("zero-geometry.hdi", hdi_image(0x20, 512, 0, 1, 1)),
        ("bad-sector-size.hdi", hdi_image(0x20, 123, 1, 1, 1)),
        ("random.nhd", vec![0x5a; 1024]),
    ];
    for (name, bytes) in cases {
        let path = fixture.write(&format!("library/{name}"), &bytes);
        assert!(
            fixture.inspect(&path).format.is_none(),
            "{name} must be refused"
        );
    }

    let mut bad_offset = hdi_image(0x20, 512, 1, 1, 1);
    bad_offset[8..12].copy_from_slice(&0x1000_u32.to_le_bytes());
    let path = fixture.write("library/bad-offset.hdi", &bad_offset);
    assert!(fixture.inspect(&path).format.is_none());

    let mut bad_payload = hdi_image(0x20, 512, 1, 1, 1);
    bad_payload[12..16].copy_from_slice(&0_u32.to_le_bytes());
    let path = fixture.write("library/bad-payload.hdi", &bad_payload);
    assert!(fixture.inspect(&path).format.is_none());

    let mut overflow = nhd_image(0x200, 512, 1, 1, 1);
    overflow[0x114..0x118].copy_from_slice(&u32::MAX.to_le_bytes());
    let path = fixture.write("library/overflow.nhd", &overflow);
    assert!(fixture.inspect(&path).format.is_none());
}
