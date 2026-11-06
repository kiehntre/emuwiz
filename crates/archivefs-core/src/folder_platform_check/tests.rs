use super::*;
use std::io::Write;

fn nes_like() -> Vec<u8> {
    let mut b = vec![0xEA_u8; 32 * 1024];
    let end = b.len();
    let target = end - 16 * 1024;
    b[target] = 0x78;
    b[end - 6..].copy_from_slice(&[0x00, 0xC1, 0x00, 0xC0, 0x00, 0xC1]);
    b
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
