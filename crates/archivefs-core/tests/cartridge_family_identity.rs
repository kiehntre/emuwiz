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
                    && e.diagnostic.contains("cross-platform mismatch")
                    && e.diagnostic.contains("rename the file")
                    && e.diagnostic.contains("EmuWiz does not guess")),
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

const KIB: usize = 1024;
const MIB: usize = 1024 * 1024;

fn accepted_loose(dir: &Path, id: &str, ext: &str, len: usize) -> bool {
    let path = write(dir, &format!("B.{ext}"), &rom(len));
    let report = inspect_catalogued_game_identity(&path, Some(id));
    report.complete && report.verified_loose_rom_sha256().is_some()
}

#[test]
fn size_boundaries_are_exact() {
    let dir = scratch();
    let d = dir.path();
    // GG and SMS: 1 KiB..=4 MiB in 512-byte steps (copier header = +512).
    for (id, ext) in [("GameGear", "gg"), ("MasterSystem", "sms")] {
        assert!(accepted_loose(d, id, ext, KIB), "{id} min");
        assert!(
            accepted_loose(d, id, ext, 32 * KIB + 512),
            "{id} copier header"
        );
        assert!(accepted_loose(d, id, ext, 4 * MIB), "{id} max");
        assert!(!accepted_loose(d, id, ext, 512), "{id} below min");
        assert!(!accepted_loose(d, id, ext, 4 * MIB + 512), "{id} above max");
        assert!(
            !accepted_loose(d, id, ext, 32 * KIB + 1),
            "{id} not a 512 step"
        );
    }
    // 32X: 512 B..=4 MiB in 512-byte steps.
    assert!(accepted_loose(d, "Sega 32X", "32x", 512));
    assert!(accepted_loose(d, "Sega 32X", "32x", 3 * MIB + 512));
    assert!(accepted_loose(d, "Sega 32X", "32x", 4 * MIB));
    assert!(!accepted_loose(d, "Sega 32X", "32x", 256));
    assert!(!accepted_loose(d, "Sega 32X", "32x", 4 * MIB + 512));
    assert!(!accepted_loose(d, "Sega 32X", "32x", 3_519_728));
    // HuCard: multiple of 8 KiB (+512 header), 8 KiB..=3 MiB (+512).
    assert!(accepted_loose(d, "TurboGrafx-16", "pce", 8 * KIB));
    assert!(accepted_loose(d, "TurboGrafx-16", "pce", 3 * MIB));
    assert!(accepted_loose(d, "TurboGrafx-16", "pce", 256 * KIB + 512));
    assert!(!accepted_loose(d, "TurboGrafx-16", "pce", 4 * KIB));
    assert!(!accepted_loose(
        d,
        "TurboGrafx-16",
        "pce",
        3 * MIB + 8 * KIB
    ));
    assert!(!accepted_loose(d, "TurboGrafx-16", "pce", 256 * KIB + 1));
    // ColecoVision: 1 KiB..=1 MiB.
    assert!(accepted_loose(d, "ColecoVision", "col", KIB));
    assert!(accepted_loose(d, "ColecoVision", "col", MIB));
    assert!(!accepted_loose(d, "ColecoVision", "col", KIB - 1));
    assert!(!accepted_loose(d, "ColecoVision", "col", MIB + 1));
}

#[test]
fn zipped_member_with_a_refused_size_is_refused_not_verified() {
    let dir = scratch();
    // Same size as a real 32X zip that is not a 512-byte multiple. Poorly
    // compressible bytes keep the zip realistic (no ratio-limit refusal).
    let mut state = 0x2545_F491_u32;
    let noisy = |len: usize, state: &mut u32| -> Vec<u8> {
        (0..len)
            .map(|_| {
                *state ^= *state << 13;
                *state ^= *state >> 17;
                *state ^= *state << 5;
                *state as u8
            })
            .collect()
    };
    let odd = noisy(3_519_728, &mut state);
    let zip = zip_of(&[("Odd.32x", &odd)]);
    let path = write(dir.path(), "Odd.zip", &zip);
    let report = inspect_catalogued_game_identity(&path, Some("Sega 32X"));
    assert_eq!(report.verified_value(IdentityKind::LooseRomSha256), None);
    assert!(!report.complete);
    assert!(
        report
            .evidence
            .iter()
            .any(|e| e.status == IdentityStatus::Invalid
                && e.diagnostic.contains("32X ROM is 3519728 bytes")),
        "{:#?}",
        report.evidence
    );
    // The same member padded to a 512-byte step is accepted.
    let ok = noisy(3_520_000, &mut state);
    let zip = zip_of(&[("Ok.32x", &ok)]);
    let path = write(dir.path(), "Ok.zip", &zip);
    let report = inspect_catalogued_game_identity(&path, Some("Sega 32X"));
    assert_eq!(
        report.verified_value(IdentityKind::LooseRomSha256),
        Some(sha(&ok).as_str())
    );
}

#[test]
fn disc_files_in_a_turbografx16_folder_are_never_verified_and_stay_incomplete_like_an_unknown_platform()
 {
    // Before this change TurboGrafx-16 resolved to `Other`; the new variant
    // must not make a .cue/.chd/.iso verify or change its outcome.
    let dir = scratch();
    for (name, bytes) in [
        (
            "Disc.cue",
            b"FILE \"Disc.bin\" BINARY\r\n  TRACK 01 MODE1/2352\r\n    INDEX 01 00:00:00\r\n"
                .to_vec(),
        ),
        ("Disc.chd", rom(4 * KIB)),
        ("Disc.iso", rom(64 * KIB)),
    ] {
        let path = write(dir.path(), name, &bytes);
        let tg = inspect_catalogued_game_identity(&path, Some("TurboGrafx-16"));
        let other = inspect_catalogued_game_identity(&path, Some("Nonsense Platform"));
        // Both routes refuse: nothing but the platform fact is Verified and
        // the report is incomplete. (TurboGrafx-16 now reports the cartridge
        // refusal kinds instead of the generic unsupported-platform ones.)
        for report in [&tg, &other] {
            assert_eq!(report.verified_loose_rom_sha256(), None, "{name}");
            assert!(!report.complete, "{name}");
            assert!(
                report
                    .evidence
                    .iter()
                    .all(|e| e.kind == IdentityKind::Platform
                        || e.status != IdentityStatus::Verified),
                "{name}"
            );
        }
    }
}
