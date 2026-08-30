use super::*;

// --- Z80 fixtures ---------------------------------------------------------

/// A 30-byte v1 header with a non-zero PC and sane shared fields.
fn z80_v1_header(compressed: bool) -> [u8; 30] {
    let mut header = [0u8; 30];
    header[6] = 0x00;
    header[7] = 0x80; // PC = 0x8000 (non-zero -> v1)
    header[8] = 0x00;
    header[9] = 0xC0; // SP = 0xC000
    header[12] = if compressed { 0x20 } else { 0x00 };
    header[27] = 1; // IFF1
    header[28] = 1; // IFF2
    header[29] = 0x01; // interrupt mode 1
    header
}

fn z80_v1_uncompressed() -> Vec<u8> {
    let mut file = z80_v1_header(false).to_vec();
    file.extend(std::iter::repeat_n(0u8, 49152));
    file
}

fn z80_v1_compressed() -> Vec<u8> {
    let mut file = z80_v1_header(true).to_vec();
    file.extend_from_slice(&[0x12, 0x34, 0x56]); // arbitrary "compressed" body
    file.extend_from_slice(&[0x00, 0xED, 0xED, 0x00]); // end marker
    file
}

/// A v2/v3 file: v1 header with PC == 0, then an extra header and one
/// uncompressed memory page.
fn z80_extended(extra_len: u16, hardware_mode: u8, modified_hardware: bool) -> Vec<u8> {
    let mut file = z80_v1_header(false).to_vec();
    file[6] = 0;
    file[7] = 0; // PC == 0 -> extended
    file.extend_from_slice(&extra_len.to_le_bytes());
    let mut extra = vec![0u8; extra_len as usize];
    extra[0] = 0x00;
    extra[1] = 0x90; // PC = 0x9000 (offset 32/33)
    extra[2] = hardware_mode; // offset 34
    if modified_hardware {
        extra[5] = 0x80; // offset 37 bit 7
    }
    file.extend_from_slice(&extra);
    // one uncompressed page: len marker 0xFFFF, page number 8, 16384 bytes
    file.extend_from_slice(&[0xFF, 0xFF, 8]);
    file.extend(std::iter::repeat_n(0u8, 16384));
    file
}

#[test]
fn z80_v1_uncompressed_is_a_48k_snapshot_implied_by_format() {
    let facts = parse_z80_snapshot(&z80_v1_uncompressed()).unwrap();
    assert_eq!(facts.format, SnapshotFormat::Z80V1);
    assert_eq!(facts.pc, Some(0x8000));
    assert_eq!(facts.sp, Some(0xC000));
    assert_eq!(facts.compressed, Some(false));
    assert_eq!(
        facts.machine,
        MachineEvidence::ImpliedByFormat(SpectrumMachine::Spectrum48K)
    );
    assert_eq!(facts.raw_hardware_mode, None);
}

#[test]
fn z80_v1_compressed_needs_the_end_marker() {
    assert!(parse_z80_snapshot(&z80_v1_compressed()).is_ok());
    let mut broken = z80_v1_compressed();
    let last = broken.len() - 1;
    broken[last] = 0x01; // corrupt the 00 ED ED 00 tail
    assert!(matches!(
        parse_z80_snapshot(&broken),
        Err(SnapshotRefusal::Malformed { .. })
    ));
}

#[test]
fn z80_v1_uncompressed_wrong_length_fails_closed() {
    let mut file = z80_v1_uncompressed();
    file.truncate(file.len() - 10);
    assert!(matches!(
        parse_z80_snapshot(&file),
        Err(SnapshotRefusal::Malformed { .. })
    ));
}

#[test]
fn z80_v2_hardware_mode_three_is_128k_not_48k() {
    // In a v2 file, mode 3 is 128K. (In a v3 file the same byte is 48K+MGT -
    // see the next test.) Proving the generation is a required input.
    let facts = parse_z80_snapshot(&z80_extended(23, 3, false)).unwrap();
    assert_eq!(facts.format, SnapshotFormat::Z80V2);
    assert_eq!(
        facts.machine,
        MachineEvidence::Encoded(SpectrumMachine::Spectrum128K)
    );
    assert_eq!(facts.raw_hardware_mode, Some(3));
}

