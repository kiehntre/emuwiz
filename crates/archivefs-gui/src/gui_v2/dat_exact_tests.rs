//! A saved exact DAT match makes a game verified in GUI-v2 with no click.
//! Everything runs through the real audit and save path on a temp database.

use super::backend::load_library;
use super::library::Game;
use archivefs_core::Database;
use archivefs_core::dat::sources::DatSourceKind;
use archivefs_core::dat::sources::audit_cache::AuditCacheConfig;
use archivefs_core::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use archivefs_core::safe_read::TrustedRoots;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

const SOURCE: &str = "165";

fn hashes(path: &Path) -> archivefs_core::identity_source::hashing::LocalHashes {
    archivefs_core::identity_source::hashing::hash_file_reporting(
        path,
        &TrustedRoots::none(),
        None,
        &|_| {},
    )
    .unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    roms: std::path::PathBuf,
    dat: std::path::PathBuf,
}

/// `exact` names are DAT-listed with SHA-256; the extras cover every state
/// that must not verify a game.
fn fixture(exact: &[String]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let roms = dir.path().join("roms");
    std::fs::create_dir(&roms).unwrap();
    let mut games = String::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let put = |name: &str, body: &[u8]| {
        std::fs::write(roms.join(name), body).unwrap();
        hashes(&roms.join(name))
    };
    for name in exact {
        let body = format!("exact rom {name}").into_bytes();
        let found = put(&format!("{name}.gbc"), &body);
        games += &format!(
            "<game name=\"{name}\"><rom name=\"{name}.gbc\" size=\"{}\" sha1=\"{}\"/></game>",
            body.len(),
            found.sha1
        );
        files.push((format!("{name}.gbc"), body));
    }
    // Two DAT games share one ROM hash: ExactMultipleCandidates.
    let shared = b"shared rom".to_vec();
    let found = put("ambiguous.gbc", &shared);
    for twin in ["Twin A (USA)", "Twin B (USA)"] {
        games += &format!(
            "<game name=\"{twin}\"><rom name=\"{twin}.gbc\" size=\"{}\" sha1=\"{}\"/></game>",
            shared.len(),
            found.sha1
        );
    }
    files.push(("ambiguous.gbc".into(), shared));
    // CRC32 + size only: Probable at best.
    let weak = b"weak rom".to_vec();
    let found = put("probable.gbc", &weak);
    games += &format!(
        "<game name=\"Weak (USA)\"><rom name=\"Weak (USA).gbc\" size=\"{}\" crc=\"{}\"/></game>",
        weak.len(),
        found.crc32
    );
    files.push(("probable.gbc".into(), weak));
    put("unlisted.gbc", b"not in any dat");
    files.push(("unlisted.gbc".into(), b"not in any dat".to_vec()));
    let dat = dir.path().join("gbc.dat");
    std::fs::write(
        &dat,
        format!(
            "<?xml version=\"1.0\"?><datafile><header><name>Nintendo - Game Boy Color</name>\
             <description>Nintendo - Game Boy Color</description><version>1</version>\
             <author>No-Intro</author><homepage>No-Intro</homepage></header>{games}</datafile>"
        ),
    )
    .unwrap();

    let db = dir.path().join("library.sqlite3");
    Database::open_or_create(&db).unwrap().close().unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute(
            "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')",
            [roms.as_os_str().as_bytes()],
        )
        .unwrap();
    for (index, (name, _)) in files.iter().enumerate() {
        connection.execute(
            "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at)
             VALUES(?1,1,?2,?3,?2,'direct_game_image',?4,?4,'now','now','now','now')",
            rusqlite::params![index as i64 + 1, name.as_bytes(), roms.join(name).as_os_str().as_bytes(), name],
        ).unwrap();
    }
    drop(connection);
    Fixture {
        _dir: dir,
        db,
        roms,
        dat,
    }
}

