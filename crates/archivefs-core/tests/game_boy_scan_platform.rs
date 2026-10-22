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