#[test]
fn z80_v3_hardware_mode_three_is_48k_not_128k() {
    let facts = parse_z80_snapshot(&z80_extended(54, 3, false)).unwrap();
    assert_eq!(facts.format, SnapshotFormat::Z80V3);
    assert_eq!(
        facts.machine,
        MachineEvidence::Encoded(SpectrumMachine::Spectrum48K)
    );
}

#[test]
fn z80_v3_hardware_mode_four_is_128k_and_seven_is_plus3() {
    let m128 = parse_z80_snapshot(&z80_extended(54, 4, false)).unwrap();
    assert_eq!(
        m128.machine,
        MachineEvidence::Encoded(SpectrumMachine::Spectrum128K)
    );
    let plus3 = parse_z80_snapshot(&z80_extended(55, 7, false)).unwrap();
    assert_eq!(
        plus3.machine,
        MachineEvidence::Encoded(SpectrumMachine::SpectrumPlus3)
    );
    assert_eq!(plus3.format, SnapshotFormat::Z80V3);
}

#[test]
fn z80_v3_modified_hardware_flag_turns_128k_into_plus2_and_plus3_into_plus2a() {
    let plus2 = parse_z80_snapshot(&z80_extended(54, 4, true)).unwrap();
    assert_eq!(
        plus2.machine,
        MachineEvidence::Encoded(SpectrumMachine::SpectrumPlus2)
    );
    let plus2a = parse_z80_snapshot(&z80_extended(55, 7, true)).unwrap();
    assert_eq!(
        plus2a.machine,
        MachineEvidence::Encoded(SpectrumMachine::SpectrumPlus2A)
    );
}

#[test]
fn z80_v3_pentagon_and_scorpion_modes_are_recognised() {
    assert_eq!(
        parse_z80_snapshot(&z80_extended(54, 9, false))
            .unwrap()
            .machine,
        MachineEvidence::Encoded(SpectrumMachine::Pentagon)
    );
    assert_eq!(
        parse_z80_snapshot(&z80_extended(54, 10, false))
            .unwrap()
            .machine,
        MachineEvidence::Encoded(SpectrumMachine::Scorpion)
    );
}

#[test]
fn z80_unknown_hardware_mode_is_preserved_not_fabricated() {
    let facts = parse_z80_snapshot(&z80_extended(54, 200, false)).unwrap();
    assert_eq!(
        facts.machine,
        MachineEvidence::EncodedButUndocumented { raw: 200 }
    );
    assert_eq!(facts.raw_hardware_mode, Some(200));
    assert_eq!(facts.machine.machine(), None);
}

#[test]
fn z80_impossible_extra_header_length_is_rejected() {
    // 42 is not 23, 54 or 55.
    assert!(matches!(
        parse_z80_snapshot(&z80_extended(42, 0, false)),
        Err(SnapshotRefusal::Malformed { .. })
    ));
}

#[test]
fn z80_page_table_running_past_eof_is_rejected() {
    let mut file = z80_extended(54, 4, false);
    // Truncate into the single memory page.
    file.truncate(file.len() - 4096);
    assert!(matches!(
        parse_z80_snapshot(&file),
        Err(SnapshotRefusal::Malformed { .. })
    ));
}

#[test]
fn z80_random_bytes_fail_closed() {
    let random: Vec<u8> = (0..4096).map(|i| (i * 37 + 11) as u8).collect();
    assert!(parse_z80_snapshot(&random).is_err());
}

#[test]
fn z80_truncated_header_fails_closed() {
    assert!(matches!(
        parse_z80_snapshot(&[0u8; 12]),
        Err(SnapshotRefusal::TooSmall { .. })
    ));
}

#[test]
fn z80_zip_bytes_named_z80_fail_closed() {
    let mut zip = vec![0u8; 4096];
    zip[..4].copy_from_slice(b"PK\x03\x04");
    assert!(parse_z80_snapshot(&zip).is_err());
}

// --- SNA fixtures -------------------------------------------------------

fn sna_48k() -> Vec<u8> {
    let mut file = vec![0u8; SNA_48K_LEN];
    file[23] = 0x00;
    file[24] = 0x80; // SP = 0x8000 (in RAM)
    file[25] = 1; // interrupt mode 1
    file[26] = 7; // border white
    file
}

