//! A saved exact DAT match makes a game verified in GUI-v2 with no click.
//! Everything runs through the real audit and save path on a temp database.

pub(super) fn catalogues(
    path: &std::path::Path,
) -> Result<
    std::collections::HashMap<
        String,
        archivefs_core::dat::configured_catalogues::ConfiguredCatalogue,
    >,
    String,
> {
    let config = archivefs_core::dat::sources::load_dat_sources_config_from(
        path.with_file_name("dat_sources.toml"),
    )
    .map_err(|error| error.to_string())?;
    let (local, problems) = archivefs_core::dat::sources::DatSourceRegistry::from_config(&config);
    assert!(problems.is_empty(), "{problems:?}");
    let sources = archivefs_core::dat::configured_catalogues::configured_catalogues(
        archivefs_core::dat::catalogue_selection::CatalogueInventoryInputs {
            local_registry: &local,
            managed_sources: &Default::default(),
            managed_root: &path.with_file_name("managed-dats"),
            limits: Default::default(),
        },
    );
    Ok(sources)
}

pub(super) fn load_library(path: &std::path::Path) -> Result<super::library::Library, String> {
    super::backend::load_library_with_catalogues(path, &catalogues(path)?)
}

use super::library::{Game, Library};
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

pub(super) struct Fixture {
    _dir: tempfile::TempDir,
    pub(super) db: std::path::PathBuf,
    roms: std::path::PathBuf,
    dat: std::path::PathBuf,
}