fn audit_and_save(fixture: &Fixture) {
    let request = DatAuditRequest {
        source_id: SOURCE.into(),
        source_display_name: "No-Intro: Nintendo - Game Boy Color".into(),
        dat_path: fixture.dat.clone(),
        dat_kind: DatSourceKind::File,
        scan_root: fixture.roms.clone(),
        limits: Default::default(),
        policy: None,
        platform: None,
    };
    let outcome = run_dat_audit_with_cache(
        &request,
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    let mut database = Database::open_or_create(&fixture.db).unwrap();
    database
        .persist_library_dat_identities_from_audit(&outcome)
        .unwrap();
}

fn names(count: usize) -> Vec<String> {
    // Beta/Demo/Proto/Sample entries are exact matches like any other.
    let tags = ["", " (Beta)", " (Demo)", " (Proto)", " (Sample)"];
    (0..count)
        .map(|index| format!("Game {index:02} (USA){}", tags[index % tags.len()]))
        .collect()
}

fn state_of(fixture: &Fixture, file: &str) -> Option<String> {
    rusqlite::Connection::open(&fixture.db)
        .unwrap()
        .query_row(
            "SELECT l.verification_state FROM library_dat_identities l JOIN archives a ON a.id = l.archive_id WHERE a.display_name = ?1",
            [file],
            |row| row.get::<_, String>(0),
        )
        .ok()
}

#[test]
fn eighty_exact_dat_matches_are_all_verified_without_a_click() {
    let exact = names(80);
    let fixture = fixture(&exact);
    audit_and_save(&fixture);
    let library = load_library(&fixture.db).unwrap();

    let verified: Vec<_> = library
        .games
        .iter()
        .filter(|game| exact.iter().any(|name| game.title == format!("{name}.gbc")))
        .collect();
    assert_eq!(verified.len(), 80);
    for game in verified {
        assert!(game.archive.identity_report.is_none());
        assert!(game.identified, "{} should be verified", game.title);
        assert_eq!(game.identity_summary(), "Verified");
        assert_ne!(game.status(), "Not checked yet");
        assert_eq!(
            game.dat_exact_label().as_deref(),
            Some("Exact No-Intro match")
        );
    }
}

#[test]
fn only_a_single_exact_match_verifies_a_game() {
    let fixture = fixture(&names(2));
    audit_and_save(&fixture);
    // The states below really were persisted (or withheld), and none verifies.
    assert_eq!(
        state_of(&fixture, "Game 00 (USA).gbc").as_deref(),
        Some("verified_single_match")
    );
    assert_ne!(
        state_of(&fixture, "ambiguous.gbc").as_deref(),
        Some("verified_single_match")
    );
    assert_ne!(
        state_of(&fixture, "probable.gbc").as_deref(),
        Some("verified_single_match")
    );
    assert_ne!(
        state_of(&fixture, "unlisted.gbc").as_deref(),
        Some("verified_single_match")
    );

    let library = load_library(&fixture.db).unwrap();
    for file in ["ambiguous.gbc", "probable.gbc", "unlisted.gbc"] {
        let game = library
            .games
            .iter()
            .find(|game| game.title == file)
            .unwrap();
        assert!(!game.identified, "{file} must not be verified");
        assert_eq!(game.identity_summary(), "Unknown");
        assert!(game.dat_exact_label().is_none());
    }
}

#[test]
fn a_stale_exact_row_does_not_verify_a_game() {
    let fixture = fixture(&names(3));
    audit_and_save(&fixture);
    assert!(
        load_library(&fixture.db)
            .unwrap()
            .games
            .iter()
            .any(|game| game.identified)
    );
    Database::open_or_create(&fixture.db)
        .unwrap()
        .mark_library_dat_identities_stale_for_source(SOURCE)
        .unwrap();
    let library = load_library(&fixture.db).unwrap();
    assert!(library.games.iter().all(|game| !game.identified));
    assert!(
        library
            .games
            .iter()
            .all(|game| game.identity_summary() == "Unknown")
    );
}

#[test]
fn a_row_with_no_recorded_catalogue_is_not_given_a_provider_name() {
    let fixture = fixture(&names(1));
    audit_and_save(&fixture);
    rusqlite::Connection::open(&fixture.db)
        .unwrap()
        .execute("UPDATE library_dat_identities SET dat_ecosystem = NULL", [])
        .unwrap();
    let library = load_library(&fixture.db).unwrap();
    let game = library.games.iter().find(|game| game.identified).unwrap();
    assert_eq!(game.dat_exact_label().as_deref(), Some("Exact DAT match"));
}

#[test]
fn scan_time_identity_still_verifies_and_needs_no_dat_row() {
    use archivefs_core::game_identity::*;
    let fixture = fixture(&names(1));
    let mut archive = load_library(&fixture.db).unwrap().games[0].archive.clone();
    archive.identity_report = Some(GameIdentityReport {
        archive_path: archive.absolute_path.clone(),
        platform: IdentityPlatform::Arcade,
        format: IdentityImageFormat::LooseCartridgeRom,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::MameMachineName,
            status: IdentityStatus::Verified,
            value: Some("pacman".into()),
            confidence: IdentityConfidence::ExactBytes,
            provenance: IdentityProvenance {
                archive_path: archive.absolute_path.clone(),
                member_path: None,
                member_index: None,
                method: "fixture".into(),
            },
            diagnostic: "fixture".into(),
        }],
        warnings: vec![],
        bytes_read: 1,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    });
    let game = Game::from_archive(archive);
    assert!(game.identified);
    assert_eq!(game.identity_summary(), "Verified");
    assert!(game.dat_exact_label().is_none());
}