fn sna_128k() -> Vec<u8> {
    let mut file = vec![0u8; SNA_128K_LEN];
    file[23] = 0x00;
    file[24] = 0x80;
    file[25] = 2;
    file[26] = 0;
    let ext = SNA_HEADER_BYTES + 3 * 16384;
    file[ext] = 0x00;
    file[ext + 1] = 0x60; // PC = 0x6000
    file[ext + 2] = 0x10; // port 0x7FFD
    file[ext + 3] = 0; // TR-DOS not paged
    file
}

#[test]
fn sna_48k_form_is_recognised_by_exact_size() {
    let facts = parse_sna_snapshot(&sna_48k()).unwrap();
    assert_eq!(facts.format, SnapshotFormat::Sna48K);
    assert_eq!(facts.pc, None);
    assert_eq!(facts.sp, Some(0x8000));
    assert_eq!(
        facts.machine,
        MachineEvidence::ImpliedByFormat(SpectrumMachine::Spectrum48K)
    );
}

#[test]
fn sna_128k_form_is_recognised_and_exposes_pc() {
    let facts = parse_sna_snapshot(&sna_128k()).unwrap();
    assert_eq!(facts.format, SnapshotFormat::Sna128K);
    assert_eq!(facts.pc, Some(0x6000));
    assert_eq!(
        facts.machine,
        MachineEvidence::ImpliedByFormat(SpectrumMachine::Spectrum128K)
    );
}

#[test]
fn sna_48k_with_rom_stack_pointer_fails_closed() {
    let mut file = sna_48k();
    file[23] = 0x00;
    file[24] = 0x20; // SP = 0x2000, inside ROM
    assert!(matches!(
        parse_sna_snapshot(&file),
        Err(SnapshotRefusal::Malformed { .. })
    ));
}

#[test]
fn sna_bad_interrupt_mode_or_border_fails_closed() {
    let mut mode = sna_48k();
    mode[25] = 5;
    assert!(parse_sna_snapshot(&mode).is_err());
    let mut border = sna_48k();
    border[26] = 9;
    assert!(parse_sna_snapshot(&border).is_err());
}

#[test]
fn sna_wrong_size_fails_closed() {
    assert!(matches!(
        parse_sna_snapshot(&vec![0u8; SNA_48K_LEN + 1]),
        Err(SnapshotRefusal::Malformed { .. })
    ));
    assert!(matches!(
        parse_sna_snapshot(&vec![0u8; 1024]),
        Err(SnapshotRefusal::TooSmall { .. })
    ));
}

#[test]
fn sna_random_bytes_of_the_right_size_almost_never_pass() {
    // Right size, but the mode/border/SP discriminators reject it.
    let random: Vec<u8> = (0..SNA_48K_LEN).map(|i| (i * 97 + 3) as u8).collect();
    assert!(parse_sna_snapshot(&random).is_err());
}

// --- SZX fixtures -------------------------------------------------------

fn szx(machine_id: u8, blocks: &[(&[u8; 4], usize)]) -> Vec<u8> {
    let mut file = Vec::new();
    file.extend_from_slice(b"ZXST");
    file.push(1); // major
    file.push(4); // minor
    file.push(machine_id);
    file.push(0); // flags
    for (id, size) in blocks {
        file.extend_from_slice(*id);
        file.extend_from_slice(&(*size as u32).to_le_bytes());
        file.extend(std::iter::repeat_n(0u8, *size));
    }
    file
}

#[test]
fn szx_header_and_block_table_validate() {
    let file = szx(2, &[(b"CRTR", 36), (b"Z80R", 37), (b"RAMP", 0x4008)]);
    let facts = parse_szx_snapshot(&file).unwrap();
    assert_eq!(facts.format, SnapshotFormat::Szx);
    assert_eq!(
        facts.machine,
        MachineEvidence::Encoded(SpectrumMachine::Spectrum128K)
    );
}

#[test]
fn szx_machine_ids_map_to_canonical_machines() {
    assert_eq!(
        parse_szx_snapshot(&szx(1, &[])).unwrap().machine,
        MachineEvidence::Encoded(SpectrumMachine::Spectrum48K)
    );
    assert_eq!(
        parse_szx_snapshot(&szx(5, &[])).unwrap().machine,
        MachineEvidence::Encoded(SpectrumMachine::SpectrumPlus3)
    );
    assert_eq!(
        parse_szx_snapshot(&szx(7, &[])).unwrap().machine,
        MachineEvidence::Encoded(SpectrumMachine::Pentagon)
    );
}