/// `exact` names are DAT-listed with SHA-256; the extras cover every state
/// that must not verify a game.
pub(super) fn fixture(exact: &[String]) -> Fixture {
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

    let mut registry = archivefs_core::dat::sources::DatSourceRegistry::new();
    registry
        .add(archivefs_core::dat::sources::DatSourceEntry::new(
            SOURCE.into(),
            "Synthetic No-Intro".into(),
            dat.clone(),
            DatSourceKind::File,
        ))
        .unwrap();
    archivefs_core::dat::sources::save_dat_sources_config_to(
        dir.path().join("dat_sources.toml"),
        &registry.to_config(),
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
    for (index, (name, bytes)) in files.iter().enumerate() {
        connection.execute(
            "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,size_bytes,first_seen_at,last_seen_at,created_at,updated_at)
             VALUES(?1,1,?2,?3,?2,'direct_game_image',?4,?4,?5,'now','now','now','now')",
            rusqlite::params![index as i64 + 1, name.as_bytes(), roms.join(name).as_os_str().as_bytes(), name, bytes.len() as i64],
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

pub(super) fn audit_and_save(fixture: &Fixture) {
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

pub(super) fn names(count: usize) -> Vec<String> {
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
    // Saved evidence is shown for what it is; none of it is a verification.
    for (file, label) in [
        ("ambiguous.gbc", "Ambiguous match"),
        ("probable.gbc", "Probable match"),
        // The fixture's files have no platform, so "not in this catalogue"
        // is not evidence about them (`negative_dat_result_applies`).
        ("unlisted.gbc", "Unknown platform"),
    ] {
        let game = library
            .games
            .iter()
            .find(|game| game.title == file)
            .unwrap();
        assert!(!game.identified, "{file} must not be verified");
        assert_eq!(game.identity_summary(), label, "{file}");
        assert!(game.dat_exact_label().is_none());
    }
}

/// A "not in this catalogue" result counts only for a game on the
/// catalogue's own platform: then it is shown, never as verified.
#[test]
fn a_same_platform_no_match_is_shown_and_another_platforms_is_not() {
    let fixture = fixture(&names(1));
    let connection = rusqlite::Connection::open(&fixture.db).unwrap();
    let set_platform = |platform: &str| {
        connection
            .execute_batch(&format!(
                "UPDATE platform_assignments SET is_current = 0; \
                 INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) \
                 SELECT id,'{platform}','manual',1,'now' FROM archives;"
            ))
            .unwrap();
    };
    // Classified before the audit, as a scan would have done.
    set_platform("Game Boy Color");
    audit_and_save(&fixture);
    assert_eq!(
        state_of(&fixture, "unlisted.gbc").as_deref(),
        Some("no_match")
    );
    let label = |library: &Library| {
        library
            .games
            .iter()
            .find(|game| game.title == "unlisted.gbc")
            .map(|game| (game.identified, game.identity_summary()))
    };
    assert_eq!(
        label(&load_library(&fixture.db).unwrap()),
        Some((false, "No catalogue match"))
    );
    // Reclassified to another platform: the saved row stays, as history only.
    set_platform("Game Boy Advance");
    assert_eq!(
        state_of(&fixture, "unlisted.gbc").as_deref(),
        Some("no_match")
    );
    assert_eq!(
        label(&load_library(&fixture.db).unwrap()),
        Some((false, "Platform recognised · Game not yet identified"))
    );
}

#[test]
fn corrupt_zip_is_not_checked_in_library_and_has_no_definitive_attention_negative() {
    let fixture = fixture(&names(1));
    let path = fixture.roms.join("broken.zip");
    std::fs::write(&path, b"PK\x03\x04unreadable archive control").unwrap();
    let connection = rusqlite::Connection::open(&fixture.db).unwrap();
    connection.execute(
        "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at)
         VALUES(100,1,CAST('broken.zip' AS BLOB),?1,CAST('broken.zip' AS BLOB),'zip','broken.zip','broken.zip','now','now','now','now')",
        [path.as_os_str().as_bytes()],
    ).unwrap();
    connection
        .execute_batch(
            "INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at)
         SELECT id,'Game Boy Color','manual',1,'now' FROM archives;",
        )
        .unwrap();
    drop(connection);
    audit_and_save(&fixture);
    assert_eq!(
        state_of(&fixture, "broken.zip").as_deref(),
        Some("no_usable_evidence")
    );
    let library = load_library(&fixture.db).unwrap();
    let broken = library
        .games
        .iter()
        .find(|game| game.title == "broken.zip")
        .unwrap();
    assert_eq!(
        broken.identity_summary(),
        "Platform recognised · Game not yet identified"
    );
    assert!(!broken.identified);
    assert!(broken.dat_exact_label().is_none());
    let unlisted = library
        .games
        .iter()
        .find(|game| game.title == "unlisted.gbc")
        .unwrap();
    assert_eq!(unlisted.identity_summary(), "No catalogue match");
    let db = Database::open_read_only(&fixture.db).unwrap();
    let attention = db.attention_snapshot().unwrap();
    let dat_items: Vec<_> = attention
        .items()
        .filter(|item| item.category == archivefs_core::attention::AttentionCategory::Dat)
        .collect();
    // Needs Attention combines verdict groups into one platform card. Its
    // total also includes the fixture's ambiguous record; the explanations
    // retain each verdict's own count, so the corrupt ZIP is not a no-match.
    assert_eq!(dat_items.len(), 1);
    assert_eq!(dat_items[0].affected_count, 3);
    assert!(
        dat_items[0]
            .summary
            .contains("1 saved identity records. No DAT entry matched the recorded evidence.")
    );
    assert!(dat_items[0].summary.contains(
        "1 saved identity records. There is not enough verified evidence to establish DAT authority."
    ));
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
    assert!(library.games.iter().all(|game| game.identity_summary()
        == if game.platform == super::library::UNKNOWN_PLATFORM {
            "Unknown platform"
        } else {
            "Platform recognised · Game not yet identified"
        }));
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

#[test]
fn saved_dat_status_is_visible_even_with_scan_identity_reports() {
    let fixture = fixture(&names(1));
    audit_and_save(&fixture);
    let library = load_library(&fixture.db).unwrap();
    let report = archivefs_core::game_identity::GameIdentityReport {
        archive_path: fixture.roms.join("unlisted.gbc"),
        platform: archivefs_core::game_identity::IdentityPlatform::Arcade,
        format: archivefs_core::game_identity::IdentityImageFormat::LooseCartridgeRom,
        evidence: vec![],
        warnings: vec![],
        bytes_read: 0,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    for (name, state, label) in [
        (
            "unlisted.gbc",
            archivefs_core::LibraryDatState::NoMatch,
            "No catalogue match",
        ),
        (
            "ambiguous.gbc",
            archivefs_core::LibraryDatState::Ambiguous,
            "Ambiguous match",
        ),
    ] {
        let mut game = library
            .games
            .iter()
            .find(|g| g.archive.relative_path == Path::new(name))
            .unwrap()
            .clone();
        game.archive.identity_report = Some(report.clone());
        game.dat_state = Some(state);
        assert_eq!(game.identity_summary(), label);
        assert_eq!(game.dat_identity_summary(), label);
    }
    let mut unchecked = library.games[0].clone();
    unchecked.dat_exact = None;
    unchecked.dat_state = None;
    unchecked.archive.identity_report = Some(report);
    assert_eq!(unchecked.dat_identity_summary(), "Not checked");
}

#[test]
fn details_show_loaded_dat_results_without_opening_review_identity_first() {
    let fixture = fixture(&names(1));
    rusqlite::Connection::open(&fixture.db).unwrap().execute("INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) SELECT id,'Game Boy Color','manual',1,'now' FROM archives",[]).unwrap();
    audit_and_save(&fixture);
    let ctx = eframe::egui::Context::default();
    let mut app = super::tests::fixture(&ctx);
    app.library = std::sync::Arc::new(load_library(&fixture.db).unwrap());
    for (name, label) in [
        ("unlisted.gbc", "DAT: No catalogue match"),
        ("ambiguous.gbc", "DAT: Ambiguous match"),
    ] {
        let id = app
            .library
            .games
            .iter()
            .find(|g| g.archive.relative_path == Path::new(name))
            .unwrap()
            .archive
            .id;
        app.router.current = super::Route::Game(id);
        assert!(app.review.knowledge_for.is_none());
        let output = ctx.run(
            eframe::egui::RawInput {
                screen_rect: Some(eframe::egui::Rect::from_min_size(
                    eframe::egui::Pos2::ZERO,
                    eframe::egui::vec2(1280.0, 1600.0),
                )),
                ..Default::default()
            },
            |ctx| app.show(ctx),
        );
        fn contains(shape: &eframe::egui::Shape, wanted: &str) -> bool {
            match shape {
                eframe::egui::Shape::Text(t) => t.galley.text() == wanted,
                eframe::egui::Shape::Vec(v) => v.iter().any(|s| contains(s, wanted)),
                _ => false,
            }
        }
        assert!(
            output.shapes.iter().any(|s| contains(&s.shape, label)),
            "missing loaded evidence {label}"
        );
        assert!(app.review.knowledge_for.is_none());
    }
}

#[test]
fn a_disabled_removed_or_changed_catalogue_does_not_supply_a_verified_badge() {
    for change in ["disabled", "removed", "same-version"] {
        let fixture = fixture(&names(2));
        audit_and_save(&fixture);
        assert_eq!(
            load_library(&fixture.db)
                .unwrap()
                .games
                .iter()
                .filter(|game| game.dat_exact.is_some())
                .count(),
            2
        );
        let library = match change {
            "disabled" => {
                super::backend::load_library_with_catalogues(&fixture.db, &Default::default())
                    .unwrap()
            }
            "removed" => {
                std::fs::remove_file(&fixture.dat).unwrap();
                load_library(&fixture.db).unwrap()
            }
            _ => {
                let body = std::fs::read_to_string(&fixture.dat).unwrap();
                std::fs::write(
                    &fixture.dat,
                    body.replace(
                        "</datafile>",
                        "<!-- same version, different bytes --></datafile>",
                    ),
                )
                .unwrap();
                load_library(&fixture.db).unwrap()
            }
        };
        assert!(
            library.games.iter().all(|game| game.dat_exact.is_none()),
            "{change}"
        );
    }
}

fn assert_saved_summary_is_not_current(fixture: &Fixture) {
    let database = Database::open_read_only(&fixture.db).unwrap();
    let snapshot = crate::database_load::load_snapshot_from(
        &database,
        &fixture.db,
        &fixture.db.with_file_name("config.toml"),
    )
    .unwrap_or_else(|_| panic!("fixture snapshot"));
    assert!(
        snapshot.dat_identities[&1]
            .iter()
            .all(|summary| summary.provenance_freshness
                != archivefs_core::dat::library_identity_summary::DatProvenanceFreshness::Current),
        "Game Details must not claim stale or unproven content is Current"
    );
}

fn assert_loose_current_evidence_is_revoked(change: impl FnOnce(&std::path::Path)) {
    let exact = names(1);
    let fixture = fixture(&exact);
    audit_and_save(&fixture);
    let title = format!("{}.gbc", exact[0]);
    assert!(
        load_library(&fixture.db)
            .unwrap()
            .games
            .iter()
            .find(|g| g.title == title)
            .unwrap()
            .identified
    );
    let conn = rusqlite::Connection::open(&fixture.db).unwrap();
    let before: Vec<u8> = conn
        .query_row(
            "SELECT facts_json FROM library_dat_identities WHERE archive_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    change(&fixture.roms.join(&title));
    let library = load_library(&fixture.db).unwrap();
    assert!(
        !library
            .games
            .iter()
            .find(|g| g.title == title)
            .unwrap()
            .identified,
        "saved database size alone cannot prove that the audited file is still current"
    );
    assert_saved_summary_is_not_current(&fixture);
    let after: Vec<u8> = conn
        .query_row(
            "SELECT facts_json FROM library_dat_identities WHERE archive_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, before, "saved evidence remains available for review");
}

#[test]
fn current_evidence_regression_deleted_loose_file_is_not_verified() {
    assert_loose_current_evidence_is_revoked(|path| std::fs::remove_file(path).unwrap());
}

#[test]
fn current_evidence_regression_same_size_replacement_is_not_verified() {
    assert_loose_current_evidence_is_revoked(|path| {
        let mut bytes = std::fs::read(path).unwrap();
        bytes[0] ^= 1;
        let replacement = path.with_extension("replacement");
        std::fs::write(&replacement, bytes).unwrap();
        std::fs::rename(replacement, path).unwrap();
    });
}

#[test]
fn current_evidence_regression_legacy_loose_match_needs_bound_file_proof() {
    let fixture = fixture(&names(1));
    audit_and_save(&fixture);
    let conn = rusqlite::Connection::open(&fixture.db).unwrap();
    conn.execute("UPDATE library_dat_identities SET facts_json=CAST(json_remove(CAST(facts_json AS TEXT),'$.audit_file') AS BLOB)", []).unwrap();
    assert!(
        load_library(&fixture.db)
            .unwrap()
            .games
            .iter()
            .all(|g| g.dat_exact.is_none()),
        "historical hashes without their audited file fingerprint do not establish current content"
    );
    assert!(state_of(&fixture, &format!("{}.gbc", names(1)[0])).is_some());
    assert_saved_summary_is_not_current(&fixture);
}

#[test]
fn current_evidence_regression_digestless_folder_catalogue_is_not_current() {
    let fixture = fixture(&names(1));
    let folder = fixture._dir.path().join("catalogues");
    std::fs::create_dir(&folder).unwrap();
    std::fs::copy(&fixture.dat, folder.join("games.dat")).unwrap();
    let mut registry = archivefs_core::dat::sources::DatSourceRegistry::new();
    registry
        .add(archivefs_core::dat::sources::DatSourceEntry::new(
            SOURCE.into(),
            "Folder DAT".into(),
            folder.clone(),
            DatSourceKind::Folder,
        ))
        .unwrap();
    archivefs_core::dat::sources::save_dat_sources_config_to(
        fixture.db.with_file_name("dat_sources.toml"),
        &registry.to_config(),
    )
    .unwrap();
    let outcome = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: SOURCE.into(),
            source_display_name: "Folder DAT".into(),
            dat_path: folder,
            dat_kind: DatSourceKind::Folder,
            scan_root: fixture.roms.clone(),
            limits: Default::default(),
            policy: None,
            platform: None,
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    assert!(outcome.catalogue_sha256.is_none());
    Database::open_or_create(&fixture.db)
        .unwrap()
        .persist_library_dat_identities_from_audit(&outcome)
        .unwrap();
    assert!(
        load_library(&fixture.db)
            .unwrap()
            .games
            .iter()
            .all(|g| g.dat_exact.is_none()),
        "a version label and two absent digests cannot prove current catalogue bytes"
    );
    assert!(state_of(&fixture, &format!("{}.gbc", names(1)[0])).is_some());
    assert_saved_summary_is_not_current(&fixture);
}

#[test]
fn current_evidence_regression_same_size_in_place_write_is_not_verified() {
    assert_loose_current_evidence_is_revoked(|path| {
        let mut bytes = std::fs::read(path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(path, bytes).unwrap();
    });
}

#[test]
fn current_evidence_regression_audit_proof_is_bound_to_database_path() {
    let fixture = fixture(&names(1));
    audit_and_save(&fixture);
    let other = fixture._dir.path().join("other.gbc");
    std::fs::write(&other, b"different file").unwrap();
    let conn = rusqlite::Connection::open(&fixture.db).unwrap();
    conn.execute(
        "UPDATE archives SET absolute_path_cached=?1 WHERE id=1",
        [other.as_os_str().as_bytes()],
    )
    .unwrap();
    assert!(
        load_library(&fixture.db)
            .unwrap()
            .games
            .iter()
            .all(|game| game.dat_exact.is_none())
    );
    assert_saved_summary_is_not_current(&fixture);
}
