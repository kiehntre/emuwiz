use super::*;
use crate::dreamcast_dcp_apply::{
    inspect_extracted_dreamcast_ip_bin, review_extracted_dreamcast_ip_bin,
};
use crate::patch_output_recovery::tree;

// Entirely synthetic metadata and opaque payload. No Sega bootstrap/game code.
fn fixture_bytes() -> Vec<u8> {
    let mut bytes = vec![0xA5; IP_BIN_BYTES];
    bytes[..0x100].fill(b' ');
    bytes[..16].copy_from_slice(b"SEGA SEGAKATANA ");
    bytes[0x10..0x20].copy_from_slice(b"SEGA ENTERPRISES");
    bytes[0x20..0x30].copy_from_slice(b"0000 GD-ROM1/1  ");
    bytes[0x30..0x38].copy_from_slice(b"JUE     ");
    bytes[0x38..0x40].copy_from_slice(b"0000010 ");
    bytes[0x40..0x4A].copy_from_slice(b"T-1234M   ");
    bytes[0x4A..0x50].copy_from_slice(b"V1.000");
    bytes[0x50..0x58].copy_from_slice(b"20000229");
    bytes[0x60..0x6C].copy_from_slice(b"1ST_READ.BIN");
    bytes[0x70..0x74].copy_from_slice(b"TEST");
    bytes[0x80..0x8B].copy_from_slice(b"SYNTHETIC  ");
    for region in [
        DreamcastRegion::Japan,
        DreamcastRegion::UsaCanada,
        DreamcastRegion::Europe,
    ] {
        bytes[region.protection_range()].copy_from_slice(region.protection_text());
    }
    let crc = format!("{:04X}", product_crc16(&bytes[0x40..0x50]));
    bytes[0x20..0x24].copy_from_slice(crc.as_bytes());
    bytes
}
fn loose(bytes: &[u8]) -> (tempfile::TempDir, IpBinFileInspection, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("no-extension");
    fs::write(&source, bytes).unwrap();
    let inspected = inspect_ip_bin_file(&source).unwrap();
    let destination = temp.path().join("edited.bin");
    (temp, inspected, destination)
}
fn text(field: IpBinTextField, value: &str) -> IpBinEdit {
    IpBinEdit::Text {
        field,
        value: value.into(),
    }
}
fn tree_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("bootsector")).unwrap();
    fs::write(source.join("bootsector/IP.BIN"), fixture_bytes()).unwrap();
    fs::write(
        source.join("1ST_READ.BIN"),
        b"synthetic executable bytes, never launched",
    )
    .unwrap();
    fs::write(source.join("keep"), b"untouched").unwrap();
    let destination = temp.path().join("published");
    (temp, source, destination)
}

