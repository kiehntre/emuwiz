//! A saved "not in this catalogue" verdict is evidence only about items on the
//! catalogue's own platform (`crate::dat::identity::negative_dat_result_applies`).
use super::*;
use crate::attention::AttentionCategory;
use crate::dat::audit::{AuditEntry, AuditReport, AuditSummary, AuditVerdict};
use crate::dat::sources::audit_run::DatAuditOutcome;

const GBA_HEADER: &str = "Nintendo - Game Boy Advance";

fn fixture() -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::open_or_create(directory.path().join("library.sqlite3")).unwrap();
    db.connection
        .execute_batch(
            "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at,last_scan_status) \
             VALUES(1,CAST('/roms' AS BLOB),'2026-01-01','2026-01-01','success');",
        )
        .unwrap();
    (directory, db)
}

fn seed(db: &mut Database, id: i64, platform: Option<&str>) -> String {
    let path = format!("/roms/game{id}.zip");
    db.connection
        .execute(
            "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,\
             archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) \
             VALUES(?1,1,CAST(?2 AS BLOB),CAST(?3 AS BLOB),CAST(?2 AS BLOB),'zip',?2,?2,\
             '2026-01-01','2026-01-02','2026-01-01','2026-01-02')",
            params![id, format!("game{id}.zip"), path],
        )
        .unwrap();
    set_platform(db, id, platform);
    path
}

/// Replaces the item's current platform; `None` leaves it unclassified.
fn set_platform(db: &mut Database, id: i64, platform: Option<&str>) {
    db.connection
        .execute(
            "UPDATE platform_assignments SET is_current = 0 WHERE archive_id = ?1",
            params![id],
        )
        .unwrap();
    if let Some(platform) = platform {
        db.connection
            .execute(
                "INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) \
                 VALUES(?1,?2,'manual',1,'2026-01-03')",
                params![id, platform],
            )
            .unwrap();
    }
}

fn entry(path: &str, verdict: AuditVerdict) -> AuditEntry {
    AuditEntry {
        local_path: path.to_string(),
        local_filename: path.rsplit('/').next().unwrap().to_string(),
        verdict,
    }
}

fn exact(path: &str) -> AuditEntry {
    entry(
        path,
        AuditVerdict::Exact {
            game_name: "Listed".into(),
            rom_name: "Listed.gba".into(),
            algorithm: "SHA-1",
        },
    )
}

fn outcome(header: &str, entries: Vec<AuditEntry>) -> DatAuditOutcome {
    DatAuditOutcome {
        population: Default::default(),
        source_id: "158".to_string(),
        source_display_name: format!("No-Intro: {header}"),
        dat_path: "/catalogues/158.dat".to_string(),
        scan_root: "/roms".to_string(),
        catalogue_names: vec![header.to_string()],
        catalogue_entries: 1,
        catalogue_roms: 1,
        catalogue_version: Some("20260825-070327".to_string()),
        catalogue_author: None,
        catalogue_homepage: None,
        catalogue_ecosystem: Some(crate::dat::model::DatEcosystem::NoIntro),
        catalogue_variant: None,
        unreadable_catalogues: Vec::new(),
        report: AuditReport {
            summary: AuditSummary::default(),
            entries,
        },
        evidence_sources: Vec::new(),
        archives: Vec::new(),
        sets: Vec::new(),
        unhashed: Vec::new(),
        files_scanned: 1,
        bytes_hashed: 9,
        archive_bytes_hashed: 0,
        truncated: false,
        policy: None,
        content: Default::default(),
        platform: Some("Game Boy Advance".to_string()),
        cache: Default::default(),
        known_hashes: Default::default(),
    }
}

fn stored_rows(db: &Database) -> i64 {
    db.connection
        .query_row("SELECT COUNT(*) FROM library_dat_identities", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn dat_attention(db: &Database) -> u64 {
    db.attention_snapshot()
        .unwrap()
        .items()
        .filter(|item| item.category == AttentionCategory::Dat)
        .map(|item| item.affected_count)
        .sum()
}

#[test]
fn the_catalogue_header_decides_which_platform_a_negative_speaks_for() {
    use crate::dat::identity::negative_dat_result_applies;
    use crate::dat::model::DatEcosystem;
    let applies = |names: &[String], platform: Option<&str>| {
        negative_dat_result_applies(names, Some(DatEcosystem::NoIntro), platform)
    };
    let gba = vec![GBA_HEADER.to_string()];
    assert!(applies(&gba, Some("Game Boy Advance")));
    assert!(applies(&gba, Some("GBA")), "an alias of the same platform");
    assert!(!applies(&gba, None), "an unclassified item");
    assert!(!applies(&gba, Some("")), "an empty assignment");
    assert!(!applies(&gba, Some("NEC PC-8801")), "another platform");
    assert!(
        !applies(&gba, Some("Game Boy")),
        "a related but different platform"
    );
    assert!(
        !applies(&[], Some("Game Boy Advance")),
        "no header recorded"
    );
    assert!(
        !applies(&["Unsorted Collection".to_string()], Some("Arcade")),
        "a header that names no platform fails closed"
    );
    assert!(
        !negative_dat_result_applies(
            &["MAME".to_string()],
            Some(DatEcosystem::MAMEArcade),
            Some("Arcade")
        ),
        "a MAME catalogue header never names a platform, as for a parsed DAT"
    );
}

#[test]
fn an_unclassified_or_other_platform_no_match_is_not_saved_but_matches_are() {
    let (_directory, mut db) = fixture();
    let unclassified = seed(&mut db, 1, None);
    let pc88 = seed(&mut db, 2, Some("NEC PC-8801"));
    let listed = seed(&mut db, 3, None);
    let result = db
        .persist_library_dat_identities_from_audit(&outcome(
            GBA_HEADER,
            vec![
                entry(&unclassified, AuditVerdict::NotInDat),
                entry(&pc88, AuditVerdict::NotInDat),
                exact(&listed),
            ],
        ))
        .unwrap();
    assert_eq!(result.negative_platform_unproven_skipped, 2);
    assert_eq!(result.inserted, 1, "an exact match is saved for any item");
    assert_eq!(stored_rows(&db), 1);
    assert!(db.library_dat_identities_for_item(1).unwrap().is_empty());
    assert!(db.library_dat_identities_for_item(2).unwrap().is_empty());
    assert_eq!(
        db.library_dat_states().unwrap(),
        HashMap::from([(3, LibraryDatState::Verified)])
    );
}

#[test]
fn a_same_platform_no_match_is_saved_and_shown_as_checked() {
    let (_directory, mut db) = fixture();
    let gba = seed(&mut db, 1, Some("Game Boy Advance"));
    let result = db
        .persist_library_dat_identities_from_audit(&outcome(
            GBA_HEADER,
            vec![entry(&gba, AuditVerdict::NotInDat)],
        ))
        .unwrap();
    assert_eq!(result.negative_platform_unproven_skipped, 0);
    assert_eq!(result.inserted, 1);
    let rows = db.library_dat_identities_for_item(1).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].carries_identity());
    assert_eq!(
        db.library_dat_states().unwrap().get(&1),
        Some(&LibraryDatState::NoMatch)
    );
    assert_eq!(dat_attention(&db), 1);
}

