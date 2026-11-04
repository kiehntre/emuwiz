use super::*;
use crate::dat::model::{DatEcosystem, DatFormat, DatGameEntry, DatRomEntry, DatSource};

fn dat_with_roms(roms: Vec<DatRomEntry>) -> ParsedDat {
    ParsedDat {
        source: DatSource {
            format: DatFormat::Logiqx,
            ecosystem: DatEcosystem::NoIntro,
            file_path: "synthetic.dat".to_string(),
            name: Some("Synthetic".to_string()),
            description: None,
            version: None,
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: 1,
            rom_count: roms.len(),
            parse_warnings: Vec::new(),
            packing_policy: crate::dat::model::DatPackingPolicy::Standard,
        },
        games: vec![DatGameEntry {
            name: "Game (Europe)".to_string(),
            roms,
            ..DatGameEntry::default()
        }],
    }
}

fn dat_with_one_rom(sha1: &str) -> ParsedDat {
    dat_with_roms(vec![DatRomEntry {
        name: "game.bin".to_string(),
        size_bytes: Some(4),
        sha1: Some(sha1.to_string()),
        ..DatRomEntry::default()
    }])
}

/// SHA-1 of `b"test"` (4 bytes).
const SHA1_TEST: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
/// SHA-1 of `b"abc"` (3 bytes).
const SHA1_ABC: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";
/// SHA-1 of `b"xyz"` (3 bytes).
const SHA1_XYZ: &str = "66b27417d37e024c46526c2f6d358a754fc552f3";

#[test]
fn a_hash_verified_file_is_matched_to_its_game_index() {
    let temp = tempfile::tempdir().expect("temp dir");
    let file = temp.path().join("loose.bin");
    std::fs::write(&file, b"test").unwrap();
    let dat = dat_with_one_rom(SHA1_TEST);

    let outcome = match_loose_files_against_dat(
        &dat,
        std::slice::from_ref(&file),
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1);
    assert_eq!(outcome.matches[0].archive_path, file);
    assert_eq!(outcome.matches[0].dat_entry_index, 0);
    assert!(outcome.matches[0].companion_paths.is_empty());
    assert!(outcome.rejected_launchers.is_empty());
}

