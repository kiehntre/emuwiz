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
        &[file.clone()],
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
        &[cue.clone()],
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

#[test]
fn a_single_chd_produces_an_ordinary_single_file_match_unchanged() {
    let temp = tempfile::tempdir().expect("temp dir");
    let chd = temp.path().join("game.chd");
    std::fs::write(&chd, b"test").unwrap();
    let dat = dat_with_one_rom(SHA1_TEST);

    let outcome = match_loose_files_against_dat(
        &dat,
        &[chd.clone()],
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
        &[m3u.clone()],
        &TrustedRoots::from_paths([temp.path()]),
        &AtomicBool::new(false),
    );

    assert!(outcome.matches.is_empty());
    assert_eq!(outcome.rejected_launchers.len(), 1);
    assert_eq!(outcome.rejected_launchers[0].launcher_path, m3u);
    assert!(outcome.rejected_launchers[0].reason.contains("unsafe"));
}
