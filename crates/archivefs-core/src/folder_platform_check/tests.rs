use super::*;
use std::io::Write;

/// A headerless NES image: `prg` bytes of PRG (vectors at its end), then `chr`
/// bytes of CHR. RESET code (SEI, CLD) is placed where the detector maps it.
fn nes_layout(prg: usize, chr: usize, nmi: u16, reset: u16, irq: u16) -> Vec<u8> {
    let mut b = vec![0xEA_u8; prg + chr];
    let r = usize::from(reset);
    let target = if r >= 0xC000 {
        prg - 16 * 1024 + (r - 0xC000)
    } else if prg >= 32 * 1024 {
        prg - 32 * 1024 + (r - 0x8000)
    } else {
        r - 0x8000
    };
    b[target] = 0x78;
    b[target + 1] = 0xD8;
    b[prg - 6..prg - 4].copy_from_slice(&nmi.to_le_bytes());
    b[prg - 4..prg - 2].copy_from_slice(&reset.to_le_bytes());
    b[prg - 2..prg].copy_from_slice(&irq.to_le_bytes());
    b
}

fn nes_like() -> Vec<u8> {
    nes_like_sized(40 * 1024)
}

/// 40 KiB = 32 KiB PRG + 8 KiB CHR; larger sizes are CHR-RAM style (PRG only).
fn nes_like_sized(len: usize) -> Vec<u8> {
    if len % (16 * 1024) == 8 * 1024 {
        nes_layout(len - 8 * 1024, 8 * 1024, 0x8100, 0x8E19, 0x8F00)
    } else {
        nes_layout(len, 0, 0xC100, 0xC000, 0xC100)
    }
}

fn hit(platform: &str) -> HashHit {
    HashHit {
        platform: platform.into(),
        dat_name: "Nintendo - Nintendo Entertainment System (Headerless)".into(),
        dat_version: Some("20260101".into()),
        source_id: "local:207".into(),
        entry_name: "Game (USA) (Unl)".into(),
        rom_name: "Game (USA) (Unl).unh".into(),
        sha1: "ab".repeat(20),
        provenance: DERIVED_PROVENANCE.into(),
    }
}

fn sig(platform: &str) -> Signal {
    Signal {
        platform: platform.into(),
        reason: "test".into(),
    }
}

fn ev(assigned: &str) -> Evidence {
    Evidence {
        assigned: assigned.into(),
        ..Default::default()
    }
}

#[test]
fn hash_hit_in_other_system_is_confirmed() {
    let mut e = ev("SNES");
    e.hash_hits.push(hit("NES"));
    let c = classify(&e).unwrap();
    assert_eq!(c.tier, Tier::Confirmed);
    assert_eq!(c.detected, "NES");
    assert!(c.lines[0].contains("version 20260101"));
    assert!(c.lines[0].contains("Game (USA) (Unl)"));
    assert!(c.lines[0].contains(&"ab".repeat(20)));
    assert!(c.lines[0].contains(DERIVED_PROVENANCE));
}

#[test]
fn hash_hit_in_own_system_is_no_mismatch() {
    let mut e = ev("NES");
    e.hash_hits.push(hit("NES"));
    assert!(classify(&e).is_none());
    e.hash_hits.push(hit("SNES"));
    assert!(classify(&e).is_none());
}

#[test]
fn confirmed_is_demoted_by_contradicting_content() {
    let mut e = ev("SNES");
    e.hash_hits.push(hit("NES"));
    e.signatures.push(sig("SNES"));
    assert_eq!(classify(&e).unwrap().tier, Tier::Ambiguous);
    let mut e = ev("SNES");
    e.hash_hits.push(hit("NES"));
    e.signatures.push(sig("Game Boy"));
    assert_eq!(classify(&e).unwrap().tier, Tier::Ambiguous);
    let mut e = ev("SNES");
    e.hash_hits.push(hit("NES"));
    e.hash_hits.push(hit("Game Boy"));
    assert_eq!(classify(&e).unwrap().tier, Tier::Ambiguous);
}

