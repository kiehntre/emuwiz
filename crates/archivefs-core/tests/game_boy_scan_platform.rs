//! A loose `.gb`/`.gbc` ROM gets its platform at scan time from its own
//! cartridge header, never from the extension alone.
archivefs_core::install_test_environment!();

use archivefs_core::{ArchiveScanner, Config, PlatformProvenance};
use std::path::Path;

const LOGO: [u8; 48] = [
    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00, 0x0D,
    0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99,
    0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
];

fn rom(cgb: u8, logo: bool, checksum_ok: bool, len: usize) -> Vec<u8> {
    let mut b = vec![0u8; len];
    if logo {
        b[0x104..0x134].copy_from_slice(&LOGO);
    }
    b[0x134..0x13C].copy_from_slice(b"SYNTH GB");
    b[0x143] = cgb;
    let mut sum = 0u8;
    for byte in &b[0x134..=0x14C] {
        sum = sum.wrapping_sub(*byte).wrapping_sub(1);
    }
    b[0x14D] = if checksum_ok { sum } else { sum ^ 0xFF };
    b
}

fn scan(root: &Path) -> Vec<(String, Option<String>, Option<PlatformProvenance>)> {
    let config = Config {
        source_folders: vec![root.to_path_buf()],
        mount_root: root.join("../mount"),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    };
    let mut rows: Vec<_> = ArchiveScanner::new(&config)
        .scan_archives_with_summary()
        .unwrap()
        .archives
        .into_iter()
        .map(|a| {
            (
                a.path.file_name().unwrap().to_string_lossy().into_owned(),
                a.identity.platform,
                a.identity.platform_provenance,
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

#[test]
fn a_valid_header_assigns_the_platform_in_a_neutral_folder() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("incoming");
    std::fs::create_dir(&root).unwrap();
    for (name, bytes) in [
        ("mono.gb", rom(0x00, true, true, 0x8000)),
        ("colour_only.gb", rom(0xC0, true, true, 0x8000)),
        ("colour.gbc", rom(0xC0, true, true, 0x8000)),
        ("dual.gbc", rom(0x80, true, true, 0x8000)),
        // Ambiguous: a dual-mode ROM named .gb, a monochrome ROM named .gbc.
        ("dual.gb", rom(0x80, true, true, 0x8000)),
        ("mono.gbc", rom(0x00, true, true, 0x8000)),
        // Invalid or short: never guessed from the extension.
        ("badlogo.gb", rom(0x00, false, true, 0x8000)),
        ("badsum.gb", rom(0x00, true, false, 0x8000)),
        ("short.gb", rom(0x00, true, true, 0x4000)),
        ("tiny.gb", vec![0u8; 16]),
    ] {
        std::fs::write(root.join(name), bytes).unwrap();
    }
    let rows = scan(&root);
    let platform = |name: &str| {
        rows.iter()
            .find(|row| row.0 == name)
            .map(|row| (row.1.as_deref(), row.2))
    };
    let header = Some(PlatformProvenance::HeaderIdentity);
    assert_eq!(platform("mono.gb"), Some((Some("Game Boy"), header)));
    assert_eq!(
        platform("colour_only.gb"),
        Some((Some("Game Boy Color"), header))
    );
    assert_eq!(
        platform("colour.gbc"),
        Some((Some("Game Boy Color"), header))
    );
    assert_eq!(platform("dual.gbc"), Some((Some("Game Boy Color"), header)));
    for unknown in ["dual.gb", "mono.gbc", "badlogo.gb", "badsum.gb", "short.gb"] {
        assert_eq!(platform(unknown).map(|p| p.0), Some(None), "{unknown}");
    }
}

#[test]
fn a_folder_name_still_outranks_the_header() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("incoming");
    let folder = root.join("Game Boy Color");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("mono.gb"), rom(0x00, true, true, 0x8000)).unwrap();
    let rows = scan(&root);
    let row = rows.iter().find(|row| row.0 == "mono.gb").unwrap();
    assert_eq!(row.1.as_deref(), Some("Game Boy Color"));
    assert_eq!(row.2, Some(PlatformProvenance::FolderAlias));
}

fn persisted_lab() -> (tempfile::TempDir, Config, archivefs_core::Database) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("incoming");
    std::fs::create_dir(&source).unwrap();
    let config = Config {
        source_folders: vec![source],
        mount_root: dir.path().join("mount"),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    };
    let database =
        archivefs_core::Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
    (dir, config, database)
}

