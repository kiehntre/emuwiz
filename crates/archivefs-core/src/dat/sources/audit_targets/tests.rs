use super::*;
use crate::dat::limits::DatLimits;
use crate::dat::sources::DatSourceKind;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{
    DatAuditError, DatAuditOutcome, DatAuditRequest, run_dat_audit_with_cache,
    run_dat_audit_with_targets,
};
use crate::safe_read::TrustedRoots;
use std::sync::atomic::AtomicBool;

// sha1/crc32 of the four bytes "abcd".
const ROM: &[u8] = b"abcd";
const DAT: &str = r#"<?xml version="1.0"?><datafile><header><name>Test GBA</name></header><game name="Game"><rom name="game.gba" size="4" crc="ed82cd11" sha1="81fe8bfe87576c3ecb22426f8e57847382917acf"/></game></datafile>"#;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    dat: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("gba");
    std::fs::create_dir_all(&root).unwrap();
    let dat = dir.path().join("test.dat");
    std::fs::write(&dat, DAT).unwrap();
    Fixture {
        _dir: dir,
        root,
        dat,
    }
}

fn request(f: &Fixture, scan_root: &Path) -> DatAuditRequest {
    DatAuditRequest {
        source_id: "test".into(),
        source_display_name: "Test GBA".into(),
        dat_path: f.dat.clone(),
        dat_kind: DatSourceKind::File,
        scan_root: scan_root.to_path_buf(),
        limits: DatLimits::default(),
        policy: None,
        platform: Some("Game Boy Advance".into()),
    }
}

/// The games-only folder walk (support files counted, not hashed).
fn walk(f: &Fixture) -> DatAuditOutcome {
    with_targets(f, &AuditTargets::FolderWalkGamesOnly).unwrap()
}