#[test]
fn structure_without_hash_is_probable_never_confirmed() {
    let mut e = ev("SNES");
    e.structure.push(sig("NES"));
    let c = classify(&e).unwrap();
    assert_eq!(c.tier, Tier::Probable);
    assert!(c.lines[0].contains("no catalogue hash matched"));
}

#[test]
fn conflicting_structure_is_ambiguous() {
    let mut e = ev("SNES");
    e.structure.push(sig("NES"));
    e.signatures.push(sig("Game Boy Advance"));
    assert_eq!(classify(&e).unwrap().tier, Tier::Ambiguous);
}

#[test]
fn structure_for_assigned_system_is_not_a_mismatch() {
    let mut e = ev("SNES");
    e.signatures.push(sig("SNES"));
    e.structure.push(sig("NES"));
    assert!(classify(&e).is_none());
}

#[test]
fn nothing_is_never_evidence_from_name_or_extension() {
    // No hash, no signature, no structure: names/extensions are not inputs.
    assert!(classify(&ev("SNES")).is_none());
    // Plain data with a .unh name carries no structure.
    assert!(nes_structure(&vec![0u8; 32768]).is_none());
    assert!(header_signals(&vec![0u8; 32768]).is_empty());
}

#[test]
fn nes_structure_needs_real_vectors_and_no_ines_header() {
    assert!(nes_structure(&nes_like()).is_some());
    let mut with_header = nes_like();
    with_header[..4].copy_from_slice(b"NES\x1a");
    assert!(nes_structure(&with_header).is_none());
    assert!(
        header_signals(&with_header)
            .iter()
            .any(|s| s.platform == "NES")
    );
    assert!(nes_structure(&nes_like()[..1000]).is_none());
}

#[test]
fn game_boy_family_is_consistent() {
    assert!(consistent("Game Boy", "Game Boy Color"));
    assert!(!consistent("Game Boy", "NES"));
}

// ---- catalogue platform derivation -------------------------------------

#[test]
fn headerless_and_headered_nes_derive_nes_from_header_name() {
    for name in [
        "Nintendo - Nintendo Entertainment System (Headerless)",
        "Nintendo - Nintendo Entertainment System (Headered)",
    ] {
        let d = derive_catalogue_platform(Some(name), Some(DatEcosystem::NoIntro)).unwrap();
        assert_eq!(d.platform, "NES", "{name}");
        assert_eq!(d.provenance, "derived from DAT header name");
    }
}

#[test]
fn ambiguous_unmapped_or_missing_names_are_refused() {
    assert!(derive_catalogue_platform(None, None).is_none());
    assert!(derive_catalogue_platform(Some(""), None).is_none());
    assert!(derive_catalogue_platform(Some("Totally Unknown Machine"), None).is_none());
    assert!(derive_catalogue_platform(Some("Nintendo - Wallpapers"), None).is_none());
    assert!(derive_catalogue_platform(Some("MAME"), Some(DatEcosystem::MAMEArcade)).is_none());
}

fn write_dat(dir: &Path, name: &str, sha1: &str) -> PathBuf {
    let path = dir.join("headerless.dat");
    let xml = format!(
        "<?xml version=\"1.0\"?><datafile><header><name>{name}</name><description>{name}</description><version>20260101</version></header><game name=\"Game (USA) (Unl)\"><description>Game</description><rom name=\"Game (USA) (Unl).unh\" size=\"32768\" sha1=\"{sha1}\"/></game></datafile>"
    );
    std::fs::write(&path, xml).unwrap();
    path
}