#[test]
fn a_historical_negative_is_kept_but_only_counts_while_the_platforms_agree() {
    let (_directory, mut db) = fixture();
    let path = seed(&mut db, 1, Some("Game Boy Advance"));
    db.persist_library_dat_identities_from_audit(&outcome(
        GBA_HEADER,
        vec![entry(&path, AuditVerdict::NotInDat)],
    ))
    .unwrap();
    assert_eq!(db.library_dat_states().unwrap().len(), 1);

    // Reclassified: the saved row stays as history but is not evidence.
    for platform in [None, Some("NEC PC-8801")] {
        set_platform(&mut db, 1, platform);
        assert_eq!(stored_rows(&db), 1, "history is never deleted");
        assert!(db.library_dat_identities_for_item(1).unwrap().is_empty());
        assert!(db.library_dat_states().unwrap().is_empty());
        assert_eq!(dat_attention(&db), 0);
        let read_only = Database::open_read_only(db.path()).unwrap();
        assert_eq!(dat_attention(&read_only), 0);
    }

    set_platform(&mut db, 1, Some("GBA"));
    assert_eq!(db.library_dat_identities_for_item(1).unwrap().len(), 1);
    assert_eq!(dat_attention(&db), 1);
}

#[test]
fn a_stale_saved_result_is_not_current_evidence() {
    let (_directory, mut db) = fixture();
    let gba = seed(&mut db, 1, Some("Game Boy Advance"));
    let listed = seed(&mut db, 2, Some("Game Boy Advance"));
    db.persist_library_dat_identities_from_audit(&outcome(
        GBA_HEADER,
        vec![entry(&gba, AuditVerdict::NotInDat), exact(&listed)],
    ))
    .unwrap();
    assert_eq!(db.library_dat_states().unwrap().len(), 2);
    db.mark_library_dat_identities_stale_for_source("158")
        .unwrap();
    assert!(db.library_dat_states().unwrap().is_empty());
}

#[test]
fn no_usable_evidence_is_not_shown_as_a_catalogue_no_match() {
    let (_directory, mut db) = fixture();
    let gba = seed(&mut db, 1, Some("Game Boy Advance"));
    db.persist_library_dat_identities_from_audit(&outcome(
        GBA_HEADER,
        vec![entry(&gba, AuditVerdict::NoUsableEvidence)],
    ))
    .unwrap();
    assert_eq!(db.library_dat_identities_for_item(1).unwrap().len(), 1);
    // No hash was compared: that is "not checked", never "not listed".
    assert!(db.library_dat_states().unwrap().is_empty());
}

#[test]
fn platform_coverage_counts_only_negatives_from_that_platforms_catalogue() {
    use crate::dat::coverage::CoverageSourceScope;
    let (_directory, mut db) = fixture();
    let gba = seed(&mut db, 1, Some("Game Boy Advance"));
    db.persist_library_dat_identities_from_audit(&outcome(
        GBA_HEADER,
        vec![entry(&gba, AuditVerdict::NotInDat)],
    ))
    .unwrap();
    let scope = CoverageSourceScope::assigned_to("Game Boy Advance", Some("20260825-070327"));
    let coverage = db
        .platform_dat_coverage("Game Boy Advance", "158", scope)
        .unwrap();
    assert_eq!((coverage.checked, coverage.unmatched), (1, 1));

    // The same saved row once the item is on another platform: not checked
    // by that catalogue at all.
    set_platform(&mut db, 1, Some("NEC PC-8801"));
    let scope = CoverageSourceScope::assigned_to("NEC PC-8801", Some("20260825-070327"));
    let coverage = db
        .platform_dat_coverage("NEC PC-8801", "158", scope)
        .unwrap();
    assert_eq!(coverage.owned_applicable, 1);
    assert_eq!((coverage.checked, coverage.unmatched), (0, 0));
}