#[test]
fn persisted_and_cached_scans_do_not_confirm_invalid_or_ambiguous_headers() {
    let (_dir, config, mut database) = persisted_lab();
    let source = &config.source_folders[0];
    for (name, bytes) in [
        ("mono.GB", rom(0, true, true, 0x8000)),
        ("colour.GBC", rom(0xC0, true, true, 0x8000)),
        ("dual.gb", rom(0x80, true, true, 0x8000)),
        ("mono.gbc", rom(0, true, true, 0x8000)),
        ("badlogo.gb", rom(0, false, true, 0x8000)),
        ("badsum.gbc", rom(0xC0, true, false, 0x8000)),
        ("short.gb", rom(0, true, true, 0x4000)),
        ("tiny.gb", vec![0; 16]),
    ] {
        std::fs::write(source.join(name), bytes).unwrap();
    }
    let outside = _dir.path().join("outside.dat");
    std::fs::write(&outside, rom(0, true, true, 0x8000)).unwrap();
    std::os::unix::fs::symlink(&outside, source.join("linked.gb")).unwrap();
    for reason in ["initial", "cached-rescan"] {
        archivefs_core::scan_and_persist(&mut database, &config, reason).unwrap();
        let rows = database.load_archives().unwrap();
        for (name, expected) in [("mono.GB", "Game Boy"), ("colour.GBC", "Game Boy Color")] {
            let row = rows
                .iter()
                .find(|r| r.absolute_path.ends_with(name))
                .unwrap();
            assert_eq!(row.platform.as_deref(), Some(expected));
            assert_eq!(row.platform_source.as_deref(), Some("header_identity"));
        }
        for name in [
            "dual.gb",
            "mono.gbc",
            "badlogo.gb",
            "badsum.gbc",
            "short.gb",
            "tiny.gb",
        ] {
            if let Some(row) = rows.iter().find(|r| r.absolute_path.ends_with(name)) {
                assert_eq!(row.platform, None, "{reason}: {name}");
                assert_eq!(row.platform_source, None, "{reason}: {name}");
            }
        }
        assert!(!rows.iter().any(|r| r.absolute_path.ends_with("linked.gb")));
    }
}

#[test]
fn source_assignment_keeps_existing_header_and_manual_precedence() {
    let (_dir, config, mut database) = persisted_lab();
    let source = &config.source_folders[0];
    std::fs::write(source.join("mono.gb"), rom(0, true, true, 0x8000)).unwrap();
    database
        .register_source_folders(std::slice::from_ref(source))
        .unwrap();
    database
        .set_source_platform_assignment(source, Some("Game Boy Color"))
        .unwrap();
    for reason in ["assigned-first-scan", "assigned-cached-rescan"] {
        archivefs_core::scan_and_persist(&mut database, &config, reason).unwrap();
        let row = &database.load_archives().unwrap()[0];
        assert_eq!(row.platform.as_deref(), Some("Game Boy"));
        assert_eq!(row.platform_source.as_deref(), Some("header_identity"));
    }
    let id = database.load_archives().unwrap()[0].id;
    database.set_manual_platform(id, "Game Boy Color").unwrap();
    archivefs_core::scan_and_persist(&mut database, &config, "manual-cached-rescan").unwrap();
    let row = &database.load_archives().unwrap()[0];
    assert_eq!(row.platform.as_deref(), Some("Game Boy Color"));
    assert_eq!(row.platform_source.as_deref(), Some("manual"));
    database.clear_manual_platform(id).unwrap();
    let row = &database.load_archives().unwrap()[0];
    assert_eq!(row.platform.as_deref(), Some("Game Boy"));
    assert_eq!(row.platform_source.as_deref(), Some("header_identity"));
}

#[test]
fn ambiguous_header_still_uses_a_deliberate_compatible_source_assignment() {
    let (_dir, config, mut database) = persisted_lab();
    let source = &config.source_folders[0];
    std::fs::write(source.join("dual.gb"), rom(0x80, true, true, 0x8000)).unwrap();
    archivefs_core::scan_and_persist(&mut database, &config, "unassigned").unwrap();
    assert_eq!(database.load_archives().unwrap()[0].platform, None);
    database
        .set_source_platform_assignment(source, Some("Game Boy Color"))
        .unwrap();
    for reason in ["assigned", "assigned-again"] {
        archivefs_core::scan_and_persist(&mut database, &config, reason).unwrap();
        let row = &database.load_archives().unwrap()[0];
        assert_eq!(row.platform.as_deref(), Some("Game Boy Color"));
        assert_eq!(row.platform_source.as_deref(), Some("source_assignment"));
    }
}

#[test]
fn a_pre_header_fix_unknown_fingerprint_is_reinspected_without_changing_the_rom() {
    let (dir, config, mut database) = persisted_lab();
    let path = config.source_folders[0].join("mono.gb");
    let bytes = rom(0, true, true, 0x8000);
    std::fs::write(&path, &bytes).unwrap();
    archivefs_core::scan_and_persist(&mut database, &config, "seed").unwrap();
    // Recreate only this fixture's old scanner state: identical media/stat,
    // Unknown platform, and the last scanner generation before GB headers.
    let fixture = rusqlite::Connection::open(dir.path().join("library.sqlite3")).unwrap();
    fixture
        .execute("UPDATE platform_assignments SET is_current=0", [])
        .unwrap();
    fixture.execute("UPDATE scan_fingerprints SET platform=NULL, platform_provenance=NULL, scanner_version='scan-v2-direct-media'", []).unwrap();
    drop(fixture);
    assert_eq!(database.load_archives().unwrap()[0].platform, None);
    for reason in ["upgrade-rescan", "current-cached-rescan"] {
        archivefs_core::scan_and_persist(&mut database, &config, reason).unwrap();
        let row = &database.load_archives().unwrap()[0];
        assert_eq!(row.platform.as_deref(), Some("Game Boy"), "{reason}");
        assert_eq!(row.platform_source.as_deref(), Some("header_identity"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