#[test]
fn derived_catalogue_finds_hash_and_never_writes_config() {
    let dir = tempfile::tempdir().unwrap();
    let rom = nes_like();
    let sha1 = sha1_hex(&rom);
    let dat_path = write_dat(
        dir.path(),
        "Nintendo - Nintendo Entertainment System (Headerless)",
        &sha1,
    );
    let config = dir.path().join("dat_sources.toml");
    std::fs::write(&config, "[[sources]]\nid = \"207\"\n").unwrap();
    let before = std::fs::read(&config).unwrap();

    let parsed = crate::dat::parsers::parse_dat_file(&dat_path, Default::default()).unwrap();
    let wanted = cartridge_platforms();
    let catalogue = catalogue_from_parsed(&parsed.dat, "207", "x", &wanted).unwrap();
    assert_eq!(catalogue.platform, "NES");

    // A zip holding one .unh member in a folder assigned to SNES.
    let zip_path = dir.path().join("snes").join("Game (USA) (Unl).zip");
    std::fs::create_dir_all(zip_path.parent().unwrap()).unwrap();
    write_zip(&zip_path, &[("Game (USA) (Unl).unh", &rom)]);
    let candidate = Candidate {
        archive_id: 1,
        title: "Game".into(),
        path: zip_path.clone(),
        assigned: "SNES".into(),
        assigned_source: Some("folder_alias".into()),
    };
    let trusted = TrustedRoots::none();
    let cancel = AtomicBool::new(false);
    let mut seen = 0;
    let out = check_candidates(
        &[candidate],
        0,
        10,
        &[catalogue],
        &trusted,
        &cancel,
        &mut |n| seen = n,
    );
    assert_eq!(seen, 1);
    assert_eq!(out.checked, 1);
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.findings[0].tier, Tier::Confirmed);
    assert_eq!(out.findings[0].detected, "NES");
    assert_eq!(
        std::fs::read(&config).unwrap(),
        before,
        "config must be untouched"
    );
}

#[test]
fn run_is_capped_and_cancellable() {
    let dir = tempfile::tempdir().unwrap();
    let mut candidates = Vec::new();
    for i in 0..5 {
        let path = dir.path().join(format!("g{i}.bin"));
        std::fs::write(&path, nes_like()).unwrap();
        candidates.push(Candidate {
            archive_id: i,
            title: format!("g{i}"),
            path,
            assigned: "SNES".into(),
            assigned_source: None,
        });
    }
    let trusted = TrustedRoots::none();
    let cancel = AtomicBool::new(false);
    let out = check_candidates(&candidates, 1, 2, &[], &trusted, &cancel, &mut |_| {});
    assert_eq!(out.checked, 2);
    assert_eq!(out.findings.len(), 2);
    assert!(out.findings.iter().all(|f| f.tier == Tier::Probable));
    assert_eq!(out.findings[0].archive_id, 1);
    let stop = AtomicBool::new(true);
    let out = check_candidates(&candidates, 0, 5, &[], &trusted, &stop, &mut |_| {});
    assert!(out.cancelled);
    assert_eq!(out.checked, 0);
}

#[test]
fn check_never_changes_files_or_database_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite");
    std::fs::write(&db, b"stand-in database bytes").unwrap();
    let rom = dir.path().join("a.bin");
    std::fs::write(&rom, nes_like()).unwrap();
    let (db_before, rom_before) = (std::fs::read(&db).unwrap(), std::fs::read(&rom).unwrap());
    let candidate = Candidate {
        archive_id: 1,
        title: "a".into(),
        path: rom.clone(),
        assigned: "SNES".into(),
        assigned_source: None,
    };
    let _ = check_candidates(
        &[candidate],
        0,
        1,
        &[],
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert_eq!(std::fs::read(&db).unwrap(), db_before);
    assert_eq!(std::fs::read(&rom).unwrap(), rom_before);
    // The module has no database or config write path at all.
    let source = include_str!("../folder_platform_check.rs");
    for forbidden in [
        "set_manual_platform",
        "Database::open(",
        ".execute(",
        "save_dat_sources",
        "fs::write",
        "fs::rename",
        "fs::remove",
    ] {
        assert!(!source.contains(forbidden), "{forbidden}");
    }
}

