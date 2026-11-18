use super::*;
use std::io::Write;

fn rom(seed: u8, size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn write_zip(path: &Path, members: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn only_a_bin_directly_in_a_supported_system_folder_is_a_candidate() {
    for (path, expected) in [
        ("/g/atari2600/Amidar (USA).bin", Some("Atari2600")),
        ("/g/atari800/Desert Falcon (USA).bin", Some("Atari 8-bit")),
        (
            "/g/atari-xegs/Barnyard Blaster (USA).bin",
            Some("Atari 8-bit"),
        ),
        (
            "/g/amstrad-gx4000/Klax (1990)(Domark).bin",
            Some("AmstradGX4000"),
        ),
        ("/g/loopy/Anime Land (Japan)_1.bin", Some("CasioLoopy")),
        ("/g/videopac/Chess (Europe).bin", Some("Videopac")),
        ("/g/atari2600/Game.BIN", Some("Atari2600")),
        // Nested game data, arcade chips and other systems stay out.
        ("/g/atari2600/extras/data.bin", None),
        ("/g/arcade/pacman/pacman.bin", None),
        ("/g/ps3/GAME/USRDIR/data.bin", None),
        ("/g/megadrive/Sonic.bin", None),
        ("/g/atari2600/Amidar.zip", None),
    ] {
        assert_eq!(candidate_platform(Path::new(path)), expected, "{path}");
    }
}

#[test]
fn platform_specific_size_and_header_checks() {
    let cart = b"CART\0\0\0\x0e";
    for (platform, size, header, expected) in [
        ("Atari2600", 2048, &b""[..], true),
        ("Atari2600", 4096, b"", true),
        ("Atari2600", 33792, b"", true), // Supercharger, 4 loads
        ("Atari2600", 3000, b"", false),
        ("Atari2600", 1024 * 1024, b"", false),
        ("Atari 8-bit", 8192, b"", true),
        ("Atari 8-bit", 65552, cart, true),
        ("Atari 8-bit", 32784, cart, true),
        ("Atari 8-bit", 65552, b"\0\0\0\0", false), // 16 extra bytes, no header
        ("Atari 8-bit", 4096, b"", false),
        ("AmstradGX4000", 131072, b"", true),
        ("AmstradGX4000", 100_000, b"", false),
        ("AmstradGX4000", 1024 * 1024, b"", false),
        ("CasioLoopy", 2 * 1024 * 1024, b"", true),
        ("CasioLoopy", 3 * 1024 * 1024, b"", true),
        ("CasioLoopy", 4096, b"", false),
        ("Videopac", 2048, b"", true),
        ("Videopac", 8192, b"", true),
        ("Videopac", 3072, b"", false),
        ("Videopac", 32768, b"", false),
        ("MegaDrive", 4096, b"", false),
    ] {
        assert_eq!(
            plausible_dump(platform, size, header),
            expected,
            "{platform} {size}"
        );
    }
}

#[test]
fn identical_revision_misleading_name_and_multi_file_zip_cases() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("atari2600");
    fs::create_dir_all(&folder).unwrap();
    let amidar = rom(1, 4096);
    let river_raid = rom(2, 4096);
    let rev0 = rom(3, 8192);
    let rev1 = rom(4, 8192);
    let combat = rom(5, 2048);
    write_zip(&folder.join("Amidar.zip"), &[("Amidar (USA).bin", &amidar)]);
    write_zip(
        &folder.join("River Raid.zip"),
        &[("River Raid (USA).bin", &river_raid)],
    );
    write_zip(
        &folder.join("Game (Rev 1).zip"),
        &[("Game (Rev 1).bin", &rev1)],
    );
    write_zip(
        &folder.join("Two Games.zip"),
        &[("Combat (USA).bin", &combat), ("Other.bin", &rom(6, 2048))],
    );

    // Identical ROM under a different (longer) name: an alternate copy.
    let loose = folder.join("Amidar (USA).bin");
    fs::write(&loose, &amidar).unwrap();
    assert_eq!(
        classify(&loose),
        LooseCartridge::IdenticalCopy {
            platform: "Atari2600",
            zip: folder.join("Amidar.zip"),
        }
    );
    // Content decides, not the name: a misleadingly named file holding River
    // Raid's bytes is River Raid's copy...
    let misnamed = folder.join("Pitfall (USA).bin");
    fs::write(&misnamed, &river_raid).unwrap();
    assert!(matches!(
        classify(&misnamed),
        LooseCartridge::IdenticalCopy { zip, .. } if zip == folder.join("River Raid.zip")
    ));
    // ...and a file named like a ZIP's ROM with other bytes is not.
    let lookalike = folder.join("River Raid (USA).bin");
    fs::write(&lookalike, rom(7, 4096)).unwrap();
    assert_eq!(
        classify(&lookalike),
        LooseCartridge::PossibleVariant {
            platform: "Atari2600",
            counterpart: folder.join("River Raid.zip"),
        }
    );
    // A different revision under the same name: both kept, flagged.
    let revision = folder.join("Game (Rev 1).bin");
    fs::write(&revision, &rev0).unwrap();
    assert!(matches!(
        classify(&revision),
        LooseCartridge::PossibleVariant { .. }
    ));
    // Matching one file of a multi-file ZIP is not "the same game".
    let from_multi = folder.join("Combat (USA).bin");
    fs::write(&from_multi, &combat).unwrap();
    assert!(
        classify(&from_multi).is_catalogued(),
        "{:?}",
        classify(&from_multi)
    );
    // No counterpart at all: new.
    let fresh = folder.join("Homebrew.bin");
    fs::write(&fresh, rom(8, 4096)).unwrap();
    assert_eq!(
        classify(&fresh),
        LooseCartridge::New {
            platform: "Atari2600"
        }
    );
    // A size no 2600 cartridge has: not a cartridge.
    let odd = folder.join("notes.bin");
    fs::write(&odd, b"not a rom").unwrap();
    assert_eq!(classify(&odd), LooseCartridge::NotCartridge);
    // Nothing was renamed, moved or deleted.
    assert_eq!(fs::read(&loose).unwrap(), amidar);
    assert!(folder.join("Amidar.zip").is_file());
}

#[test]
fn a_zip_whose_member_matches_size_and_crc_but_not_bytes_is_not_identical() {
    // The index's CRC is trusted only to find a candidate; the bytes decide.
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("videopac");
    fs::create_dir_all(&folder).unwrap();
    let bytes = rom(9, 2048);
    write_zip(&folder.join("Chess.zip"), &[("Chess.bin", &bytes)]);
    let loose = folder.join("Chess (Europe).bin");
    fs::write(&loose, &bytes).unwrap();
    assert!(matches!(
        classify(&loose),
        LooseCartridge::IdenticalCopy { .. }
    ));
    // Replace the ZIP in place without touching the folder: the cached index
    // still claims a match, the byte comparison refuses it.
    let mut other = bytes.clone();
    other[0] ^= 0xff;
    write_zip(&folder.join("Chess.zip"), &[("Chess.bin", &other)]);
    assert!(classify(&loose).is_catalogued(), "{:?}", classify(&loose));
}

#[test]
fn cached_ambiguous_verdicts_of_candidates_are_never_reused() {
    assert!(!crate::non_archive_verdict_is_still_current(
        Path::new("/g/atari2600/Amidar (USA).bin"),
        "ambiguous"
    ));
    assert!(crate::non_archive_verdict_is_still_current(
        Path::new("/g/scummvm/game/RESOURCE.bin"),
        "ambiguous"
    ));
    assert!(!crate::non_archive_verdict_is_still_current(
        Path::new("/g/atari2600/Amidar (USA).bin"),
        "identical_copy"
    ));
}

#[test]
fn title_key_ignores_tags_case_and_punctuation() {
    assert_eq!(title_key("Asterix (PAL).bin").as_deref(), Some("asterix"));
    assert_eq!(title_key("Asterix.zip"), title_key("ASTERIX [!].bin"));
    assert_eq!(
        title_key("G.I. Joe - Cobra Strike (USA).bin").as_deref(),
        Some("gijoecobrastrike")
    );
    assert_eq!(
        title_key("sub/River Raid (USA).bin").as_deref(),
        Some("riverraid")
    );
    assert_eq!(title_key("(USA).bin"), None);
}

#[test]
fn a_region_tagged_name_with_other_bytes_is_a_possible_variant() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("atari2600");
    fs::create_dir_all(&folder).unwrap();
    write_zip(
        &folder.join("Asterix.zip"),
        &[("Asterix.bin", &rom(10, 8192))],
    );
    let loose = folder.join("Asterix (PAL).bin");
    fs::write(&loose, rom(11, 8192)).unwrap();
    assert_eq!(
        classify(&loose),
        LooseCartridge::PossibleVariant {
            platform: "Atari2600",
            counterpart: folder.join("Asterix.zip"),
        }
    );
}