#[test]
fn an_unmatched_file_is_silently_dropped() {
    let temp = tempfile::tempdir().expect("temp dir");
    let file = temp.path().join("unknown.bin");
    std::fs::write(&file, b"not in the catalogue").unwrap();
    let dat = dat_with_one_rom(SHA1_TEST);

    let outcome = match_loose_files_against_dat(
        &dat,
        &[file],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert!(outcome.rejected_launchers.is_empty());
}

fn write_cue(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn a_cue_with_one_matched_bin_becomes_one_combined_match() {
    let temp = tempfile::tempdir().expect("temp dir");
    let bin = temp.path().join("game.bin");
    std::fs::write(&bin, b"test").unwrap();
    let cue = write_cue(temp.path(), "game.cue", "FILE \"game.bin\" BINARY\n");
    let dat = dat_with_one_rom(SHA1_TEST);

    let outcome = match_loose_files_against_dat(
        &dat,
        &[bin.clone(), cue.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1, "{:?}", outcome.matches);
    assert_eq!(outcome.matches[0].archive_path, cue);
    assert_eq!(outcome.matches[0].dat_entry_index, 0);
    assert_eq!(outcome.matches[0].companion_paths, vec![bin]);
    assert!(outcome.rejected_launchers.is_empty());
}

#[test]
fn a_cue_with_multiple_matched_tracks_combines_them_all() {
    let temp = tempfile::tempdir().expect("temp dir");
    let bin1 = temp.path().join("track1.bin");
    let bin2 = temp.path().join("track2.bin");
    std::fs::write(&bin1, b"test").unwrap();
    std::fs::write(&bin2, b"abc").unwrap();
    let cue = write_cue(
        temp.path(),
        "game.cue",
        "FILE \"track1.bin\" BINARY\nFILE \"track2.bin\" BINARY\n",
    );
    let dat = dat_with_roms(vec![
        DatRomEntry {
            name: "track1.bin".to_string(),
            size_bytes: Some(4),
            sha1: Some(SHA1_TEST.to_string()),
            ..DatRomEntry::default()
        },
        DatRomEntry {
            name: "track2.bin".to_string(),
            size_bytes: Some(3),
            sha1: Some(SHA1_ABC.to_string()),
            ..DatRomEntry::default()
        },
    ]);

    let outcome = match_loose_files_against_dat(
        &dat,
        &[bin1.clone(), bin2.clone(), cue.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1, "{:?}", outcome.matches);
    assert_eq!(outcome.matches[0].archive_path, cue);
    assert_eq!(outcome.matches[0].companion_paths.len(), 2);
    assert!(outcome.matches[0].companion_paths.contains(&bin1));
    assert!(outcome.matches[0].companion_paths.contains(&bin2));
}

#[test]
fn a_cue_referencing_a_missing_track_is_rejected_with_a_plain_reason() {
    // A missing referenced track fails the whole release closed - it is
    // reported as a RejectedLauncher, not silently dropped, per the
    // "missing/escaping/ambiguous companion paths fail closed with plain
    // reasons" requirement.
    let temp = tempfile::tempdir().expect("temp dir");
    let cue = write_cue(temp.path(), "game.cue", "FILE \"missing.bin\" BINARY\n");
    let dat = dat_with_one_rom(SHA1_TEST);

    let outcome = match_loose_files_against_dat(
        &dat,
        std::slice::from_ref(&cue),
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert_eq!(outcome.rejected_launchers[0].launcher_path, cue);
    assert!(outcome.rejected_launchers[0].reason.contains("incomplete"));
}

#[test]
fn a_cue_with_one_missing_track_among_several_suppresses_the_present_track_too() {
    // The one track that DOES exist and DOES verify against the catalogue
    // must not leak out as its own standalone election just because its
    // sibling track is missing - the whole logical release fails closed.
    let temp = tempfile::tempdir().expect("temp dir");
    let bin1 = temp.path().join("track1.bin");
    std::fs::write(&bin1, b"test").unwrap();
    let cue = write_cue(
        temp.path(),
        "game.cue",
        "FILE \"track1.bin\" BINARY\nFILE \"track2.bin\" BINARY\n",
    );
    let dat = dat_with_roms(vec![
        DatRomEntry {
            name: "track1.bin".to_string(),
            size_bytes: Some(4),
            sha1: Some(SHA1_TEST.to_string()),
            ..DatRomEntry::default()
        },
        DatRomEntry {
            name: "track2.bin".to_string(),
            size_bytes: Some(3),
            sha1: Some(SHA1_ABC.to_string()),
            ..DatRomEntry::default()
        },
    ]);

    let outcome = match_loose_files_against_dat(
        &dat,
        &[bin1.clone(), cue.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert_eq!(outcome.rejected_launchers[0].launcher_path, cue);
    assert!(
        outcome
            .matches
            .iter()
            .all(|found| found.archive_path != bin1),
        "{:?}",
        outcome.matches
    );
}

#[test]
fn an_unrelated_file_sharing_a_directory_with_a_rejected_cue_is_still_matched() {
    // Suppression only ever follows structural reference, never physical
    // proximity: a file the CUE never named must remain eligible even
    // though it sits right next to a rejected launcher.
    let temp = tempfile::tempdir().expect("temp dir");
    let bin1 = temp.path().join("track1.bin");
    std::fs::write(&bin1, b"test").unwrap();
    let unrelated = temp.path().join("standalone.chd");
    std::fs::write(&unrelated, b"xyz").unwrap();
    let cue = write_cue(
        temp.path(),
        "game.cue",
        "FILE \"track1.bin\" BINARY\nFILE \"track2.bin\" BINARY\n",
    );
    let dat = dat_with_roms(vec![
        DatRomEntry {
            name: "track1.bin".to_string(),
            size_bytes: Some(4),
            sha1: Some(SHA1_TEST.to_string()),
            ..DatRomEntry::default()
        },
        DatRomEntry {
            name: "track2.bin".to_string(),
            size_bytes: Some(3),
            sha1: Some(SHA1_ABC.to_string()),
            ..DatRomEntry::default()
        },
        DatRomEntry {
            name: "standalone.chd".to_string(),
            size_bytes: Some(3),
            sha1: Some(SHA1_XYZ.to_string()),
            ..DatRomEntry::default()
        },
    ]);

    let outcome = match_loose_files_against_dat(
        &dat,
        &[bin1, unrelated.clone(), cue],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert!(
        outcome
            .matches
            .iter()
            .any(|found| found.archive_path == unrelated && found.dat_entry_index == 0),
        "{:?}",
        outcome.matches
    );
}

#[test]
fn a_cue_whose_tracks_verify_against_two_different_games_is_rejected() {
    let temp = tempfile::tempdir().expect("temp dir");
    let bin1 = temp.path().join("track1.bin");
    let bin2 = temp.path().join("track2.bin");
    std::fs::write(&bin1, b"test").unwrap();
    std::fs::write(&bin2, b"xyz").unwrap();
    let cue = write_cue(
        temp.path(),
        "game.cue",
        "FILE \"track1.bin\" BINARY\nFILE \"track2.bin\" BINARY\n",
    );
    let dat = ParsedDat {
        source: dat_with_one_rom(SHA1_TEST).source,
        games: vec![
            DatGameEntry {
                name: "Game One".to_string(),
                roms: vec![DatRomEntry {
                    name: "track1.bin".to_string(),
                    size_bytes: Some(4),
                    sha1: Some(SHA1_TEST.to_string()),
                    ..DatRomEntry::default()
                }],
                ..DatGameEntry::default()
            },
            DatGameEntry {
                name: "Game Two".to_string(),
                roms: vec![DatRomEntry {
                    name: "track2.bin".to_string(),
                    size_bytes: Some(3),
                    sha1: Some(SHA1_XYZ.to_string()),
                    ..DatRomEntry::default()
                }],
                ..DatGameEntry::default()
            },
        ],
    };

    let outcome = match_loose_files_against_dat(
        &dat,
        &[bin1.clone(), bin2.clone(), cue.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    // The launcher itself is rejected with a plain reason - never silently
    // combined across two different games.
    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert_eq!(outcome.rejected_launchers[0].launcher_path, cue);
    assert!(outcome.rejected_launchers[0].reason.contains("2 different"));
    // The whole logical release fails closed: neither track becomes its
    // own independent election, even though each one individually verifies
    // against a real catalogue entry. A structurally referenced companion
    // of a rejected release must never surface as a standalone game.
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
}

#[test]
fn a_gdi_with_matched_tracks_becomes_one_combined_match() {
    let temp = tempfile::tempdir().expect("temp dir");
    let track1 = temp.path().join("track01.bin");
    let track2 = temp.path().join("track02.bin");
    std::fs::write(&track1, b"test").unwrap();
    std::fs::write(&track2, b"abc").unwrap();
    let gdi = temp.path().join("game.gdi");
    std::fs::write(
        &gdi,
        "2\n1 0 4 2352 track01.bin 0\n2 45000 4 2352 track02.bin 0\n",
    )
    .unwrap();
    let dat = dat_with_roms(vec![
        DatRomEntry {
            name: "track01.bin".to_string(),
            size_bytes: Some(4),
            sha1: Some(SHA1_TEST.to_string()),
            ..DatRomEntry::default()
        },
        DatRomEntry {
            name: "track02.bin".to_string(),
            size_bytes: Some(3),
            sha1: Some(SHA1_ABC.to_string()),
            ..DatRomEntry::default()
        },
    ]);

    let outcome = match_loose_files_against_dat(
        &dat,
        &[track1.clone(), track2.clone(), gdi.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1, "{:?}", outcome.matches);
    assert_eq!(outcome.matches[0].archive_path, gdi);
    assert_eq!(outcome.matches[0].companion_paths.len(), 2);
    assert!(outcome.matches[0].companion_paths.contains(&track1));
    assert!(outcome.matches[0].companion_paths.contains(&track2));
}

#[test]
fn a_gdi_referencing_a_missing_track_is_rejected_and_the_present_track_is_suppressed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let track1 = temp.path().join("track01.bin");
    std::fs::write(&track1, b"test").unwrap();
    let gdi = temp.path().join("game.gdi");
    std::fs::write(
        &gdi,
        "2\n1 0 4 2352 track01.bin 0\n2 45000 4 2352 track02.bin 0\n",
    )
    .unwrap();
    let dat = dat_with_roms(vec![
        DatRomEntry {
            name: "track01.bin".to_string(),
            size_bytes: Some(4),
            sha1: Some(SHA1_TEST.to_string()),
            ..DatRomEntry::default()
        },
        DatRomEntry {
            name: "track02.bin".to_string(),
            size_bytes: Some(3),
            sha1: Some(SHA1_ABC.to_string()),
            ..DatRomEntry::default()
        },
    ]);

    let outcome = match_loose_files_against_dat(
        &dat,
        &[track1.clone(), gdi.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert_eq!(outcome.rejected_launchers[0].launcher_path, gdi);
    assert!(outcome.rejected_launchers[0].reason.contains("incomplete"));
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
}

/// A two-track GDI fixture: `track01.bin` holds `b"test"` and
/// `track02.bin` holds `track2`. Returns (descriptor, track01, track02).
fn gdi_release(dir: &Path, track2: &[u8]) -> (PathBuf, PathBuf, PathBuf) {
    let track1 = dir.join("track01.bin");
    let track2_path = dir.join("track02.bin");
    std::fs::write(&track1, b"test").unwrap();
    std::fs::write(&track2_path, track2).unwrap();
    let gdi = dir.join("game.gdi");
    std::fs::write(
        &gdi,
        "2\n1 0 4 2352 track01.bin 0\n2 45000 4 2352 track02.bin 0\n",
    )
    .unwrap();
    (gdi, track1, track2_path)
}

fn gdi_catalogue(descriptor_sha1: Option<String>) -> ParsedDat {
    let mut roms = vec![
        DatRomEntry {
            name: "track01.bin".to_string(),
            size_bytes: Some(4),
            sha1: Some(SHA1_TEST.to_string()),
            ..DatRomEntry::default()
        },
        DatRomEntry {
            name: "track02.bin".to_string(),
            size_bytes: Some(3),
            sha1: Some(SHA1_ABC.to_string()),
            ..DatRomEntry::default()
        },
    ];
    if let Some(sha1) = descriptor_sha1 {
        roms.insert(
            0,
            DatRomEntry {
                name: "game.gdi".to_string(),
                sha1: Some(sha1),
                ..DatRomEntry::default()
            },
        );
    }
    dat_with_roms(roms)
}

fn sha1_of(path: &Path) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(std::fs::read(path).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Regression (optical Priority 2): a track that is present but holds other
/// bytes is not verified, so the release must not combine as if it were.
#[test]
fn regression_a_gdi_with_a_changed_track_is_rejected_not_combined() {
    let temp = tempfile::tempdir().expect("temp dir");
    let (gdi, track1, track2) = gdi_release(temp.path(), b"xyz");
    let outcome = match_loose_files_against_dat(
        &gdi_catalogue(None),
        &[track1, track2, gdi.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
    assert_eq!(outcome.rejected_launchers.len(), 1, "{outcome:?}");
    assert_eq!(outcome.rejected_launchers[0].launcher_path, gdi);
    assert!(
        outcome.rejected_launchers[0].reason.contains("track02.bin"),
        "{}",
        outcome.rejected_launchers[0].reason
    );
}

/// Regression (optical Priority 2): catalogues that list the descriptor
/// itself (TOSEC lists `.gdi`, Redump lists `.cue`) make its own hash
/// match. That proves only the descriptor text, never the tracks.
#[test]
fn regression_a_matching_gdi_descriptor_with_changed_tracks_is_not_a_match() {
    let temp = tempfile::tempdir().expect("temp dir");
    let (gdi, track1, track2) = gdi_release(temp.path(), b"xyz");
    std::fs::write(&track1, b"nope").unwrap();
    let dat = gdi_catalogue(Some(sha1_of(&gdi)));
    let outcome = match_loose_files_against_dat(
        &dat,
        &[track1, track2, gdi.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
    assert_eq!(outcome.rejected_launchers.len(), 1, "{outcome:?}");
    assert_eq!(outcome.rejected_launchers[0].launcher_path, gdi);
}

/// A catalogue that lists the descriptor still combines a complete,
/// matching release: the descriptor is one more verified member.
#[test]
fn a_listed_gdi_descriptor_with_matching_tracks_combines_once() {
    let temp = tempfile::tempdir().expect("temp dir");
    let (gdi, track1, track2) = gdi_release(temp.path(), b"abc");
    let dat = gdi_catalogue(Some(sha1_of(&gdi)));
    let outcome = match_loose_files_against_dat(
        &dat,
        &[track1.clone(), track2.clone(), gdi.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert_eq!(outcome.matches.len(), 1, "{:?}", outcome.matches);
    assert_eq!(outcome.matches[0].archive_path, gdi);
    assert_eq!(outcome.matches[0].companion_paths, vec![track1, track2]);
    assert!(outcome.rejected_launchers.is_empty());
}

/// Existing behaviour, kept as a guard (this passed before the Priority 2
/// fix too): a track that is not a regular file is refused by the GDI
/// resolver itself, so the release is rejected as incomplete.
#[cfg(unix)]
#[test]
fn a_gdi_whose_track_is_not_a_regular_file_is_rejected_by_the_resolver() {
    let temp = tempfile::tempdir().expect("temp dir");
    let (gdi, track1, track2) = gdi_release(temp.path(), b"abc");
    std::fs::remove_file(&track2).unwrap();
    let status = std::process::Command::new("mkfifo")
        .arg(&track2)
        .status()
        .expect("mkfifo");
    assert!(status.success());
    let outcome = match_loose_files_against_dat(
        &gdi_catalogue(None),
        &[track1, gdi.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
    assert_eq!(outcome.rejected_launchers.len(), 1, "{outcome:?}");
    assert!(outcome.rejected_launchers[0].reason.contains("incomplete"));
}

/// Regression (optical Priority 2): a track that exists but was never
/// hashed (left out of the scan, or a hash refusal) is unverified evidence,
/// so the release must not combine on its other tracks alone.
#[test]
fn regression_a_gdi_track_that_was_not_hashed_blocks_the_release() {
    let temp = tempfile::tempdir().expect("temp dir");
    let (gdi, track1, _track2) = gdi_release(temp.path(), b"abc");
    let outcome = match_loose_files_against_dat(
        &gdi_catalogue(None),
        &[track1, gdi.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
    assert_eq!(outcome.rejected_launchers.len(), 1, "{outcome:?}");
    assert!(outcome.rejected_launchers[0].reason.contains("track02.bin"));
}

/// Regression (optical Priority 2): the same rule for CUE sheets.
#[test]
fn regression_a_cue_with_a_changed_track_is_rejected_not_combined() {
    let temp = tempfile::tempdir().expect("temp dir");
    let bin1 = temp.path().join("track1.bin");
    let bin2 = temp.path().join("track2.bin");
    std::fs::write(&bin1, b"test").unwrap();
    std::fs::write(&bin2, b"xyz").unwrap();
    let cue = write_cue(
        temp.path(),
        "game.cue",
        "FILE \"track1.bin\" BINARY\nFILE \"track2.bin\" BINARY\n",
    );
    let dat = dat_with_roms(vec![
        DatRomEntry {
            name: "track1.bin".to_string(),
            sha1: Some(SHA1_TEST.to_string()),
            ..DatRomEntry::default()
        },
        DatRomEntry {
            name: "track2.bin".to_string(),
            sha1: Some(SHA1_ABC.to_string()),
            ..DatRomEntry::default()
        },
    ]);
    let outcome = match_loose_files_against_dat(
        &dat,
        &[bin1, bin2, cue.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
    assert_eq!(outcome.rejected_launchers.len(), 1, "{outcome:?}");
    assert_eq!(outcome.rejected_launchers[0].launcher_path, cue);
}

/// Identical tracks across catalogue entries are common (audio tracks
/// shared by regional releases). A complete release whose other
/// track is unique to one entry still combines with that entry. Commit
/// 117ad73 wrongly refused this; the parent accepted it.
#[test]
fn regression_a_complete_release_with_a_track_shared_by_two_entries_still_combines() {
    let temp = tempfile::tempdir().expect("temp dir");
    let (gdi, track1, track2) = gdi_release(temp.path(), b"abc");
    let rom = |name: &str, sha1: &str| DatRomEntry {
        name: name.to_string(),
        sha1: Some(sha1.to_string()),
        ..DatRomEntry::default()
    };
    let mut dat = dat_with_roms(vec![
        rom("track01.bin", SHA1_TEST),
        rom("track02.bin", SHA1_ABC),
    ]);
    dat.games.push(DatGameEntry {
        name: "Game (USA)".to_string(),
        roms: vec![rom("track01.bin", SHA1_XYZ), rom("track02.bin", SHA1_ABC)],
        ..DatGameEntry::default()
    });
    let outcome = match_loose_files_against_dat(
        &dat,
        &[track1.clone(), track2.clone(), gdi.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert_eq!(outcome.matches.len(), 1, "{outcome:?}");
    assert_eq!(outcome.matches[0].archive_path, gdi);
    assert_eq!(outcome.matches[0].dat_entry_index, 0);
    assert_eq!(outcome.matches[0].companion_paths, vec![track1, track2]);
    assert!(outcome.rejected_launchers.is_empty());
}

/// Regression (optical Priority 2): a listed descriptor that our parser
/// refuses still hashes to its catalogue row, but it proves nothing about
/// any track, so it must not become a standalone match.
#[test]
fn regression_a_listed_descriptor_that_does_not_parse_is_not_a_match() {
    let temp = tempfile::tempdir().expect("temp dir");
    let gdi = temp.path().join("game.gdi");
    std::fs::write(&gdi, "not a gdi descriptor\n").unwrap();
    let dat = gdi_catalogue(Some(sha1_of(&gdi)));
    let outcome = match_loose_files_against_dat(
        &dat,
        std::slice::from_ref(&gdi),
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
    assert_eq!(outcome.rejected_launchers.len(), 1, "{outcome:?}");
    assert_eq!(outcome.rejected_launchers[0].launcher_path, gdi);
}

/// A real release reached through a symlinked source folder. The matcher
/// compares canonical track paths with the scan's own spellings; whatever
/// that yields, it must never be a descriptor-only match or a partial one.
#[cfg(unix)]
#[test]
fn a_release_under_a_symlinked_source_root_is_never_a_partial_match() {
    let temp = tempfile::tempdir().expect("temp dir");
    let real = temp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    gdi_release(&real, b"abc");
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let gdi = link.join("game.gdi");
    let dat = gdi_catalogue(Some(sha1_of(&gdi)));
    let outcome = match_loose_files_against_dat(
        &dat,
        &[link.join("track01.bin"), link.join("track02.bin"), gdi],
        &TrustedRoots::from_paths([&link]),
        &AtomicBool::new(false),
    );
    for found in &outcome.matches {
        assert!(
            found.companion_paths.len() == 2,
            "a descriptor-only, partial or lone-track match: {outcome:?}"
        );
    }
}

/// Two GDI discs of one multidisc release. The catalogue lists each disc's
/// descriptor and tracks; `track_bytes` are written for every disc track.
fn two_gdi_discs(dir: &Path, track_bytes: &[u8]) -> (ParsedDat, Vec<PathBuf>, PathBuf) {
    let mut games = Vec::new();
    let mut files = Vec::new();
    for disc in 1..=2 {
        let disc_dir = dir.join(format!("disc{disc}"));
        std::fs::create_dir(&disc_dir).unwrap();
        let (gdi, track1, track2) = gdi_release(&disc_dir, track_bytes);
        std::fs::write(&track1, track_bytes).unwrap();
        if disc == 2 {
            // A distinct (still valid) descriptor, so each disc's own row is
            // a unique catalogue match.
            std::fs::write(
                &gdi,
                "2\n1 0 4 2352 track01.bin 0\n2 45150 4 2352 track02.bin 0\n",
            )
            .unwrap();
        }
        let descriptor = DatRomEntry {
            name: format!("disc{disc}.gdi"),
            sha1: Some(sha1_of(&gdi)),
            ..DatRomEntry::default()
        };
        let track = |name: &str, sha1: &str| DatRomEntry {
            name: name.to_string(),
            sha1: Some(sha1.to_string()),
            ..DatRomEntry::default()
        };
        let (a, b) = if disc == 1 {
            (SHA1_TEST, SHA1_ABC)
        } else {
            (SHA1_XYZ, "0000000000000000000000000000000000000002")
        };
        games.push(DatGameEntry {
            name: format!("Orbit (Disc {disc} of 2)"),
            roms: vec![descriptor, track("track01.bin", a), track("track02.bin", b)],
            ..DatGameEntry::default()
        });
        files.extend([track1, track2, gdi]);
    }
    let m3u = dir.join("orbit.m3u");
    std::fs::write(&m3u, "disc1/game.gdi\ndisc2/game.gdi\n").unwrap();
    files.push(m3u.clone());
    let dat = ParsedDat {
        source: dat_with_one_rom(SHA1_TEST).source,
        games,
    };
    (dat, files, m3u)
}

/// Regression (optical Priority 2): an M3U of GDI discs whose descriptors
/// match but whose tracks were never checked or have changed is not a
/// verified multidisc release, and no lone track or descriptor stands in.
#[test]
fn regression_an_m3u_of_gdi_discs_with_changed_tracks_is_not_a_match() {
    let temp = tempfile::tempdir().expect("temp dir");
    let (dat, files, m3u) = two_gdi_discs(temp.path(), b"changed");
    let outcome = match_loose_files_against_dat(
        &dat,
        &files,
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
    // The playlist is refused, and so is disc 2's own descriptor when it is
    // tried on its own afterwards (its tracks changed too).
    assert!(
        outcome
            .rejected_launchers
            .iter()
            .any(|rejected| rejected.launcher_path == m3u),
        "{outcome:?}"
    );
}

/// Regression (optical Priority 2): an M3U disc that is a CUE with one
/// changed track is not verified, so the playlist is refused.
#[test]
fn regression_an_m3u_cue_disc_with_one_changed_track_is_rejected() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rom = |name: &str, sha1: &str| DatRomEntry {
        name: name.to_string(),
        sha1: Some(sha1.to_string()),
        ..DatRomEntry::default()
    };
    std::fs::write(temp.path().join("d1t1.bin"), b"test").unwrap();
    std::fs::write(temp.path().join("d1t2.bin"), b"changed").unwrap();
    std::fs::write(temp.path().join("d2t1.bin"), b"xyz").unwrap();
    let cue1 = write_cue(
        temp.path(),
        "disc1.cue",
        "FILE \"d1t1.bin\" BINARY\nFILE \"d1t2.bin\" BINARY\n",
    );
    let cue2 = write_cue(temp.path(), "disc2.cue", "FILE \"d2t1.bin\" BINARY\n");
    let m3u = temp.path().join("game.m3u");
    std::fs::write(&m3u, "disc1.cue\ndisc2.cue\n").unwrap();
    let dat = ParsedDat {
        source: dat_with_one_rom(SHA1_TEST).source,
        games: vec![
            DatGameEntry {
                name: "Orbit (Disc 1 of 2)".to_string(),
                roms: vec![rom("d1t1.bin", SHA1_TEST), rom("d1t2.bin", SHA1_ABC)],
                ..DatGameEntry::default()
            },
            DatGameEntry {
                name: "Orbit (Disc 2 of 2)".to_string(),
                roms: vec![rom("d2t1.bin", SHA1_XYZ)],
                ..DatGameEntry::default()
            },
        ],
    };
    let outcome = match_loose_files_against_dat(
        &dat,
        &[
            temp.path().join("d1t1.bin"),
            temp.path().join("d1t2.bin"),
            temp.path().join("d2t1.bin"),
            cue1,
            cue2.clone(),
            m3u.clone(),
        ],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );
    assert!(
        outcome
            .rejected_launchers
            .iter()
            .any(|rejected| rejected.launcher_path == m3u),
        "{outcome:?}"
    );
    // Nothing claims disc 1. Disc 2 is complete and matches its own entry,
    // so it may still stand as that single disc: the M3U pass stops at the
    // first bad disc and never claims disc 2 (existing behaviour).
    for found in &outcome.matches {
        assert_eq!(found.archive_path, cue2, "{outcome:?}");
        assert_eq!(found.companion_paths, vec![temp.path().join("d2t1.bin")]);
    }
}

#[test]
fn a_single_chd_produces_an_ordinary_single_file_match_unchanged() {
    let temp = tempfile::tempdir().expect("temp dir");
    let chd = temp.path().join("game.chd");
    std::fs::write(&chd, b"test").unwrap();
    let dat = dat_with_one_rom(SHA1_TEST);

    let outcome = match_loose_files_against_dat(
        &dat,
        std::slice::from_ref(&chd),
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1);
    assert_eq!(outcome.matches[0].archive_path, chd);
    assert!(outcome.matches[0].companion_paths.is_empty());
}

fn two_disc_dat() -> ParsedDat {
    ParsedDat {
        source: dat_with_one_rom(SHA1_TEST).source,
        games: vec![
            DatGameEntry {
                name: "Final Fantasy VII (Disc 1 of 3)".to_string(),
                roms: vec![DatRomEntry {
                    name: "disc1.chd".to_string(),
                    size_bytes: Some(4),
                    sha1: Some(SHA1_TEST.to_string()),
                    ..DatRomEntry::default()
                }],
                ..DatGameEntry::default()
            },
            DatGameEntry {
                name: "Final Fantasy VII (Disc 2 of 3)".to_string(),
                roms: vec![DatRomEntry {
                    name: "disc2.chd".to_string(),
                    size_bytes: Some(3),
                    sha1: Some(SHA1_ABC.to_string()),
                    ..DatRomEntry::default()
                }],
                ..DatGameEntry::default()
            },
        ],
    }
}

#[test]
fn an_m3u_with_two_discs_becomes_one_combined_match() {
    let temp = tempfile::tempdir().expect("temp dir");
    let disc1 = temp.path().join("disc1.chd");
    let disc2 = temp.path().join("disc2.chd");
    std::fs::write(&disc1, b"test").unwrap();
    std::fs::write(&disc2, b"abc").unwrap();
    let m3u = temp.path().join("game.m3u");
    std::fs::write(&m3u, "disc1.chd\ndisc2.chd\n").unwrap();
    let dat = two_disc_dat();

    let outcome = match_loose_files_against_dat(
        &dat,
        &[disc1.clone(), disc2.clone(), m3u.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1, "{:?}", outcome.matches);
    assert_eq!(outcome.matches[0].archive_path, m3u);
    assert_eq!(outcome.matches[0].dat_entry_index, 0);
    assert_eq!(outcome.matches[0].companion_paths.len(), 2);
    assert!(outcome.matches[0].companion_paths.contains(&disc1));
    assert!(outcome.matches[0].companion_paths.contains(&disc2));
    assert!(outcome.rejected_launchers.is_empty());
}

#[test]
fn an_m3u_whose_discs_are_cues_includes_each_discs_own_companions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let bin1 = temp.path().join("disc1_track1.bin");
    let bin2 = temp.path().join("disc2_track1.bin");
    std::fs::write(&bin1, b"test").unwrap();
    std::fs::write(&bin2, b"abc").unwrap();
    let cue1 = write_cue(
        temp.path(),
        "disc1.cue",
        "FILE \"disc1_track1.bin\" BINARY\n",
    );
    let cue2 = write_cue(
        temp.path(),
        "disc2.cue",
        "FILE \"disc2_track1.bin\" BINARY\n",
    );
    let m3u = temp.path().join("game.m3u");
    std::fs::write(&m3u, "disc1.cue\ndisc2.cue\n").unwrap();
    let dat = ParsedDat {
        source: dat_with_one_rom(SHA1_TEST).source,
        games: vec![
            DatGameEntry {
                name: "Final Fantasy VII (Disc 1 of 2)".to_string(),
                roms: vec![DatRomEntry {
                    name: "disc1_track1.bin".to_string(),
                    size_bytes: Some(4),
                    sha1: Some(SHA1_TEST.to_string()),
                    ..DatRomEntry::default()
                }],
                ..DatGameEntry::default()
            },
            DatGameEntry {
                name: "Final Fantasy VII (Disc 2 of 2)".to_string(),
                roms: vec![DatRomEntry {
                    name: "disc2_track1.bin".to_string(),
                    size_bytes: Some(3),
                    sha1: Some(SHA1_ABC.to_string()),
                    ..DatRomEntry::default()
                }],
                ..DatGameEntry::default()
            },
        ],
    };

    let outcome = match_loose_files_against_dat(
        &dat,
        &[
            bin1.clone(),
            bin2.clone(),
            cue1.clone(),
            cue2.clone(),
            m3u.clone(),
        ],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.matches.len(), 1, "{:?}", outcome.matches);
    assert_eq!(outcome.matches[0].archive_path, m3u);
    // Every disc CUE plus its own BIN companion is included.
    assert_eq!(outcome.matches[0].companion_paths.len(), 4);
    for path in [&bin1, &bin2, &cue1, &cue2] {
        assert!(
            outcome.matches[0].companion_paths.contains(path),
            "missing {path:?} in {:?}",
            outcome.matches[0].companion_paths
        );
    }
}

#[test]
fn an_m3u_naming_a_missing_disc_is_rejected_with_a_plain_reason() {
    let temp = tempfile::tempdir().expect("temp dir");
    let disc1 = temp.path().join("disc1.chd");
    std::fs::write(&disc1, b"test").unwrap();
    let m3u = temp.path().join("game.m3u");
    std::fs::write(&m3u, "disc1.chd\nmissing_disc2.chd\n").unwrap();
    let dat = two_disc_dat();

    let outcome = match_loose_files_against_dat(
        &dat,
        &[disc1.clone(), m3u.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    // The M3U itself is rejected with a plain reason - never silently
    // combined with a disc missing.
    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert_eq!(outcome.rejected_launchers[0].launcher_path, m3u);
    assert!(outcome.rejected_launchers[0].reason.contains("missing"));
    // Disc 1 is structurally named by the rejected M3U, so it is excluded
    // from independent election too, even though it individually verifies
    // against the catalogue - the whole logical release fails closed.
    assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
}

#[test]
fn a_rejected_m3u_suppresses_its_disc_cue_launchers_and_their_own_companions() {
    // Disc 2 is missing entirely; disc 1 is a fully valid, fully verified
    // CUE+BIN pair on its own. The rejected M3U must still suppress disc
    // 1's CUE (and its BIN) from becoming its own independent election -
    // "M3U rejection must suppress its structurally referenced disc
    // launchers and their structurally resolved companions."
    let temp = tempfile::tempdir().expect("temp dir");
    let bin1 = temp.path().join("disc1_track1.bin");
    std::fs::write(&bin1, b"test").unwrap();
    let cue1 = write_cue(
        temp.path(),
        "disc1.cue",
        "FILE \"disc1_track1.bin\" BINARY\n",
    );
    let m3u = temp.path().join("game.m3u");
    std::fs::write(&m3u, "disc1.cue\ndisc2.cue\n").unwrap();
    let dat = ParsedDat {
        source: dat_with_one_rom(SHA1_TEST).source,
        games: vec![DatGameEntry {
            name: "Final Fantasy VII (Disc 1 of 2)".to_string(),
            roms: vec![DatRomEntry {
                name: "disc1_track1.bin".to_string(),
                size_bytes: Some(4),
                sha1: Some(SHA1_TEST.to_string()),
                ..DatRomEntry::default()
            }],
            ..DatGameEntry::default()
        }],
    };

    let outcome = match_loose_files_against_dat(
        &dat,
        &[bin1.clone(), cue1.clone(), m3u.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert_eq!(outcome.rejected_launchers[0].launcher_path, m3u);
    // Neither disc 1's CUE nor its BIN companion appear as elections, even
    // though both individually verify against the catalogue.
    assert!(
        outcome
            .matches
            .iter()
            .all(|found| found.archive_path != cue1 && found.archive_path != bin1),
        "{:?}",
        outcome.matches
    );
}

#[test]
fn an_m3u_referencing_traversal_paths_is_rejected() {
    let temp = tempfile::tempdir().expect("temp dir");
    let m3u = temp.path().join("game.m3u");
    std::fs::write(&m3u, "../outside.chd\n").unwrap();
    let dat = two_disc_dat();

    let outcome = match_loose_files_against_dat(
        &dat,
        std::slice::from_ref(&m3u),
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert_eq!(outcome.rejected_launchers[0].launcher_path, m3u);
    assert!(outcome.rejected_launchers[0].reason.contains("unsafe"));
}