// ---- bounded zip reader --------------------------------------------------

fn write_zip(path: &Path, members: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in members {
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

fn read_zip(members: &[(&str, &[u8])]) -> Result<Vec<u8>, ReadRefusal> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.zip");
    write_zip(&path, members);
    read_single_zip_member(std::fs::File::open(&path).unwrap(), 0)
}

#[test]
fn zip_reads_one_member_of_any_extension() {
    assert_eq!(read_zip(&[("x.unh", b"hello")]).unwrap(), b"hello");
}

#[test]
fn zip_refuses_unsafe_or_ambiguous() {
    assert!(matches!(
        read_zip(&[("a", b"1"), ("b", b"2")]),
        Err(ReadRefusal::NotSingleMember(_))
    ));
    assert!(matches!(
        read_zip(&[]),
        Err(ReadRefusal::NotSingleMember(_))
    ));
    assert!(matches!(
        read_zip(&[("../evil.unh", b"1")]),
        Err(ReadRefusal::Unsafe(_))
    ));
    assert!(matches!(
        read_zip(&[("/abs.unh", b"1")]),
        Err(ReadRefusal::Unsafe(_))
    ));
    assert!(matches!(
        read_zip(&[("C:x.unh", b"1")]),
        Err(ReadRefusal::Unsafe(_))
    ));
}

#[test]
fn zip_refuses_oversize_member() {
    let big = vec![0u8; (MAX_CHECK_BYTES + 1) as usize];
    assert!(matches!(
        read_zip(&[("big.unh", &big)]),
        Err(ReadRefusal::TooLarge(_))
    ));
}

#[test]
fn zip_refuses_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    zip.add_symlink("link.unh", "target", options).unwrap();
    zip.finish().unwrap();
    let result = read_single_zip_member(std::fs::File::open(&path).unwrap(), 0);
    assert!(matches!(result, Err(ReadRefusal::Unsafe(_))), "{result:?}");
}

#[test]
fn loose_oversize_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.bin");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_CHECK_BYTES + 1).unwrap();
    let result = read_check_bytes(&path, &TrustedRoots::none());
    assert!(matches!(result, Err(ReadRefusal::TooLarge(_))));
}

// ---- Probable tier strictness ---------------------------------------------

fn xorshift(state: &mut u64) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 24) as u8
}

/// Regression guard, not a proof: 20,000 deterministic pseudo-random images
/// must never look like NES. Real false-positive rates on real ROMs are not
/// measured here.
#[test]
fn random_data_is_never_probable_nes() {
    let sizes = [24 * 1024, 40 * 1024, 72 * 1024, 136 * 1024];
    let mut false_positives = 0;
    for seed in 1..=20_000_u64 {
        let size = sizes[(seed % 4) as usize];
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut data = Vec::with_capacity(size);
        while data.len() < size {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            data.extend_from_slice(&state.to_le_bytes());
        }
        data.truncate(size);
        if nes_structure(&data).is_some() {
            false_positives += 1;
        }
    }
    assert_eq!(false_positives, 0);
}

#[test]
fn realistic_nes_layouts_are_probable() {
    // NROM-128: 16K PRG + 8K CHR, RESET in $8000-$BFFF (mirrored bank).
    assert!(nes_structure(&nes_layout(16 * 1024, 8 * 1024, 0x8500, 0x8E19, 0x8F00)).is_some());
    // NROM-256 + CHR.
    assert!(nes_structure(&nes_layout(32 * 1024, 8 * 1024, 0x8100, 0x8E19, 0x8F00)).is_some());
    // MMC3-style 128K PRG + 128K CHR: vectors at the end of PRG, not EOF.
    assert!(nes_structure(&nes_layout(128 * 1024, 128 * 1024, 0xE100, 0xFFE0, 0xF000)).is_some());
    // CHR-RAM UxROM: 128K PRG only.
    assert!(nes_structure(&nes_layout(128 * 1024, 0, 0xC100, 0xC000, 0xC200)).is_some());
    // Other accepted start sequences.
    for pair in [[0xD8, 0x78], [0x78, 0xA2], [0x78, 0xA9], [0x78, 0xAD]] {
        let mut image = nes_layout(128 * 1024, 0, 0xC100, 0xC000, 0xC200);
        let target = image.len() - 16 * 1024;
        image[target..target + 2].copy_from_slice(&pair);
        assert!(nes_structure(&image).is_some(), "{pair:02x?}");
    }
}