/// The legacy full walk, which stays the default.
fn full_walk(f: &Fixture) -> DatAuditOutcome {
    run_dat_audit_with_cache(
        &request(f, &f.root),
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap()
}

fn with_targets(f: &Fixture, targets: &AuditTargets) -> Result<DatAuditOutcome, DatAuditError> {
    run_dat_audit_with_targets(
        &request(f, &f.root),
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
        targets,
    )
}

fn write(f: &Fixture, rel: &str, bytes: &[u8]) -> PathBuf {
    let path = f.root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

const SUPPORT: [&str; 12] = [
    "a.png",
    "a.jpg",
    "a.jpeg",
    "manual.pdf",
    "notes.txt",
    "game.nfo",
    "meta.json",
    "meta.xml",
    "trailer.mp4",
    "theme.mp3",
    "t.webm",
    "t.ogg",
];

#[test]
fn the_folder_walk_never_hashes_artwork_manuals_or_metadata() {
    let f = fixture();
    write(&f, "game.gba", ROM);
    for name in SUPPORT {
        // Large relative to the ROM: if any were hashed bytes_hashed would show it.
        write(&f, &format!("assets/{name}"), &[7u8; 4096]);
    }
    let outcome = walk(&f);
    assert_eq!(outcome.files_scanned, 1, "only the ROM is a candidate");
    assert_eq!(outcome.bytes_hashed, ROM.len() as u64);
    assert_eq!(outcome.population.basis, AuditPopulationBasis::FolderWalk);
    assert_eq!(outcome.population.support_files_skipped, SUPPORT.len());
}

#[test]
fn the_default_full_walk_is_unchanged_so_ancillary_accounting_keeps_working() {
    // The repair report counts artwork and manuals as ignored ancillary files
    // and needs the walk to see them; only callers that ask for games-only skip them.
    let f = fixture();
    write(&f, "game.gba", ROM);
    for name in SUPPORT {
        write(&f, &format!("assets/{name}"), &[7u8; 64]);
    }
    let outcome = full_walk(&f);
    assert_eq!(outcome.files_scanned, 1 + SUPPORT.len());
    assert_eq!(outcome.population.basis, AuditPopulationBasis::FolderWalk);
    assert_eq!(outcome.population.support_files_skipped, 0);
    assert_eq!(AuditTargets::default(), AuditTargets::FolderWalk);
}

#[test]
fn extensions_with_game_meaning_are_still_candidates_including_the_md_collision() {
    let f = fixture();
    // `.md` is a Mega Drive ROM extension, not a markdown sidecar.
    for name in [
        "game.md", "game.gba", "game.bin", "game.zip", "game.cue", "game.chd", "game.iso",
        "game.gen",
    ] {
        write(&f, name, ROM);
    }
    let outcome = walk(&f);
    assert_eq!(outcome.files_scanned, 8);
    assert_eq!(outcome.population.support_files_skipped, 0);
    for extension in ["md", "gen", "bin", "cue", "chd", "iso", "zip", "gba"] {
        assert!(
            !crate::ingestion::discovery::is_known_non_game_extension(extension),
            ".{extension} must never be classed as a support file"
        );
    }
    // Upper-case support extensions are support files too.
    let g = fixture();
    write(&g, "game.gba", ROM);
    write(&g, "COVER.PNG", &[1u8; 64]);
    write(&g, "Manual.PDF", &[1u8; 64]);
    assert_eq!(walk(&g).files_scanned, 1);
}

#[test]
fn a_file_named_directly_is_audited_even_if_it_looks_like_support() {
    let f = fixture();
    let png = write(&f, "odd.png", ROM);
    let outcome = run_dat_audit_with_cache(
        &request(&f, &png),
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    assert_eq!(
        outcome.files_scanned, 1,
        "an explicit single-file target is a deliberate choice"
    );
}

#[test]
fn catalogue_targets_verify_only_the_catalogued_games_not_unknown_neighbours() {
    let f = fixture();
    let game = write(&f, "game.gba", ROM);
    // Unknown files that are not known support types: a folder walk would hash them.
    write(&f, "Castlevania.cia", &[3u8; 2048]);
    write(&f, "weird.bin2", &[3u8; 2048]);
    write(&f, "assets/cover.png", &[3u8; 2048]);
    let targets = catalogue_audit_targets([game.clone()], &f.root);
    let outcome = with_targets(&f, &AuditTargets::Catalogue(targets)).unwrap();
    assert_eq!(outcome.files_scanned, 1);
    assert_eq!(outcome.bytes_hashed, ROM.len() as u64);
    assert_eq!(outcome.population.basis, AuditPopulationBasis::Catalogue);
    assert_eq!(outcome.population.catalogue_rows, 1);
    assert_eq!(outcome.population.catalogue_rows_unavailable, 0);
    // The one catalogued ROM still verifies exactly.
    assert_eq!(outcome.report.entries.len(), 1);
    assert!(
        matches!(
            outcome.report.entries[0].verdict,
            crate::dat::audit::AuditVerdict::Exact { .. }
        ),
        "{:?}",
        outcome.report.entries[0].verdict
    );
}

#[test]
fn a_catalogue_row_whose_file_is_gone_is_unavailable_not_silently_dropped() {
    let f = fixture();
    let present = write(&f, "game.gba", ROM);
    let gone = f.root.join("missing.gba");
    let targets = catalogue_audit_targets([present, gone], &f.root);
    assert_eq!(targets.catalogue_rows, 1);
    assert_eq!(targets.catalogue_rows_unavailable, 1);
    let outcome = with_targets(&f, &AuditTargets::Catalogue(targets)).unwrap();
    assert_eq!(outcome.population.catalogue_rows_unavailable, 1);
    assert_eq!(outcome.files_scanned, 1);
}

#[test]
fn an_empty_catalogue_population_is_a_clear_error_not_a_folder_walk() {
    let f = fixture();
    write(&f, "game.gba", ROM);
    let targets = catalogue_audit_targets(Vec::<PathBuf>::new(), &f.root);
    let error = with_targets(&f, &AuditTargets::Catalogue(targets)).unwrap_err();
    assert!(
        matches!(&error, DatAuditError::NothingToAudit(text) if text.contains("Scan the folder first")),
        "{error:?}"
    );
}

#[test]
fn rows_outside_the_scan_folder_and_directory_rows_are_not_targets() {
    let f = fixture();
    let inside = write(&f, "game.gba", ROM);
    let elsewhere = f._dir.path().join("other/game.gba");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::write(&elsewhere, ROM).unwrap();
    let dir_row = f.root.join("a_set_dir");
    std::fs::create_dir_all(&dir_row).unwrap();
    let targets = catalogue_audit_targets([inside.clone(), elsewhere, dir_row], &f.root);
    assert_eq!(targets.files, vec![inside]);
    assert_eq!(targets.catalogue_rows, 1);
    assert_eq!(targets.catalogue_rows_unavailable, 0);
}

#[test]
fn a_catalogued_cue_brings_its_track_files_and_an_unrelated_bin_stays_out() {
    let f = fixture();
    let cue = write(
        &f,
        "Game.cue",
        b"FILE \"track01.bin\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\nFILE \"track02.bin\" BINARY\n TRACK 02 AUDIO\n  INDEX 01 00:00:00\n",
    );
    let t1 = write(&f, "track01.bin", &[1u8; 16]);
    let t2 = write(&f, "track02.bin", &[2u8; 16]);
    write(&f, "orphan.bin", &[9u8; 16]);
    let targets = catalogue_audit_targets([cue.clone()], &f.root);
    assert_eq!(targets.files, vec![cue, t1, t2]);
    assert_eq!(targets.catalogue_rows, 1);
    assert_eq!(targets.companion_files, 2);
}

#[test]
fn an_archive_row_is_one_target_without_expansion() {
    let f = fixture();
    let zip = write(&f, "Game.zip", b"PK\x03\x04not a real zip");
    let targets = catalogue_audit_targets([zip.clone()], &f.root);
    assert_eq!(targets.files, vec![zip]);
    assert_eq!(targets.companion_files, 0);
}

#[test]
fn targets_are_sorted_deduplicated_and_order_independent() {
    let f = fixture();
    let a = write(&f, "a.gba", ROM);
    let b = write(&f, "b.gba", ROM);
    let one = catalogue_audit_targets([b.clone(), a.clone(), a.clone()], &f.root);
    let two = catalogue_audit_targets([a.clone(), b.clone()], &f.root);
    assert_eq!(one, two);
    assert_eq!(one.files, vec![a, b]);
}

#[test]
fn a_pilot_shaped_folder_hashes_only_the_games() {
    // Many games beside far more artwork and manuals, as in a real library
    // folder. Asserts on bytes actually hashed, not on timing.
    let f = fixture();
    let mut games = Vec::new();
    for i in 0..300 {
        games.push(write(&f, &format!("Game {i:04}.gba"), ROM));
    }
    for i in 0..900 {
        write(&f, &format!("covers/c{i}.png"), &[5u8; 512]);
        write(&f, &format!("screenshots/s{i}.jpg"), &[5u8; 512]);
    }
    for i in 0..500 {
        write(&f, &format!("manuals/m{i}.pdf"), &[5u8; 2048]);
    }
    // Folder walk: games only.
    let walked = walk(&f);
    assert_eq!(walked.files_scanned, 300);
    assert_eq!(walked.bytes_hashed, 300 * ROM.len() as u64);
    assert_eq!(walked.population.support_files_skipped, 900 + 900 + 500);
    // Catalogue targets: the same population by explicit rows.
    let catalogue = catalogue_audit_targets(games, &f.root);
    let outcome = with_targets(&f, &AuditTargets::Catalogue(catalogue)).unwrap();
    assert_eq!(outcome.files_scanned, 300);
    assert_eq!(outcome.bytes_hashed, 300 * ROM.len() as u64);
}
