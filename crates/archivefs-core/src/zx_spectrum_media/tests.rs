use super::*;

fn block(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// A DATA block body: `bits` data bits, two-pulse zero/one sequences.
fn data_body(bits: u32) -> Vec<u8> {
    let mut body = bits.to_le_bytes().to_vec();
    body.extend_from_slice(&945u16.to_le_bytes()); // tail
    body.extend_from_slice(&[2, 2]); // p0, p1
    for pulse in [855u16, 855, 1710, 1710] {
        body.extend_from_slice(&pulse.to_le_bytes());
    }
    body.extend(std::iter::repeat_n(0xAA, bits.div_ceil(8) as usize));
    body
}

pub(super) fn pzx() -> Vec<u8> {
    let mut out = block(b"PZXT", &[1, 0, b'T', 0]);
    out.extend(block(b"PULS", &[0x7F, 0x08, 0x9B, 0x02]));
    out.extend(block(b"DATA", &data_body(19 * 8)));
    out.extend(block(b"PAUS", &3500u32.to_le_bytes()));
    out.extend(block(b"BRWS", b"Side A\0"));
    out
}

pub(super) fn rzx(snapshot: &[u8], flags: u32) -> Vec<u8> {
    let mut out = b"RZX!".to_vec();
    out.extend_from_slice(&[0, 13]);
    out.extend_from_slice(&flags.to_le_bytes());
    let mut push = |id: u8, body: &[u8]| {
        out.push(id);
        out.extend_from_slice(&((body.len() + 5) as u32).to_le_bytes());
        out.extend_from_slice(body);
    };
    push(0x10, &[0; 24]);
    let mut snap = 0u32.to_le_bytes().to_vec();
    snap.extend_from_slice(b"z80\0");
    snap.extend_from_slice(&(snapshot.len() as u32).to_le_bytes());
    snap.extend_from_slice(snapshot);
    push(0x30, &snap);
    push(0x80, &[0; 13]);
    out
}

/// A v2 `.z80` whose hardware byte says 128K (one uncompressed page).
fn z80_128k() -> Vec<u8> {
    let mut b = vec![0u8; 32 + 23];
    b[30] = 23;
    b[34] = 3;
    b.extend_from_slice(&[0xFF, 0xFF, 3]);
    b.extend(std::iter::repeat_n(0, 16384));
    b
}

pub(super) fn nex(banks: &[usize]) -> Vec<u8> {
    let mut h = vec![0u8; 512];
    h[0..4].copy_from_slice(b"Next");
    h[4..8].copy_from_slice(b"V1.2");
    h[9] = banks.len() as u8;
    for bank in banks {
        h[18 + bank] = 1;
    }
    h.extend(std::iter::repeat_n(0, banks.len() * 16384));
    h
}

pub(super) fn udi(cylinders: u8, sides: u8, track_len: u16) -> Vec<u8> {
    let mut b = b"UDI!".to_vec();
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&[0, cylinders - 1, sides - 1, 0]);
    b.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..u32::from(cylinders) * u32::from(sides) {
        b.push(0);
        b.extend_from_slice(&track_len.to_le_bytes());
        b.extend(std::iter::repeat_n(0x4E, track_len as usize));
        b.extend(std::iter::repeat_n(0, (track_len as usize).div_ceil(8)));
    }
    let declared = b.len() as u32;
    b[4..8].copy_from_slice(&declared.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b
}

fn sum255(bytes: &[u8]) -> u8 {
    (bytes.iter().map(|b| u32::from(*b)).sum::<u32>() % 255) as u8
}

pub(super) fn mdr(sectors: usize) -> Vec<u8> {
    let mut b = Vec::new();
    for n in (1..=sectors).rev() {
        let mut s = vec![0u8; MDR_SECTOR_BYTES];
        s[0] = 1;
        s[1] = n as u8;
        s[4..14].copy_from_slice(b"GAMES     ");
        s[14] = sum255(&s[..14]);
        if n == 1 {
            s[17..19].copy_from_slice(&512u16.to_le_bytes());
            s[19..29].copy_from_slice(b"run       ");
            s[30..542].fill(0xC9);
        }
        s[29] = sum255(&s[15..29]);
        s[542] = sum255(&s[30..542]);
        b.extend(s);
    }
    b.push(0);
    b
}

#[test]
fn pzx_valid_tape_is_structurally_validated_without_a_machine() {
    let r = parse_pzx(&pzx()).unwrap();
    assert_eq!(r.status, SpectrumMediaStatus::StructurallyValidated);
    assert_eq!(r.machine, MachineEvidence::Absent);
    assert!(r.catalogue_eligible());
    assert!(r.summary.contains("1 DATA"), "{}", r.summary);
}

#[test]
fn pzx_truncated_corrupt_and_future_versions_are_distinguished() {
    let good = pzx();
    assert!(matches!(
        parse_pzx(&good[..good.len() - 3]),
        Err(SpectrumMediaRefusal::Truncated(_))
    ));
    let mut bad = block(b"PZXT", &[1, 0]);
    let mut short = data_body(16);
    short.pop();
    bad.extend(block(b"DATA", &short));
    assert!(matches!(
        parse_pzx(&bad),
        Err(SpectrumMediaRefusal::Corrupt(_))
    ));
    let mut future = block(b"PZXT", &[2, 0]);
    future.extend(block(b"NEWB", &[1, 2, 3]));
    let r = parse_pzx(&future).unwrap();
    assert_eq!(r.status, SpectrumMediaStatus::UnsupportedVariant);
    assert!(!r.catalogue_eligible());
    assert!(matches!(
        parse_pzx(b"ZXTape!\x1a\x01\x14"),
        Err(SpectrumMediaRefusal::NotRecognised(_))
    ));
    // A header with nothing to load is not a tape.
    assert!(parse_pzx(&block(b"PZXT", &[1, 0])).is_err());
}

#[test]
fn rzx_is_preservation_only_and_reads_machine_from_an_uncompressed_snapshot() {
    let r = parse_rzx(&rzx(&z80_128k(), 0)).unwrap();
    assert_eq!(r.role, SpectrumMediaRole::PreservationOnly);
    assert_eq!(r.status, SpectrumMediaStatus::StructurallyValidated);
    assert_eq!(
        r.machine,
        MachineEvidence::Encoded(SpectrumMachine::Spectrum128K)
    );
    // Kept as a preservation-only record, which is not a playable role.
    assert!(r.catalogue_eligible());
}

#[test]
fn rzx_signed_is_protected_and_bad_lengths_refuse() {
    let r = parse_rzx(&rzx(&z80_128k(), 1)).unwrap();
    assert_eq!(r.status, SpectrumMediaStatus::Protected);
    assert!(r.catalogue_eligible());
    let mut future = rzx(&[], 0);
    future[4] = 1;
    let r = parse_rzx(&future).unwrap();
    assert_eq!(r.status, SpectrumMediaStatus::UnsupportedVariant);
    assert!(!r.catalogue_eligible());
    let good = rzx(&[], 0);
    assert!(matches!(
        parse_rzx(&good[..good.len() - 1]),
        Err(SpectrumMediaRefusal::Truncated(_))
    ));
    let mut zero = b"RZX!\0\x0d\0\0\0\0".to_vec();
    zero.extend_from_slice(&[0x80, 0, 0, 0, 0]);
    assert!(matches!(
        parse_rzx(&zero),
        Err(SpectrumMediaRefusal::Corrupt(_))
    ));
}

#[test]
fn nex_encodes_spectrum_next_and_checks_bank_table() {
    let r = parse_nex(&nex(&[5, 2, 0]), nex(&[5, 2, 0]).len() as u64).unwrap();
    assert_eq!(
        r.machine,
        MachineEvidence::Encoded(SpectrumMachine::SpectrumNext)
    );
    assert_eq!(r.status, SpectrumMediaStatus::StructurallyValidated);
    let mut mismatch = nex(&[5]);
    mismatch[9] = 2;
    assert!(matches!(
        parse_nex(&mismatch, mismatch.len() as u64),
        Err(SpectrumMediaRefusal::Corrupt(_))
    ));
    let short = nex(&[5, 2]);
    assert!(matches!(
        parse_nex(&short, short.len() as u64 - 1),
        Err(SpectrumMediaRefusal::Truncated(_))
    ));
    let mut future = nex(&[]);
    future[4..8].copy_from_slice(b"V2.0");
    assert_eq!(
        parse_nex(&future, 512).unwrap().status,
        SpectrumMediaStatus::UnsupportedVariant
    );
}

#[test]
fn udi_track_walk_is_partial_because_crc_is_unchecked() {
    let image = udi(2, 2, 100);
    let r = parse_udi(&image).unwrap();
    assert_eq!(r.status, SpectrumMediaStatus::PartiallyUnderstood);
    assert!(r.notes.iter().any(|n| n.contains("CRC32 not verified")));
    assert!(matches!(
        parse_udi(&image[..image.len() - 10]),
        Err(SpectrumMediaRefusal::Truncated(_))
    ));
    let mut raw = image.clone();
    raw[16] = 0xF0;
    assert_eq!(
        parse_udi(&raw).unwrap().status,
        SpectrumMediaStatus::UnsupportedVariant
    );
    // Declaring one track too many leaves the walk short.
    let mut extra = image;
    extra[9] = 2;
    assert!(parse_udi(&extra).is_err());
}

#[test]
fn mdr_checksums_decide_and_size_alone_is_only_ambiguous() {
    let image = mdr(254);
    assert_eq!(image.len() as u64, MAX_MDR_BYTES);
    let r = parse_mdr(&image).unwrap();
    assert_eq!(r.status, SpectrumMediaStatus::StructurallyValidated);
    assert!(r.summary.contains("1 holding data"), "{}", r.summary);
    assert!(matches!(
        parse_mdr(&image[..image.len() - 1]),
        Err(SpectrumMediaRefusal::NotRecognised(_))
    ));
    // Right size, flag bits set, but no checksum agrees: never Spectrum proof.
    let mut sized = vec![0u8; MDR_SECTOR_BYTES * 10 + 1];
    for sector in sized.chunks_exact_mut(MDR_SECTOR_BYTES) {
        sector[0] = 1;
        sector[1] = 7;
        sector[14] = 99;
    }
    assert!(matches!(
        parse_mdr(&sized),
        Err(SpectrumMediaRefusal::Ambiguous(_))
    ));
    assert!(matches!(
        parse_mdr(&vec![0u8; MDR_SECTOR_BYTES * 10 + 1]),
        Err(SpectrumMediaRefusal::Ambiguous(_))
    ));
    // One damaged data block: still recognised, but only partially.
    let mut damaged = mdr(4);
    damaged[MDR_SECTOR_BYTES * 3 + 100] ^= 0xFF;
    let r = parse_mdr(&damaged).unwrap();
    assert_eq!(r.status, SpectrumMediaStatus::PartiallyUnderstood);
    assert!(r.notes[0].contains("3/4 data blocks"), "{:?}", r.notes);
    // Some headers agree and others do not: malformed, not quietly accepted.
    let mut mixed = mdr(4);
    mixed[3] ^= 1;
    assert!(matches!(
        parse_mdr(&mixed),
        Err(SpectrumMediaRefusal::Corrupt(_))
    ));
    // Repeated sector numbers are malformed.
    let mut repeated = mdr(2);
    repeated[MDR_SECTOR_BYTES + 1] = 2;
    repeated[MDR_SECTOR_BYTES + 14] = sum255(&repeated[MDR_SECTOR_BYTES..MDR_SECTOR_BYTES + 14]);
    assert!(matches!(
        parse_mdr(&repeated),
        Err(SpectrumMediaRefusal::Corrupt(_))
    ));
}

#[test]
fn file_inspection_is_bounded_read_only_and_dispatches_by_extension() {
    let dir = tempfile::tempdir().unwrap();
    let cases: [(&str, Vec<u8>); 5] = [
        ("a.pzx", pzx()),
        ("b.rzx", rzx(&z80_128k(), 0)),
        ("c.nex", nex(&[5])),
        ("d.udi", udi(1, 1, 64)),
        ("e.mdr", mdr(3)),
    ];
    for (name, bytes) in &cases {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        inspect_spectrum_media_file(&path).unwrap();
        assert_eq!(&std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
    }
    let big = dir.path().join("big.mdr");
    std::fs::write(&big, vec![0; MAX_MDR_BYTES as usize + 1]).unwrap();
    assert!(matches!(
        inspect_spectrum_media_file(&big),
        Err(SpectrumMediaRefusal::TooLarge { .. })
    ));
    assert!(matches!(
        inspect_spectrum_media_file(&dir.path().join("x.tap")),
        Err(SpectrumMediaRefusal::UnsupportedExtension(_))
    ));
}