#[test]
fn atari_and_sms_shaped_images_are_not_probable_nes() {
    // 48K headerless 7800: 6502 vectors at EOF, SEI/CLD start.
    assert!(nes_structure(&nes_layout(48 * 1024, 0, 0xF100, 0xF000, 0xF200)).is_none());
    // 2600/7800-style 16K and 32K with distinct NMI/RESET.
    assert!(nes_structure(&nes_layout(16 * 1024, 0, 0xF100, 0xF000, 0xF200)).is_none());
    assert!(nes_structure(&nes_layout(32 * 1024, 0, 0xF100, 0xF000, 0xF200)).is_none());
    // 2600 repeated vectors.
    assert!(nes_structure(&nes_layout(32 * 1024, 0, 0xF000, 0xF000, 0xF000)).is_none());
    // SMS/GG without "TMR SEGA": Z80-looking bytes, no 6502 vectors.
    let mut state = 0xDEAD_BEEF_u64;
    let z80: Vec<u8> = (0..128 * 1024)
        .map(|i| match i % 4 {
            0 => 0xF3,
            1 => 0xC3,
            _ => xorshift(&mut state),
        })
        .collect();
    assert!(nes_structure(&z80).is_none());
}

#[test]
fn other_systems_signatures_veto_the_nes_structure() {
    let stamps: Vec<(&str, usize, Vec<u8>)> = vec![
        ("SMS 0x7FF0", 0x7FF0, b"TMR SEGA".to_vec()),
        ("GG 0x3FF0", 0x3FF0, b"TMR SEGA".to_vec()),
        ("SMS 0x1FF0", 0x1FF0, b"TMR SEGA".to_vec()),
        ("Genesis", 0x100, b"SEGA MEGA DRIVE ".to_vec()),
        ("Genesis (bad name)", 0x101, b"SEGA".to_vec()),
        (
            "Game Boy logo",
            0x104,
            vec![0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B],
        ),
        ("GBA logo", 0x04, vec![0x24, 0xFF, 0xAE, 0x51]),
        ("N64", 0, vec![0x80, 0x37, 0x12, 0x40]),
        ("Atari 7800", 1, b"ATARI7800".to_vec()),
        ("Lynx", 0, b"LYNX".to_vec()),
        ("NGP", 0, b"COPYRIGHT BY SNK".to_vec()),
    ];
    for size in [40 * 1024, 128 * 1024, 512 * 1024] {
        assert!(nes_structure(&nes_like_sized(size)).is_some(), "{size}");
        for (name, offset, bytes) in &stamps {
            let mut image = nes_like_sized(size);
            image[*offset..*offset + bytes.len()].copy_from_slice(bytes);
            assert!(nes_structure(&image).is_none(), "{name} at {size}");
        }
        // SNES image with a plausible header but a wrong checksum.
        let mut snes = nes_like_sized(size);
        let title = b"SUPER TEST GAME      ";
        snes[0x7FC0..0x7FC0 + 21].copy_from_slice(title);
        snes[0x7FD5] = 0x20;
        assert!(header_signals(&snes).iter().all(|s| s.platform != "SNES"));
        assert!(nes_structure(&snes).is_none(), "bad-header SNES at {size}");
    }
}