#[test]
fn valid_minimal_content_detection_and_typed_metadata() {
    let bytes = fixture_bytes();
    let before = bytes.clone();
    let inspected = inspect_ip_bin(&bytes);
    assert_eq!(
        inspected.status,
        IpBinStatus::Valid,
        "{:?}",
        inspected.issues
    );
    let ip = inspected.ip_bin.unwrap();
    assert_eq!(ip.metadata.hardware_id.value, "SEGA SEGAKATANA");
    assert_eq!(ip.metadata.maker_id.value, "SEGA ENTERPRISES");
    assert_eq!(ip.metadata.product_number.value, "T-1234M");
    assert_eq!(ip.metadata.software_title.value, "SYNTHETIC");
    assert_eq!(ip.device.unwrap().disc_count, 1);
    assert_eq!(ip.product_crc.status, IpBinProductCrcStatus::Matched);
    assert_eq!(bytes, before);
    let (_temp, source, _) = loose(&bytes);
    assert_eq!(source.inspection.status, IpBinStatus::Valid);
}
#[test]
fn extension_and_path_cannot_make_garbage_valid() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("IP.BIN");
    fs::write(&path, vec![b' '; IP_BIN_BYTES]).unwrap();
    assert_eq!(
        inspect_ip_bin_file(&path).unwrap().inspection.status,
        IpBinStatus::Malformed
    );
}
#[test]
fn truncated_metadata_and_bootstrap_are_explicit() {
    let bytes = fixture_bytes();
    for length in [0, 15, 255, 256, IP_BIN_BYTES - 1] {
        assert_eq!(
            inspect_ip_bin(&bytes[..length]).status,
            IpBinStatus::Truncated
        );
        let (_temp, source, _) = loose(&bytes[..length]);
        assert!(preview_ip_bin_edits(&source, &[]).is_err());
    }
}
#[test]
fn oversized_input_is_bounded_and_has_no_whole_file_hash() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("disc.iso");
    let mut file = File::create(&path).unwrap();
    file.write_all(&fixture_bytes()).unwrap();
    file.set_len(4 * 1024 * 1024 * 1024).unwrap();
    let inspected = inspect_ip_bin_file(&path).unwrap();
    assert_eq!(inspected.inspection.status, IpBinStatus::UnsupportedVariant);
    assert!(inspected.source.is_none());
    assert_eq!(
        inspected.inspection.ip_bin.unwrap().raw_bytes().len(),
        IP_BIN_BYTES
    );
}
#[test]
fn malformed_hardware_and_maker_are_refused() {
    for offset in [0, 0x10] {
        let mut bytes = fixture_bytes();
        bytes[offset] = b'X';
        let (_temp, source, _) = loose(&bytes);
        assert_eq!(source.inspection.status, IpBinStatus::Malformed);
        assert!(preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, "EDIT")]).is_err());
    }
}
#[test]
fn unknown_hardware_and_media_variants_are_readable_but_not_editable() {
    for hardware in [true, false] {
        let mut bytes = fixture_bytes();
        if hardware {
            bytes[..16].copy_from_slice(b"SEGA SEGAMARIO  ");
        } else {
            bytes[0x25..0x2B].copy_from_slice(b"ODDROM");
        }
        let (_temp, source, _) = loose(&bytes);
        assert_eq!(source.inspection.status, IpBinStatus::UnsupportedVariant);
        assert!(preview_ip_bin_edits(&source, &[]).is_err());
    }
}
#[test]
fn invalid_region_symbols_are_malformed_and_preserved() {
    for symbol in [0, b'?', 0xFF] {
        let mut bytes = fixture_bytes();
        bytes[0x34] = symbol;
        let inspected = inspect_ip_bin(&bytes);
        assert_eq!(inspected.status, IpBinStatus::Malformed);
        let ip = inspected.ip_bin.unwrap();
        assert_eq!(ip.regions[4], DreamcastRegionSymbol::Unknown(symbol));
        assert_eq!(ip.raw_bytes(), bytes);
    }
}
#[test]
fn known_and_unknown_region_symbols_keep_positions() {
    let mut bytes = fixture_bytes();
    bytes[0x33] = b'K';
    let inspected = inspect_ip_bin(&bytes);
    assert_eq!(inspected.status, IpBinStatus::SuspiciousButParseable);
    let ip = inspected.ip_bin.unwrap();
    assert_eq!(
        ip.regions[0],
        DreamcastRegionSymbol::Known(DreamcastRegion::Japan)
    );
    assert_eq!(ip.regions[3], DreamcastRegionSymbol::Unknown(b'K'));
    bytes[0x30..0x33].copy_from_slice(b"UEJ");
    let ip = inspect_ip_bin(&bytes).ip_bin.unwrap();
    assert!(
        ip.regions[..3]
            .iter()
            .all(|s| matches!(s, DreamcastRegionSymbol::Unknown(_)))
    );
}
#[test]
fn peripheral_known_reserved_and_high_bits_are_distinct() {
    let mut bytes = fixture_bytes();
    bytes[0x38..0x40].copy_from_slice(b"0000012 ");
    let inspected = inspect_ip_bin(&bytes);
    assert_eq!(inspected.status, IpBinStatus::SuspiciousButParseable);
    let ip = inspected.ip_bin.unwrap();
    assert_eq!(ip.peripherals.declared, vec![DreamcastPeripheral::Vga]);
    assert_eq!(ip.peripherals.unknown_bits, 2);
    assert_eq!(
        ip.metadata.vga_compatibility,
        DreamcastVgaCompatibility::Declared
    );
    bytes[0x38..0x40].copy_from_slice(b"F0000010");
    let ip = inspect_ip_bin(&bytes).ip_bin.unwrap();
    assert_eq!(ip.peripherals.unknown_bits, 0xF0000000);
    assert_eq!(ip.peripherals.declared, vec![DreamcastPeripheral::Vga]);
    assert!(!ip.peripherals.documented_encoding);
}
#[test]
fn every_documented_peripheral_bit_has_one_declaration() {
    let mut mask = 0;
    for capability in PERIPHERAL_CAPABILITIES {
        let bit = 1u32 << capability as u8;
        let mut bytes = fixture_bytes();
        bytes[0x38..0x40].copy_from_slice(format!("{bit:07X} ").as_bytes());
        let ip = inspect_ip_bin(&bytes).ip_bin.unwrap();
        assert_eq!(ip.peripherals.declared, vec![capability]);
        assert_eq!(ip.peripherals.unknown_bits, 0);
        mask |= bit;
    }
    assert_eq!(mask, KNOWN_PERIPHERAL_MASK);
}
#[test]
fn bad_peripherals_and_impossible_disc_information_are_malformed() {
    for (range, replacement) in [
        (0x38..0x40, &b"ZZZZZZZ "[..]),
        (0x2B..0x2E, &b"2/1"[..]),
        (0x20..0x24, &b"ZZZZ"[..]),
    ] {
        let mut bytes = fixture_bytes();
        bytes[range].copy_from_slice(replacement);
        assert_eq!(inspect_ip_bin(&bytes).status, IpBinStatus::Malformed);
    }
}
#[test]
fn boot_filename_padding_and_unsafe_names() {
    let ip = inspect_ip_bin(&fixture_bytes()).ip_bin.unwrap();
    assert_eq!(ip.metadata.boot_filename.value, "1ST_READ.BIN");
    assert_eq!(ip.metadata.boot_filename.raw_bytes, b"1ST_READ.BIN    ");
    for value in [
        "../BAD.BIN",
        "dir/BOOT.BIN",
        "C:BOOT.BIN",
        ".",
        "",
        "BAD\0NAME.BIN",
    ] {
        let (_temp, source, _) = loose(&fixture_bytes());
        assert!(
            preview_ip_bin_edits(&source, &[text(IpBinTextField::BootFilename, value)]).is_err(),
            "{value:?}"
        );
    }
}
#[test]
fn nonstandard_nul_padding_is_preserved_but_embedded_nuls_are_invalid() {
    let mut bytes = fixture_bytes();
    bytes[0x8B..0x100].fill(0);
    let inspected = inspect_ip_bin(&bytes);
    assert_eq!(inspected.status, IpBinStatus::SuspiciousButParseable);
    assert_eq!(inspected.ip_bin.unwrap().raw_bytes(), bytes);
    bytes[0x84] = 0;
    assert_eq!(inspect_ip_bin(&bytes).status, IpBinStatus::Malformed);
}
#[test]
fn replacement_byte_width_and_ascii_are_enforced_without_truncation() {
    let (_temp, source, _) = loose(&fixture_bytes());
    for value in [
        "x".repeat(129),
        "Pokémon".into(),
        "TITLE\0".into(),
        "TITLE\n".into(),
    ] {
        assert!(preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, &value)]).is_err());
    }
    assert!(
        preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, &"x".repeat(128))]).is_ok()
    );
}
#[test]
fn calendar_and_version_validation_handles_edge_cases() {
    let (_temp, source, _) = loose(&fixture_bytes());
    for value in ["19000229", "20250229", "20000431", "00000101", "20241301"] {
        assert!(
            preview_ip_bin_edits(&source, &[text(IpBinTextField::ReleaseDate, value)]).is_err()
        );
    }
    for value in ["20000229", "20240229", "19000228"] {
        assert!(preview_ip_bin_edits(&source, &[text(IpBinTextField::ReleaseDate, value)]).is_ok());
    }
    assert!(
        preview_ip_bin_edits(&source, &[text(IpBinTextField::ProductVersion, "V1..00")]).is_err()
    );
}
#[test]
fn title_and_company_preview_are_pure_and_exact_ranges() {
    let (_temp, source, _) = loose(&fixture_bytes());
    let path = &source.source.as_ref().unwrap().path;
    let before = fs::read(path).unwrap();
    let members_before = fs::read_dir(path.parent().unwrap()).unwrap().count();
    let preview = preview_ip_bin_edits(
        &source,
        &[
            text(IpBinTextField::Title, "NEW TITLE"),
            text(IpBinTextField::Company, "COMPANY"),
        ],
    )
    .unwrap();
    assert_eq!(preview.changes()[0].byte_range, 0x80..0x100);
    assert_eq!(preview.changes()[0].original_value, "SYNTHETIC");
    assert_eq!(preview.changes()[0].proposed_value, "NEW TITLE");
    assert_eq!(preview.changes()[0].validation, IpBinFieldValidity::Valid);
    assert!(!preview.integrity_bytes_change());
    assert_eq!(&preview.expected_bytes()[0x100..], &before[0x100..]);
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(
        fs::read_dir(path.parent().unwrap()).unwrap().count(),
        members_before
    );
    assert_eq!(preview.expected_sha256(), digest(preview.expected_bytes()));
}
#[test]
fn region_edit_preserves_unknown_symbols_and_bootstrap() {
    let mut bytes = fixture_bytes();
    bytes[0x34] = b'X';
    let (_temp, source, _) = loose(&bytes);
    let preview = preview_ip_bin_edits(
        &source,
        &[IpBinEdit::Region {
            region: DreamcastRegion::UsaCanada,
            enabled: false,
        }],
    )
    .unwrap();
    assert_eq!(preview.changes()[0].byte_range, 0x31..0x32);
    assert_eq!(preview.expected_bytes()[0x31], b' ');
    assert_eq!(preview.expected_bytes()[0x34], b'X');
    assert_eq!(&preview.expected_bytes()[0x100..], &bytes[0x100..]);
    assert!(!preview.integrity_bytes_change());
}
#[test]
fn enabling_region_requires_existing_area_protection_text() {
    let mut bytes = fixture_bytes();
    bytes[0x32] = b' ';
    let (_temp, source, _) = loose(&bytes);
    let edit = IpBinEdit::Region {
        region: DreamcastRegion::Europe,
        enabled: true,
    };
    assert!(preview_ip_bin_edits(&source, &[edit.clone()]).is_ok());
    bytes[DreamcastRegion::Europe.protection_range()].fill(b' ');
    let (_temp, source, _) = loose(&bytes);
    assert!(preview_ip_bin_edits(&source, &[edit]).is_err());
    bytes[0x30] = b'X';
    let (_temp, source, _) = loose(&bytes);
    assert!(
        preview_ip_bin_edits(
            &source,
            &[IpBinEdit::Region {
                region: DreamcastRegion::Japan,
                enabled: false
            }]
        )
        .is_err()
    );
}
#[test]
fn peripheral_edits_keep_reserved_bits_and_combine_changes() {
    let mut bytes = fixture_bytes();
    bytes[0x38..0x40].copy_from_slice(b"0000012 ");
    let (_temp, source, _) = loose(&bytes);
    let preview = preview_ip_bin_edits(
        &source,
        &[
            IpBinEdit::Peripheral {
                capability: DreamcastPeripheral::Vga,
                declared: false,
            },
            IpBinEdit::Peripheral {
                capability: DreamcastPeripheral::Keyboard,
                declared: true,
            },
        ],
    )
    .unwrap();
    assert_eq!(preview.changes().len(), 1);
    assert_eq!(preview.changes()[0].original_bytes, b"0000012 ");
    assert_eq!(&preview.expected_bytes()[0x38..0x40], b"4000002 ");
    assert_eq!(
        preview
            .expected()
            .ip_bin
            .as_ref()
            .unwrap()
            .peripherals
            .unknown_bits,
        2
    );
}
#[test]
fn boot_filename_edit_changes_only_its_fixed_width_bytes() {
    let (_temp, source, _) = loose(&fixture_bytes());
    let preview =
        preview_ip_bin_edits(&source, &[text(IpBinTextField::BootFilename, "BOOT.BIN")]).unwrap();
    assert_eq!(preview.changes()[0].byte_range, 0x60..0x70);
    assert_eq!(&preview.expected_bytes()[0x60..0x70], b"BOOT.BIN        ");
    assert!(!preview.integrity_bytes_change());
}
#[test]
fn no_op_round_trip_is_byte_identical_even_with_unknowns_and_nul_padding() {
    let mut bytes = fixture_bytes();
    bytes[0x33] = b'X';
    bytes[0x38..0x40].copy_from_slice(b"8000012 ");
    bytes[0x8B..0x100].fill(0);
    let (_temp, source, destination) = loose(&bytes);
    let preview = preview_ip_bin_edits(&source, &[]).unwrap();
    assert!(preview.changes().is_empty());
    assert_eq!(preview.expected_bytes(), bytes);
    assert_eq!(preview.expected_sha256(), preview.source().sha256);
    let same_title =
        preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, "SYNTHETIC")]).unwrap();
    assert!(same_title.changes().is_empty());
    assert_eq!(same_title.expected_bytes(), bytes);
    review_ip_bin_file(&preview, &destination)
        .unwrap()
        .apply()
        .unwrap();
    assert_eq!(fs::read(destination).unwrap(), bytes);
}
#[test]
fn checksum_vectors_product_edit_and_explicit_repair() {
    assert_eq!(product_crc16(b"123456789"), 0x29B1);
    assert_eq!(product_crc16(b""), 0xFFFF);
    // Independently reproducible with Python binascii.crc_hqx(data, 0xFFFF).
    assert_eq!(product_crc16(b"T-1234M   V1.000"), 0xD937);
    let (_temp, source, _) = loose(&fixture_bytes());
    let preview = preview_ip_bin_edits(
        &source,
        &[
            text(IpBinTextField::ProductNumber, "T-5678M"),
            text(IpBinTextField::ProductVersion, "V2.001"),
        ],
    )
    .unwrap();
    assert!(preview.integrity_bytes_change());
    assert_eq!(preview.changes().last().unwrap().byte_range, 0x20..0x24);
    assert_eq!(
        preview
            .expected()
            .ip_bin
            .as_ref()
            .unwrap()
            .product_crc
            .status,
        IpBinProductCrcStatus::Matched
    );
    let mut bytes = fixture_bytes();
    bytes[0x20..0x24].copy_from_slice(b"0000");
    let (_temp, source, _) = loose(&bytes);
    assert_eq!(
        source
            .inspection
            .ip_bin
            .as_ref()
            .unwrap()
            .product_crc
            .status,
        IpBinProductCrcStatus::PlaceholderZero
    );
    let unrelated = preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, "EDIT")]).unwrap();
    assert_eq!(&unrelated.expected_bytes()[0x20..0x24], b"0000");
    let repaired = preview_ip_bin_edits(&source, &[IpBinEdit::RecalculateProductCrc]).unwrap();
    assert_eq!(repaired.changes().len(), 1);
    assert_eq!(
        repaired
            .expected()
            .ip_bin
            .as_ref()
            .unwrap()
            .product_crc
            .status,
        IpBinProductCrcStatus::Matched
    );
}
#[test]
fn checksum_mismatch_is_observed_without_automatic_repair() {
    let mut bytes = fixture_bytes();
    bytes[0x20..0x24].copy_from_slice(b"FFFF");
    let (_temp, source, _) = loose(&bytes);
    assert_eq!(
        source
            .inspection
            .ip_bin
            .as_ref()
            .unwrap()
            .product_crc
            .status,
        IpBinProductCrcStatus::Mismatch
    );
    let preview = preview_ip_bin_edits(&source, &[]).unwrap();
    assert_eq!(preview.expected_bytes(), bytes);
}
#[test]
fn duplicate_edits_are_refused() {
    let (_temp, source, _) = loose(&fixture_bytes());
    let edit = text(IpBinTextField::Title, "EDIT");
    assert!(preview_ip_bin_edits(&source, &[edit.clone(), edit]).is_err());
}
#[test]
fn incomplete_or_inconsistent_typed_snapshots_are_refused() {
    let (_temp, source, _) = loose(&fixture_bytes());
    let mut missing = source.clone();
    missing.inspection.ip_bin = None;
    assert!(preview_ip_bin_edits(&missing, &[]).is_err());
    let mut inconsistent = source;
    inconsistent.source.as_mut().unwrap().sha256 = "0".repeat(64);
    assert!(preview_ip_bin_edits(&inconsistent, &[]).is_err());
    let (_temp, mut truncated, _) = loose(&fixture_bytes()[..256]);
    truncated.inspection.status = IpBinStatus::Valid;
    assert!(preview_ip_bin_edits(&truncated, &[]).is_err());
}
#[test]
fn stale_source_after_preview_and_after_review_never_publishes() {
    for before_review in [true, false] {
        let (_temp, source, destination) = loose(&fixture_bytes());
        let preview =
            preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, "EDIT")]).unwrap();
        let plan = review_ip_bin_file(&preview, &destination).unwrap();
        let path = &preview.source().path;
        let modified = fs::metadata(path).unwrap().modified().unwrap();
        let mut changed = fs::read(path).unwrap();
        changed[IP_BIN_BYTES - 1] ^= 1; // Beyond parsed metadata.
        fs::write(path, &changed).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let error = if before_review {
            review_ip_bin_file(&preview, &destination).unwrap_err()
        } else {
            plan.apply().unwrap_err()
        };
        assert!(error.to_string().contains("STALE PLAN"));
        assert!(!destination.exists());
        assert_eq!(fs::read(path).unwrap(), changed);
    }
}
#[test]
fn replaced_inode_with_identical_bytes_is_stale() {
    let (_temp, source, destination) = loose(&fixture_bytes());
    let preview = preview_ip_bin_edits(&source, &[]).unwrap();
    let replacement = destination.with_extension("replacement");
    fs::write(&replacement, fixture_bytes()).unwrap();
    File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(preview.source().modified)
        .unwrap();
    fs::rename(replacement, &preview.source().path).unwrap();
    assert!(
        review_ip_bin_file(&preview, &destination)
            .unwrap_err()
            .to_string()
            .contains("STALE PLAN")
    );
}
#[test]
fn malformed_edited_output_fails_before_publication() {
    let (_temp, source, destination) = loose(&fixture_bytes());
    let preview = preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, "EDIT")]).unwrap();
    let mut plan = review_ip_bin_file(&preview, &destination).unwrap();
    // Fault injection into a private staged expectation: even updating its hash
    // cannot bypass semantic reparse. Production plans expose no mutable fields.
    let mut broken = plan.preview.expected_bytes().to_vec();
    broken[0] = b'X';
    plan.preview.expected = inspect_ip_bin(&broken);
    plan.preview.expected_sha256 = digest(&broken);
    assert!(plan.apply().is_err());
    assert!(!destination.exists());
    assert_eq!(
        fs::read(&plan.preview.source.path).unwrap(),
        fixture_bytes()
    );
}
#[test]
fn existing_destination_and_late_collision_never_clobber() {
    let (_temp, source, destination) = loose(&fixture_bytes());
    let preview = preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, "EDIT")]).unwrap();
    let plan = review_ip_bin_file(&preview, &destination).unwrap();
    fs::write(&destination, b"owned by someone else").unwrap();
    assert!(review_ip_bin_file(&preview, &destination).is_err());
    assert!(plan.apply().is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"owned by someone else");
    assert!(review_ip_bin_file(&preview, &preview.source().path).is_err());
}
#[test]
fn successful_apply_preserves_source_bytes_identity_and_mtime() {
    let (_temp, source, destination) = loose(&fixture_bytes());
    let preview = preview_ip_bin_edits(&source, &[text(IpBinTextField::Title, "EDIT")]).unwrap();
    let plan = review_ip_bin_file(&preview, &destination).unwrap();
    plan.apply().unwrap();
    assert_eq!(inspect_ip_bin_file(&preview.source().path).unwrap(), source);
    assert_eq!(fs::read(&destination).unwrap(), preview.expected_bytes());
}
#[test]
fn extracted_boot_target_present_absent_case_mismatch_and_ambiguity() {
    let (_temp, source, _) = tree_fixture();
    assert!(matches!(
        inspect_extracted_dreamcast_ip_bin(&source)
            .unwrap()
            .boot_target,
        IpBinBootTargetStatus::Present(_)
    ));
    fs::rename(source.join("1ST_READ.BIN"), source.join("1st_read.bin")).unwrap();
    assert!(matches!(
        inspect_extracted_dreamcast_ip_bin(&source)
            .unwrap()
            .boot_target,
        IpBinBootTargetStatus::CaseMismatch(_)
    ));
    fs::write(source.join("1ST_READ.BIN"), b"second candidate").unwrap();
    assert!(matches!(
        inspect_extracted_dreamcast_ip_bin(&source)
            .unwrap()
            .boot_target,
        IpBinBootTargetStatus::Ambiguous(_)
    ));
    fs::remove_file(source.join("1ST_READ.BIN")).unwrap();
    fs::remove_file(source.join("1st_read.bin")).unwrap();
    let before = fs::read(source.join("bootsector/IP.BIN")).unwrap();
    assert_eq!(
        inspect_extracted_dreamcast_ip_bin(&source)
            .unwrap()
            .boot_target,
        IpBinBootTargetStatus::Missing
    );
    assert_eq!(fs::read(source.join("bootsector/IP.BIN")).unwrap(), before);
    assert_eq!(
        check_boot_target(&source, "../unsafe").unwrap(),
        IpBinBootTargetStatus::NotChecked
    );
}
#[test]
fn extracted_tree_edits_reuse_dcp_publication_and_undo() {
    let (_temp, source, destination) = tree_fixture();
    let before = crate::optical_patch_tree::Contents::read(
        &source,
        crate::dreamcast_dcp_apply::MAX_SOURCE_BYTES,
    )
    .unwrap();
    let inspection = inspect_extracted_dreamcast_ip_bin(&source).unwrap();
    let preview = preview_ip_bin_edits(
        &inspection.ip_bin,
        &[text(IpBinTextField::Title, "TREE EDIT")],
    )
    .unwrap();
    let plan = review_extracted_dreamcast_ip_bin(&source, &destination, &preview).unwrap();
    let prepared = plan.prepare().unwrap();
    assert!(!destination.exists());
    tree::publish(&prepared.journal_path).unwrap();
    assert_eq!(
        fs::read(destination.join("bootsector/IP.BIN")).unwrap(),
        preview.expected_bytes()
    );
    assert_eq!(fs::read(destination.join("keep")).unwrap(), b"untouched");
    before
        .verify(&source, crate::dreamcast_dcp_apply::MAX_SOURCE_BYTES)
        .unwrap();
    tree::undo(&prepared.journal_path).unwrap();
    assert!(!destination.exists());
}
#[test]
fn extracted_boot_edit_requires_present_target_and_tree_freshness() {
    let (_temp, source, destination) = tree_fixture();
    let inspection = inspect_extracted_dreamcast_ip_bin(&source).unwrap();
    let preview = preview_ip_bin_edits(
        &inspection.ip_bin,
        &[text(IpBinTextField::BootFilename, "BOOT.BIN")],
    )
    .unwrap();
    assert!(review_extracted_dreamcast_ip_bin(&source, &destination, &preview).is_err());
    fs::write(source.join("BOOT.BIN"), b"reviewed target").unwrap();
    let plan = review_extracted_dreamcast_ip_bin(&source, &destination, &preview).unwrap();
    let prepared = plan.prepare().unwrap();
    fs::write(source.join("keep"), b"changed dependency").unwrap();
    assert!(plan.prepare().is_err());
    assert!(tree::publish(&prepared.journal_path).is_err());
    assert!(!destination.exists());
}
#[cfg(unix)]
#[test]
fn symlink_sources_targets_and_destination_ancestors_are_refused() {
    use std::os::unix::fs::symlink;
    let (temp, source, destination) = loose(&fixture_bytes());
    let link = temp.path().join("source-link");
    symlink(&source.source.as_ref().unwrap().path, &link).unwrap();
    assert!(inspect_ip_bin_file(&link).is_err());
    let preview = preview_ip_bin_edits(&source, &[]).unwrap();
    let dir_link = temp.path().join("directory-link");
    symlink(temp.path(), &dir_link).unwrap();
    assert!(review_ip_bin_file(&preview, &dir_link.join("out.bin")).is_err());
    symlink("missing", &destination).unwrap();
    assert!(review_ip_bin_file(&preview, &destination).is_err());
    let (_temp, root, _) = tree_fixture();
    fs::remove_file(root.join("1ST_READ.BIN")).unwrap();
    symlink("keep", root.join("1ST_READ.BIN")).unwrap();
    assert!(matches!(
        inspect_extracted_dreamcast_ip_bin(&root)
            .unwrap()
            .boot_target,
        IpBinBootTargetStatus::Unsafe(_)
    ));
}
