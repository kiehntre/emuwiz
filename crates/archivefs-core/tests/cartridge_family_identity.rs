//! Synthetic fixtures only (no real ROM or library is read): Game Gear,
//! Master System, Sega 32X, ColecoVision and TurboGrafx-16 (HuCard) loose
//! and zipped single-ROM identity through the shared loose-ROM design.

archivefs_core::install_test_environment!();

use std::io::Write;
use std::path::{Path, PathBuf};

use archivefs_core::game_identity::{
    IdentityKind, IdentityPlatform, IdentityStatus, inspect_catalogued_game_identity,
    supported_loose_rom_format,
};
use archivefs_core::launch::evidence_bridge::canonical_identity_from_game_report;
use archivefs_core::launch::planning::CanonicalIdentityStatus;
use archivefs_core::zip_cartridge_select::is_loose_cartridge_platform;

/// (stored platform id, platform, extension, valid size)
const FAMILY: &[(&str, IdentityPlatform, &str, usize)] = &[
    ("GameGear", IdentityPlatform::GameGear, "gg", 32 * 1024),
    (
        "MasterSystem",
        IdentityPlatform::MasterSystem,
        "sms",
        32 * 1024,
    ),
    ("Sega 32X", IdentityPlatform::Sega32x, "32x", 64 * 1024),
    (
        "ColecoVision",
        IdentityPlatform::ColecoVision,
        "col",
        8 * 1024,
    ),
    (
        "TurboGrafx-16",
        IdentityPlatform::TurboGrafx16,
        "pce",
        128 * 1024,
    ),
];

fn rom(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn scratch() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn loose_happy_path_resolves_identity_and_a_launch_platform_id() {
    let dir = scratch();
    for (id, platform, ext, len) in FAMILY {
        let bytes = rom(*len);
        let path = write(dir.path(), &format!("Game.{ext}"), &bytes);
        let report = inspect_catalogued_game_identity(&path, Some(id));
        assert_eq!(report.platform, *platform, "{id}");
        assert_eq!(
            report.verified_loose_rom_sha256(),
            Some(sha(&bytes).as_str()),
            "{id}"
        );
        assert!(report.complete, "{id}");
        // Honest trust text: exact bytes, never a known-good dump claim.
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.diagnostic.contains("not a known-good dump claim"))
        );
        let (status, _) = canonical_identity_from_game_report(&report);
        match status {
            CanonicalIdentityStatus::Resolved(resolved) => {
                assert_eq!(resolved.platform_id, *id);
                assert_eq!(resolved.game_key, sha(&bytes));
            }
            other => panic!("{id}: {other:?}"),
        }
    }
}

#[test]
fn zipped_single_rom_member_happy_path() {
    let dir = scratch();
    for (id, platform, ext, len) in FAMILY {
        let bytes = rom(*len);
        let zip = zip_of(&[(&format!("Game.{ext}"), &bytes), ("notes.txt", b"hello")]);
        let path = write(dir.path(), "Game.zip", &zip);
        let report = inspect_catalogued_game_identity(&path, Some(id));
        assert_eq!(report.platform, *platform, "{id}");
        // The member's hash, never the container's.
        assert_eq!(
            report.verified_loose_rom_sha256(),
            None,
            "{id}: container is not the ROM"
        );
        assert_eq!(
            report.verified_value(IdentityKind::LooseRomSha256),
            Some(sha(&bytes).as_str()),
            "{id}"
        );
        assert!(report.complete, "{id}");
        assert!(is_loose_cartridge_platform(*platform));
    }
}

#[test]
fn multiple_candidate_members_are_ambiguous_never_picked() {
    let dir = scratch();
    for (id, _, ext, len) in FAMILY {
        let bytes = rom(*len);
        let zip = zip_of(&[(&format!("A.{ext}"), &bytes), (&format!("B.{ext}"), &bytes)]);
        let path = write(dir.path(), "Two.zip", &zip);
        let report = inspect_catalogued_game_identity(&path, Some(id));
        assert_eq!(report.verified_loose_rom_sha256(), None, "{id}");
        assert!(!report.complete, "{id}");
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.status == IdentityStatus::Ambiguous),
            "{id}"
        );
    }
}

#[test]
fn unsupported_extension_is_refused() {
    let dir = scratch();
    for (id, _, _, len) in FAMILY {
        let path = write(dir.path(), "Game.bin", &rom(*len));
        let report = inspect_catalogued_game_identity(&path, Some(id));
        assert_eq!(report.verified_loose_rom_sha256(), None, "{id}");
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.status == IdentityStatus::Unsupported),
            "{id}"
        );
    }
}

#[test]
fn sms_under_game_gear_and_gg_under_master_system_are_refused_as_mismatches() {
    let dir = scratch();
    for (id, file) in [("GameGear", "Game.sms"), ("MasterSystem", "Game.gg")] {
        let path = write(dir.path(), file, &rom(32 * 1024));
        let report = inspect_catalogued_game_identity(&path, Some(id));
        assert_eq!(report.verified_loose_rom_sha256(), None, "{id}");
        assert!(!report.complete);
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.status == IdentityStatus::Invalid
                    && e.diagnostic.contains("cross-platform mismatch")),
            "{id}"
        );
        let zip = zip_of(&[(file, &rom(32 * 1024))]);
        let zpath = write(dir.path(), "Wrong.zip", &zip);
        let zreport = inspect_catalogued_game_identity(&zpath, Some(id));
        assert_eq!(zreport.verified_loose_rom_sha256(), None, "{id} zip");
        assert!(
            zreport
                .evidence
                .iter()
                .any(|e| e.diagnostic.contains("cross-platform mismatch")),
            "{id} zip"
        );
    }
}