#[test]
fn repeated_vectors_and_odd_start_code_are_refused() {
    let mut repeated = nes_layout(128 * 1024, 0, 0xC100, 0xC000, 0xC100);
    let end = repeated.len();
    repeated[end - 6..].copy_from_slice(&[0x00, 0xC0, 0x00, 0xC0, 0x00, 0xC0]);
    assert!(nes_structure(&repeated).is_none());
    let mut other_start = nes_layout(128 * 1024, 0, 0xC100, 0xC000, 0xC200);
    let target = other_start.len() - 16 * 1024;
    other_start[target] = 0xA9;
    assert!(nes_structure(&other_start).is_none());
    // Size rules.
    assert!(nes_structure(&nes_like()[..24 * 1024 + 100]).is_none());
}

// ---- related-system families ---------------------------------------------

fn id(alias: &str) -> String {
    crate::canonical_platform_for_alias(alias)
        .unwrap()
        .to_string()
}

#[test]
fn related_systems_are_not_a_confirmed_mismatch() {
    for (a, b) in [
        ("mastersystem", "gamegear"),
        ("gamegear", "mastersystem"),
        ("megadrive", "32x"),
        ("32x", "megadrive"),
        ("gb", "gbc"),
        ("ngp", "ngpc"),
    ] {
        let mut e = ev(&id(a));
        e.hash_hits.push(hit(&id(b)));
        assert!(classify(&e).is_none(), "{a} vs {b}");
    }
    let mut e = ev(&id("mastersystem"));
    e.hash_hits.push(hit("NES"));
    assert_eq!(classify(&e).unwrap().tier, Tier::Confirmed);
}

// ---- cancellation, counts, wording support --------------------------------

#[test]
fn cancelled_catalogue_load_is_reported_incomplete() {
    use crate::dat::catalogue_selection::CatalogueRef;
    use crate::dat::configured_catalogues::ConfiguredCatalogue;
    let mut configured = std::collections::HashMap::new();
    configured.insert(
        "x".to_string(),
        ConfiguredCatalogue {
            reference: CatalogueRef::local("x"),
            source_id: "x".into(),
            display_name: "x".into(),
            path: PathBuf::from("/nonexistent.dat"),
            kind: crate::dat::sources::DatSourceKind::File,
            sha256: None,
            revision: None,
        },
    );
    let (list, complete) = load_cartridge_catalogues(&configured, &AtomicBool::new(true));
    assert!(list.is_empty());
    assert!(!complete);
    let (_, complete) = load_cartridge_catalogues(&configured, &AtomicBool::new(false));
    assert!(complete);
}

#[test]
fn refusals_are_counted_separately_and_unh_is_a_real_extension_fact() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.bin");
    let big = dir.path().join("big.bin");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(MAX_CHECK_BYTES + 1)
        .unwrap();
    let multi = dir.path().join("multi.zip");
    write_zip(&multi, &[("a", b"1"), ("b", b"2")]);
    let unh = dir.path().join("unh.zip");
    write_zip(&unh, &[("Game.unh", &nes_like())]);
    let plain = dir.path().join("plain.bin");
    std::fs::write(&plain, nes_like()).unwrap();
    let candidates: Vec<Candidate> = [missing, big, multi, unh, plain]
        .into_iter()
        .enumerate()
        .map(|(i, path)| Candidate {
            archive_id: i as i64,
            title: format!("g{i}"),
            path,
            assigned: "SNES".into(),
            assigned_source: None,
        })
        .collect();
    let out = check_candidates(
        &candidates,
        0,
        10,
        &[],
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert_eq!((out.unreadable, out.too_large, out.skipped), (1, 1, 1));
    assert_eq!(out.findings.len(), 2);
    assert!(
        out.findings
            .iter()
            .find(|f| f.archive_id == 3)
            .unwrap()
            .has_unh
    );
    assert!(
        !out.findings
            .iter()
            .find(|f| f.archive_id == 4)
            .unwrap()
            .has_unh
    );
}