#[test]
fn szx_unknown_machine_id_is_preserved_not_fabricated() {
    let facts = parse_szx_snapshot(&szx(240, &[])).unwrap();
    assert_eq!(
        facts.machine,
        MachineEvidence::EncodedButUndocumented { raw: 240 }
    );
}

#[test]
fn szx_block_running_past_eof_is_rejected() {
    let mut file = szx(1, &[(b"RAMP", 64)]);
    file.truncate(file.len() - 32);
    assert!(matches!(
        parse_szx_snapshot(&file),
        Err(SnapshotRefusal::Malformed { .. })
    ));
}

#[test]
fn szx_wrong_magic_is_not_recognised() {
    let mut file = szx(1, &[]);
    file[1] = b'Y';
    assert!(matches!(
        parse_szx_snapshot(&file),
        Err(SnapshotRefusal::NotRecognised { .. })
    ));
}

// --- evidence -----------------------------------------------------------

#[test]
fn evidence_is_content_only_and_grades_machine_confidence_by_how_it_was_known() {
    // Encoded -> Strong.
    let encoded = parse_z80_snapshot(&z80_extended(54, 4, false)).unwrap();
    let ev = observe_spectrum_snapshot_evidence(&encoded);
    assert_eq!(ev.len(), 2);
    assert!(
        ev.iter()
            .all(|e| e.kind == ContentEvidenceKind::ContentSignature)
    );
    let machine_item = &ev[1];
    assert_eq!(machine_item.value, "ZX Spectrum 128K");
    assert_eq!(machine_item.confidence, ContentEvidenceConfidence::Strong);

    // Implied by format -> Corroborated, never Strong.
    let implied = parse_z80_snapshot(&z80_v1_uncompressed()).unwrap();
    let ev = observe_spectrum_snapshot_evidence(&implied);
    assert_eq!(ev[1].confidence, ContentEvidenceConfidence::Corroborated);

    // Undocumented -> Weak, and the value string never names a subtype.
    let unknown = parse_z80_snapshot(&z80_extended(54, 200, false)).unwrap();
    let ev = observe_spectrum_snapshot_evidence(&unknown);
    assert_eq!(ev[1].confidence, ContentEvidenceConfidence::Weak);
    assert!(ev[1].value.contains("not encoded"));
}

#[test]
fn detectors_recognise_their_own_format_and_decline_others() {
    assert!(matches!(
        Z80SnapshotDetector.detect(&z80_v1_uncompressed()),
        ContentDetectionOutcome::Recognized { .. }
    ));
    assert!(matches!(
        Z80SnapshotDetector.detect(&sna_48k()),
        ContentDetectionOutcome::NotRecognized
    ));
    assert!(matches!(
        SnaSnapshotDetector.detect(&sna_128k()),
        ContentDetectionOutcome::Recognized { .. }
    ));
    assert!(matches!(
        SzxSnapshotDetector.detect(&szx(1, &[])),
        ContentDetectionOutcome::Recognized { .. }
    ));
    assert!(matches!(
        SzxSnapshotDetector.detect(b"not szx at all"),
        ContentDetectionOutcome::NotRecognized
    ));
}

#[test]
fn parsing_is_deterministic_and_never_mutates_input() {
    let file = z80_extended(55, 7, true);
    let before = file.clone();
    let a = parse_z80_snapshot(&file);
    let b = parse_z80_snapshot(&file);
    assert_eq!(a, b);
    assert_eq!(file, before);
}

#[test]
fn file_entry_point_reads_bounded_and_rejects_foreign_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("Game (1987).z80");
    std::fs::write(&good, z80_v1_uncompressed()).unwrap();
    let inspection = inspect_spectrum_snapshot_file(&good).unwrap();
    assert_eq!(inspection.facts.format, SnapshotFormat::Z80V1);
    assert!(!inspection.machine_subtype_is_encoded());
    assert_eq!(inspection.machine(), Some(SpectrumMachine::Spectrum48K));

    let wrong_ext = dir.path().join("Game.tap");
    std::fs::write(&wrong_ext, b"whatever").unwrap();
    assert!(matches!(
        inspect_spectrum_snapshot_file(&wrong_ext),
        Err(SnapshotRefusal::UnsupportedExtension(_))
    ));

    let liar = dir.path().join("Definitely A Game.z80");
    std::fs::write(&liar, b"this is not a z80 snapshot").unwrap();
    assert!(inspect_spectrum_snapshot_file(&liar).is_err());
}