#[test]
fn obviously_wrong_sizes_are_refused() {
    let dir = scratch();
    for (id, platform, ext, len) in FAMILY {
        // ColecoVision has no size-multiple rule, only 1 KiB..1 MiB.
        let odd = if *platform == IdentityPlatform::ColecoVision {
            2_000_000
        } else {
            *len + 3
        };
        for bad in [0_usize, 100, odd] {
            let path = write(dir.path(), &format!("Bad.{ext}"), &rom(bad));
            let report = inspect_catalogued_game_identity(&path, Some(id));
            assert_eq!(report.verified_loose_rom_sha256(), None, "{id} {bad}");
            assert!(
                report
                    .evidence
                    .iter()
                    .any(|e| e.status == IdentityStatus::Invalid),
                "{id} {bad}"
            );
        }
    }
}

#[test]
fn copier_header_and_optional_magics_are_supporting_only() {
    let dir = scratch();
    // PCE: 512-byte copier header accepted, no magic needed.
    let pce = rom(128 * 1024 + 512);
    let p = write(dir.path(), "Hdr.pce", &pce);
    assert!(inspect_catalogued_game_identity(&p, Some("TurboGrafx-16")).complete);
    // 32X with the SEGA 32X marker: supporting text, same verification path.
    let mut x32 = rom(64 * 1024);
    x32[0x100..0x108].copy_from_slice(b"SEGA 32X");
    let p = write(dir.path(), "Marked.32x", &x32);
    let report = inspect_catalogued_game_identity(&p, Some("Sega 32X"));
    assert!(report.complete);
    assert!(
        report
            .evidence
            .iter()
            .any(|e| e.diagnostic.contains("SEGA 32X header at 0x100 present"))
    );
    // Coleco marker.
    let mut col = rom(8 * 1024);
    col[..2].copy_from_slice(&[0xAA, 0x55]);
    let p = write(dir.path(), "Marked.col", &col);
    let report = inspect_catalogued_game_identity(&p, Some("ColecoVision"));
    assert!(
        report
            .evidence
            .iter()
            .any(|e| e.diagnostic.contains("start marker present"))
    );
    // GG without any TMR SEGA header is still accepted (trust = size + ext + folder).
    let p = write(dir.path(), "Plain.gg", &rom(32 * 1024));
    let report = inspect_catalogued_game_identity(&p, Some("GameGear"));
    assert!(report.complete);
    assert!(
        report
            .evidence
            .iter()
            .any(|e| e.diagnostic.contains("trusted folder platform"))
    );
}

#[test]
fn untrusted_platform_never_verifies() {
    let dir = scratch();
    let path = write(dir.path(), "Game.gg", &rom(32 * 1024));
    let report = archivefs_core::game_identity::inspect_game_identity(&path, Some("GameGear"));
    assert_eq!(report.verified_loose_rom_sha256(), None);
}

#[test]
fn aliases_resolve_and_existing_platforms_are_unaffected() {
    for (alias, expected) in [
        ("game gear", IdentityPlatform::GameGear),
        ("GameGear", IdentityPlatform::GameGear),
        ("Sega Game Gear", IdentityPlatform::GameGear),
        ("Master System", IdentityPlatform::MasterSystem),
        ("MasterSystem", IdentityPlatform::MasterSystem),
        ("sms", IdentityPlatform::MasterSystem),
        ("32x", IdentityPlatform::Sega32x),
        ("Sega 32X", IdentityPlatform::Sega32x),
        ("ColecoVision", IdentityPlatform::ColecoVision),
        ("coleco", IdentityPlatform::ColecoVision),
        ("TurboGrafx-16", IdentityPlatform::TurboGrafx16),
        ("tg16", IdentityPlatform::TurboGrafx16),
        ("PC Engine", IdentityPlatform::TurboGrafx16),
        ("pcengine", IdentityPlatform::TurboGrafx16),
        ("PC Engine CD", IdentityPlatform::PcEngineCd),
        ("TurboGrafx-CD", IdentityPlatform::PcEngineCd),
        ("Sega CD", IdentityPlatform::SegaCd),
        ("MegaDrive", IdentityPlatform::MegaDrive),
        ("Atari 8-bit", IdentityPlatform::Atari8Bit),
        ("Atari 2600", IdentityPlatform::Atari2600),
        ("Nonsense", IdentityPlatform::Other),
    ] {
        assert_eq!(
            IdentityPlatform::from_catalogue(Some(alias)),
            expected,
            "{alias}"
        );
    }
    // Atari 2600 .bin and MegaDrive formats are not widened by this change.
    assert_eq!(
        supported_loose_rom_format(Path::new("x.bin"), IdentityPlatform::Atari2600),
        None
    );
    assert_eq!(
        supported_loose_rom_format(Path::new("x.md"), IdentityPlatform::MegaDrive),
        Some("md")
    );
    // Wrong-system extensions and unreviewed ones stay unsupported.
    assert_eq!(
        supported_loose_rom_format(Path::new("x.sms"), IdentityPlatform::GameGear),
        None
    );
    assert_eq!(
        supported_loose_rom_format(Path::new("x.sg"), IdentityPlatform::MasterSystem),
        None
    );
    assert_eq!(
        supported_loose_rom_format(Path::new("x.bin"), IdentityPlatform::ColecoVision),
        None
    );
}

#[test]
fn registry_ids_round_trip_through_identity_and_back() {
    use archivefs_core::platform::platform_by_id;
    for (id, platform, _, _) in FAMILY {
        assert!(
            platform_by_id(id).is_some(),
            "{id} must be a real Platform::id"
        );
        assert_eq!(IdentityPlatform::from_catalogue(Some(id)), *platform);
    }
}