/// An image whose code writes every NES-only register, among padding.
fn nes_program(len: usize, registers: &[u16]) -> Vec<u8> {
    let mut b = vec![0xEA_u8; len];
    for (i, r) in registers.iter().enumerate() {
        let at = 0x100 + i * 16;
        b[at] = 0x8D; // STA absolute
        b[at + 1..at + 3].copy_from_slice(&r.to_le_bytes());
    }
    b
}

#[test]
fn nes_register_validator_needs_every_nes_only_register() {
    let all = [0x2000, 0x2001, 0x2006, 0x2007, 0x4014, 0x4016];
    assert!(nes_register_signature(&nes_program(32 * 1024, &all)).is_some());
    for missing in 0..all.len() {
        let some: Vec<u16> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != missing)
            .map(|(_, r)| *r)
            .collect();
        assert!(
            nes_register_signature(&nes_program(32 * 1024, &some)).is_none(),
            "{missing}"
        );
    }
    // Reads (LDA abs, AD) are not writes.
    let mut reads = nes_program(32 * 1024, &all);
    reads[0x100 + 5 * 16] = 0xAD;
    assert!(nes_register_signature(&reads).is_none());
}

#[test]
fn validator_without_dat_match_is_platform_confirmed_not_unknown() {
    let mut e = ev(&id("snes"));
    e.validators = vec![sig(&id("nes"))];
    let c = classify(&e).unwrap();
    assert_eq!(c.tier, Tier::PlatformConfirmed);
    assert!(c.tier.is_movable());
    assert_eq!(c.detected, id("nes"));
    assert!(c.lines[0].contains("game identity unverified"));
}

#[test]
fn validator_contradicted_by_other_content_is_ambiguous_and_never_movable() {
    let mut e = ev(&id("snes"));
    e.validators = vec![sig(&id("nes"))];
    e.signatures = vec![sig(&id("megadrive"))];
    let c = classify(&e).unwrap();
    assert_eq!(c.tier, Tier::Ambiguous);
    assert!(!c.tier.is_movable());
}

#[test]
fn validator_agreeing_with_folder_is_no_mismatch() {
    let mut e = ev(&id("nes"));
    e.validators = vec![sig(&id("nes"))];
    assert!(classify(&e).is_none());
}

#[test]
fn probable_structure_alone_is_never_movable() {
    assert!(!Tier::Probable.is_movable());
    assert!(!Tier::Ambiguous.is_movable());
}

#[test]
fn a_verified_game_outside_its_system_folder_is_movable_without_changing_system() {
    let mut e = ev(&id("nes"));
    e.hash_hits = vec![hit(&id("nes"))];
    let c = outside_own_folder(&e, Path::new("/games/mixed/Game.zip")).unwrap();
    assert_eq!(c.tier, Tier::Confirmed);
    assert_eq!(c.detected, id("nes"));
    assert!(outside_own_folder(&e, Path::new("/games/nes/Game.zip")).is_none());
    assert!(outside_own_folder(&e, Path::new("/games/famicom/Game.zip")).is_none());
}

#[test]
fn a_header_alone_outside_the_folder_is_platform_confirmed_and_contradictions_stay_put() {
    let mut e = ev(&id("gba"));
    e.signatures = vec![sig(&id("gba"))];
    let c = outside_own_folder(&e, Path::new("/games/mixed/Game.zip")).unwrap();
    assert_eq!(c.tier, Tier::PlatformConfirmed);
    e.structure = vec![sig(&id("nes"))];
    assert!(outside_own_folder(&e, Path::new("/games/mixed/Game.zip")).is_none());
    // Folder name and nothing else is never evidence.
    assert!(outside_own_folder(&ev(&id("gba")), Path::new("/games/mixed/Game.zip")).is_none());
}
