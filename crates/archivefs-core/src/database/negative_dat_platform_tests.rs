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
        catalogue_sha256: None,
        catalogue_inventory: None,
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
        loose_discs: Default::default(),
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
fn a_game_with_no_system_keeps_no_result_on_the_plain_route() {
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
    // Owner policy 2026-11-26: a game with no system keeps no result unless
    // the same save gives it its system (see the identity-and-platform tests).
    assert_eq!(result.inserted, 0);
    assert_eq!(result.platform_refused, 1);
    assert_eq!(stored_rows(&db), 0);
    assert!(db.library_dat_states().unwrap().is_empty());
}

fn current_platform(db: &Database, id: i64) -> Option<String> {
    db.current_platform_for_archive(id).unwrap()
}

fn verdict_entry(path: &str, verdict: AuditVerdict) -> AuditEntry {
    entry(path, verdict)
}

/// Owner requirement 2026-11-26: identifying a game's system and saving its
/// verified result happen together, in one save.
#[test]
fn an_exact_match_identifies_an_unclassified_game_and_saves_its_result_together() {
    let (_directory, mut db) = fixture();
    let listed = seed(&mut db, 1, None);
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit(
            &outcome(GBA_HEADER, vec![exact(&listed)]),
            1,
        )
        .unwrap();
    assert_eq!(platforms.applied, 1);
    assert_eq!(identities.inserted, 1);
    assert_eq!(
        current_platform(&db, 1)
            .as_deref()
            .and_then(crate::canonical_platform_for_alias),
        crate::canonical_platform_for_alias("Game Boy Advance")
    );
    let saved = db.library_dat_identities_for_item(1).unwrap();
    assert_eq!(saved.len(), 1);
    // The algorithm actually used is what is recorded.
    assert!(matches!(
        &saved[0].verification_state,
        crate::dat::library_identity_summary::DatVerificationState::VerifiedSingleMatch { algorithm }
            if algorithm == "SHA-1"
    ));
}

/// Failure injection: the system assignment fails, so nothing from the
/// audit is kept - no result for the game it would have identified, and no
/// result for the other games of the same audit either.
#[test]
fn a_failed_system_assignment_leaves_no_result_behind() {
    let (_directory, mut db) = fixture();
    let unclassified = seed(&mut db, 1, None);
    let gba = seed(&mut db, 2, Some("Game Boy Advance"));
    db.connection
        .execute_batch(
            "CREATE TEMP TRIGGER inject_platform_failure BEFORE INSERT ON platform_assignments \
             BEGIN SELECT RAISE(ABORT, 'injected platform assignment failure'); END;",
        )
        .unwrap();
    let before = stored_rows(&db);
    let error = db
        .persist_library_dat_identities_and_platforms_from_audit(
            &outcome(GBA_HEADER, vec![exact(&unclassified), exact(&gba)]),
            1,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected platform assignment failure"),
        "{error}"
    );
    assert_eq!(stored_rows(&db), before);
    assert!(db.library_dat_identities_for_item(1).unwrap().is_empty());
    assert!(db.library_dat_identities_for_item(2).unwrap().is_empty());
    assert_eq!(current_platform(&db, 1), None);
    assert!(
        db.connection.is_autocommit(),
        "the save's transaction was closed"
    );
    // With the fault removed the same audit saves both.
    db.connection
        .execute_batch("DROP TRIGGER inject_platform_failure;")
        .unwrap();
    let (identities, _) = db
        .persist_library_dat_identities_and_platforms_from_audit(
            &outcome(GBA_HEADER, vec![exact(&unclassified), exact(&gba)]),
            2,
        )
        .unwrap();
    assert_eq!(identities.inserted, 2);
}

/// Weak, several-candidate and MD5-only matches identify no system, so a
/// game with no system keeps no result from them.
#[test]
fn weak_ambiguous_or_md5_only_matches_never_identify_a_system() {
    let (_directory, mut db) = fixture();
    let paths: Vec<String> = (1..=4).map(|id| seed(&mut db, id, None)).collect();
    let entries = vec![
        verdict_entry(
            &paths[0],
            AuditVerdict::Probable {
                game_name: "Listed".into(),
                rom_name: "Listed.gba".into(),
            },
        ),
        verdict_entry(
            &paths[1],
            AuditVerdict::ExactMultipleCandidates {
                algorithm: "SHA-1",
                count: 2,
                game_names: vec!["Listed".into(), "Listed (Rev 1)".into()],
            },
        ),
        verdict_entry(
            &paths[2],
            AuditVerdict::Exact {
                game_name: "Listed".into(),
                rom_name: "Listed.gba".into(),
                algorithm: "MD5",
            },
        ),
        verdict_entry(
            &paths[3],
            AuditVerdict::FilenameOnly {
                game_name: "Listed".into(),
                rom_name: "Listed.gba".into(),
            },
        ),
    ];
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit(&outcome(GBA_HEADER, entries), 1)
        .unwrap();
    assert_eq!(platforms.applied, 0);
    assert_eq!(identities.inserted + identities.updated, 0);
    assert_eq!(stored_rows(&db), 0);
    for id in 1..=4 {
        assert_eq!(current_platform(&db, id), None, "{id}");
    }
}

/// A catalogue assigned to one system whose header names another identifies
/// nothing, and a known system that differs keeps no result.
#[test]
fn a_catalogue_whose_assignment_contradicts_its_header_identifies_nothing() {
    let (_directory, mut db) = fixture();
    let unclassified = seed(&mut db, 1, None);
    let pc88 = seed(&mut db, 2, Some("NEC PC-8801"));
    let mut audit = outcome(GBA_HEADER, vec![exact(&unclassified), exact(&pc88)]);
    audit.platform = Some("NES".into());
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit(&audit, 1)
        .unwrap();
    assert_eq!(platforms.applied, 0);
    assert_eq!(identities.inserted, 0);
    assert_eq!(identities.platform_refused, 2);
    assert_eq!(current_platform(&db, 1), None);
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

#[test]
fn a_preservation_recording_never_becomes_a_game_dat_match() {
    let (_directory, mut db) = fixture();
    let game = seed(&mut db, 1, Some("Game Boy Advance"));
    let recording = seed(&mut db, 2, Some("Game Boy Advance"));
    db.connection
        .execute(
            "UPDATE archives SET archive_kind = 'preservation_recording' WHERE id = 2",
            [],
        )
        .unwrap();
    let result = db
        .persist_library_dat_identities_from_audit(&outcome(
            GBA_HEADER,
            vec![exact(&game), exact(&recording)],
        ))
        .unwrap();
    assert_eq!(result.preservation_only_skipped, 1);
    assert_eq!(result.inserted, 1);
    assert_eq!(stored_rows(&db), 1);
    assert!(db.library_dat_identities_for_item(2).unwrap().is_empty());
}

// ---- Owner decisions 2026-11-26: identification, correction and review ----

const NES_HEADER: &str = "Nintendo - Nintendo Entertainment System";

/// Gives the game a current system recorded as coming from `source`.
fn set_platform_from(db: &mut Database, id: i64, platform: &str, source: &str) {
    db.connection
        .execute(
            "UPDATE platform_assignments SET is_current = 0 WHERE archive_id = ?1",
            params![id],
        )
        .unwrap();
    db.connection
        .execute(
            "INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) \
             VALUES(?1,?2,?3,1,'2026-01-03')",
            params![id, platform, source],
        )
        .unwrap();
}

fn current_source(db: &Database, id: i64) -> Option<String> {
    db.connection
        .query_row(
            "SELECT source FROM platform_assignments WHERE archive_id = ?1 AND is_current = 1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn assignment_history(db: &Database, id: i64) -> Vec<(String, String, i64)> {
    let mut statement = db
        .connection
        .prepare(
            "SELECT platform, source, is_current FROM platform_assignments \
             WHERE archive_id = ?1 ORDER BY id",
        )
        .unwrap();
    statement
        .query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

fn is_canonical(platform: Option<String>, expected: &str) -> bool {
    platform
        .as_deref()
        .and_then(crate::canonical_platform_for_alias)
        == crate::canonical_platform_for_alias(expected)
}

fn nes_exact(path: &str, algorithm: &'static str) -> AuditEntry {
    entry(
        path,
        AuditVerdict::Exact {
            game_name: "Listed".into(),
            rom_name: "Listed.nes".into(),
            algorithm,
        },
    )
}

fn nes_audit(entries: Vec<AuditEntry>) -> DatAuditOutcome {
    let mut audit = outcome(NES_HEADER, entries);
    audit.platform = Some("NES".into());
    audit
}

/// The real-world case: NES games found in the SNES folder. When corrections
/// are applied (the Platform Corrections slice), the folder's guess is
/// corrected by a strong exact match, the result is saved with the
/// corrected system, and the guess stays in the history.
#[test]
fn a_strong_match_corrects_nes_games_guessed_as_snes_from_their_folder() {
    let (_directory, mut db) = fixture();
    let misfiled = seed(&mut db, 1, None);
    set_platform_from(&mut db, 1, "SNES", "folder_alias");
    let by_extension = seed(&mut db, 2, None);
    set_platform_from(&mut db, 2, "SNES", "registry_detector");
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit_with(
            &nes_audit(vec![
                nes_exact(&misfiled, "SHA-1"),
                nes_exact(&by_extension, "SHA-256/SHA-1"),
            ]),
            1,
            PlatformCorrections::Apply,
        )
        .unwrap();
    assert_eq!(platforms.corrected, 2);
    assert!(platforms.correction_reviews.is_empty());
    assert_eq!(identities.inserted, 2);
    for id in [1, 2] {
        assert!(is_canonical(current_platform(&db, id), "NES"), "{id}");
        assert_eq!(
            current_source(&db, id).as_deref(),
            Some(VERIFIED_DAT_PLATFORM_SOURCE)
        );
        assert_eq!(db.library_dat_identities_for_item(id).unwrap().len(), 1);
    }
    let history = assignment_history(&db, 1);
    assert_eq!(
        history.first(),
        Some(&("SNES".to_string(), "folder_alias".to_string(), 0)),
        "the folder's guess is kept as history: {history:?}"
    );
    // A second run changes nothing.
    let (_, again) = db
        .persist_library_dat_identities_and_platforms_from_audit_with(
            &nes_audit(vec![nes_exact(&misfiled, "SHA-1")]),
            2,
            PlatformCorrections::Apply,
        )
        .unwrap();
    assert_eq!(again.corrected, 0);
    assert_eq!(assignment_history(&db, 1).len(), history.len());
}

/// Verified, chosen, header-derived and unattributable systems are never
/// changed automatically: the disagreement is listed for review, and the
/// other catalogue's result is not saved against them.
#[test]
fn a_confirmed_or_unattributable_system_is_reviewed_not_corrected() {
    use crate::database::PlatformProvenanceClass as Class;
    let (_directory, mut db) = fixture();
    let cases = [
        (1, MANUAL_PLATFORM_SOURCE, Class::Person),
        (2, VERIFIED_DAT_PLATFORM_SOURCE, Class::Verified),
        (3, "header_identity", Class::Header),
        (4, SOURCE_PLATFORM_ASSIGNMENT_SOURCE, Class::Person),
        (5, "an_unrecorded_origin", Class::Unknown),
    ];
    let mut entries = Vec::new();
    for (id, source, _) in cases {
        let path = seed(&mut db, id, None);
        set_platform_from(&mut db, id, "SNES", source);
        entries.push(nes_exact(&path, "SHA-1"));
    }
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit(&nes_audit(entries), 1)
        .unwrap();
    assert_eq!(platforms.corrected, 0);
    assert_eq!(identities.inserted, 0);
    assert_eq!(identities.platform_refused, cases.len());
    assert_eq!(platforms.correction_reviews.len(), cases.len());
    for (id, source, class) in cases {
        assert_eq!(
            current_platform(&db, id).as_deref(),
            Some("SNES"),
            "{source}"
        );
        assert_eq!(current_source(&db, id).as_deref(), Some(source));
        let review = platforms
            .correction_reviews
            .iter()
            .find(|review| review.archive_id == id)
            .unwrap();
        assert_eq!(review.provenance, class, "{source}");
        assert_eq!(review.current_platform, "SNES");
        assert_eq!(review.current_source, source);
        assert_eq!(
            crate::canonical_platform_for_alias(&review.proposed_platform),
            crate::canonical_platform_for_alias("NES")
        );
    }
}

/// Weak, MD5-only, several-candidate and filename evidence never changes a
/// guessed system either, and its result is not saved against it.
#[test]
fn weak_evidence_never_corrects_a_guessed_system() {
    let (_directory, mut db) = fixture();
    let paths: Vec<String> = (1..=4)
        .map(|id| {
            let path = seed(&mut db, id, None);
            set_platform_from(&mut db, id, "SNES", "folder_alias");
            path
        })
        .collect();
    let entries = vec![
        nes_exact(&paths[0], "MD5"),
        nes_exact(&paths[1], "SHA-1/MD5"),
        entry(
            &paths[2],
            AuditVerdict::ExactMultipleCandidates {
                algorithm: "SHA-1",
                count: 2,
                game_names: vec!["Listed".into(), "Listed (Rev 1)".into()],
            },
        ),
        entry(
            &paths[3],
            AuditVerdict::Probable {
                game_name: "Listed".into(),
                rom_name: "Listed.nes".into(),
            },
        ),
    ];
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit(&nes_audit(entries), 1)
        .unwrap();
    assert_eq!(platforms.corrected, 0);
    assert_eq!(platforms.applied, 0);
    assert_eq!(identities.inserted, 0);
    for id in 1..=4 {
        assert_eq!(current_platform(&db, id).as_deref(), Some("SNES"), "{id}");
        assert_eq!(current_source(&db, id).as_deref(), Some("folder_alias"));
        assert_eq!(assignment_history(&db, id).len(), 1);
    }
}

/// MD5 still verifies a game whose system is already the catalogue's.
#[test]
fn md5_still_verifies_a_game_on_the_catalogues_system() {
    let (_directory, mut db) = fixture();
    let path = seed(&mut db, 1, None);
    set_platform_from(&mut db, 1, "NES", "folder_alias");
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit(
            &nes_audit(vec![nes_exact(&path, "MD5")]),
            1,
        )
        .unwrap();
    assert_eq!(identities.inserted, 1);
    assert_eq!(platforms.corrected, 0);
    assert_eq!(current_source(&db, 1).as_deref(), Some("folder_alias"));
}

/// A correction and its result are one change: if the result cannot be
/// saved, the folder's original assignment is current again, unchanged.
#[test]
fn a_failed_result_save_undoes_the_correction() {
    let (_directory, mut db) = fixture();
    let path = seed(&mut db, 1, None);
    set_platform_from(&mut db, 1, "SNES", "folder_alias");
    let before = assignment_history(&db, 1);
    db.connection
        .execute_batch(
            "CREATE TEMP TRIGGER inject_identity_failure BEFORE INSERT ON library_dat_identities \
             BEGIN SELECT RAISE(ABORT, 'injected identity failure'); END;",
        )
        .unwrap();
    let error = db
        .persist_library_dat_identities_and_platforms_from_audit_with(
            &nes_audit(vec![nes_exact(&path, "SHA-1")]),
            1,
            PlatformCorrections::Apply,
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("injected identity failure"),
        "{error}"
    );
    assert_eq!(assignment_history(&db, 1), before);
    assert_eq!(stored_rows(&db), 0);
    assert!(db.connection.is_autocommit());
}

/// F2: a preservation recording keeps no result, so it gets no system.
#[test]
fn a_preservation_recording_gets_no_system_from_a_match() {
    let (_directory, mut db) = fixture();
    let recording = seed(&mut db, 1, None);
    db.connection
        .execute(
            "UPDATE archives SET archive_kind = 'preservation_recording' WHERE id = 1",
            [],
        )
        .unwrap();
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit(
            &outcome(GBA_HEADER, vec![exact(&recording)]),
            1,
        )
        .unwrap();
    assert_eq!(identities.preservation_only_skipped, 1);
    assert_eq!(platforms.applied, 0);
    assert_eq!(current_platform(&db, 1), None);
    assert!(assignment_history(&db, 1).is_empty());
}

fn arcade_audit(
    ecosystem: crate::dat::model::DatEcosystem,
    entries: Vec<AuditEntry>,
) -> DatAuditOutcome {
    let mut audit = outcome("MAME", entries);
    audit.catalogue_ecosystem = Some(ecosystem);
    audit.platform = None;
    audit
}

/// F3: an unassigned MAME arcade or FBNeo catalogue speaks only for the
/// arcade systems; an unassigned MAME software list speaks for none.
#[test]
fn an_unassigned_arcade_catalogue_speaks_only_for_arcade_systems() {
    use crate::dat::model::DatEcosystem;
    for ecosystem in [DatEcosystem::MAMEArcade, DatEcosystem::FBNeo] {
        let (_directory, mut db) = fixture();
        let arcade = seed(&mut db, 1, Some("Arcade"));
        let neogeo = seed(&mut db, 2, Some("NeoGeo"));
        let psp = seed(&mut db, 3, Some("PSP"));
        let nes = seed(&mut db, 4, Some("NES"));
        let unclassified = seed(&mut db, 5, None);
        let (identities, platforms) = db
            .persist_library_dat_identities_and_platforms_from_audit(
                &arcade_audit(
                    ecosystem,
                    vec![
                        exact(&arcade),
                        exact(&neogeo),
                        exact(&psp),
                        exact(&nes),
                        exact(&unclassified),
                    ],
                ),
                1,
            )
            .unwrap();
        assert_eq!(identities.inserted, 2, "{ecosystem:?}");
        assert_eq!(identities.platform_refused, 3, "{ecosystem:?}");
        assert_eq!(db.library_dat_identities_for_item(1).unwrap().len(), 1);
        assert_eq!(db.library_dat_identities_for_item(2).unwrap().len(), 1);
        for id in [3, 4, 5] {
            assert!(
                db.library_dat_identities_for_item(id).unwrap().is_empty(),
                "{id}"
            );
        }
        // It names machines, not one system, so it identifies nothing.
        assert_eq!(platforms.applied, 0);
        assert_eq!(current_platform(&db, 5), None);
    }
    let (_directory, mut db) = fixture();
    let arcade = seed(&mut db, 1, Some("Arcade"));
    let result = db
        .persist_library_dat_identities_from_audit(&arcade_audit(
            DatEcosystem::MAMESoftwareList,
            vec![exact(&arcade)],
        ))
        .unwrap();
    assert_eq!(
        result.inserted, 0,
        "an unassigned software list needs its system"
    );
    assert_eq!(result.platform_refused, 1);
}

/// Independent review 2 (N3): a system set by the save whose result row is
/// not written fails the save, and the rollback restores everything,
/// including what the assignment triggers recorded (the catalogue epoch).
#[test]
fn a_system_without_its_saved_result_fails_the_whole_save() {
    let (_directory, mut db) = fixture();
    let unclassified = seed(&mut db, 1, None);
    let other = seed(&mut db, 2, Some("Game Boy Advance"));
    let epoch = |db: &Database| -> i64 {
        db.connection
            .query_row(
                "SELECT revision FROM catalogue_health_epoch WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let before = epoch(&db);
    crate::database::test_seams::set_skip_identity_row(Some(1));
    let error = db
        .persist_library_dat_identities_and_platforms_from_audit(
            &outcome(GBA_HEADER, vec![exact(&unclassified), exact(&other)]),
            1,
        )
        .unwrap_err();
    crate::database::test_seams::set_skip_identity_row(None);
    assert!(
        error
            .to_string()
            .contains("was given a system but its verification result was not saved"),
        "{error}"
    );
    assert_eq!(current_platform(&db, 1), None);
    assert!(assignment_history(&db, 1).is_empty());
    assert_eq!(stored_rows(&db), 0, "nothing from the audit is kept");
    assert_eq!(
        epoch(&db),
        before,
        "the assignment trigger's epoch bump is rolled back too"
    );
    assert!(db.connection.is_autocommit());
}

/// Independent review 2 (N1/T2): a game with no system is told what is wrong
/// with the catalogue before being told its match was not strong enough.
#[test]
fn a_game_with_no_system_is_given_the_catalogues_real_reason() {
    use crate::dat::model::DatEcosystem;
    let (_directory, mut db) = fixture();
    let paths: Vec<String> = (1..=6).map(|id| seed(&mut db, id, None)).collect();
    let save = |db: &mut Database, audit: DatAuditOutcome| {
        db.persist_library_dat_identities_and_platforms_from_audit(&audit, 1)
            .unwrap()
            .0
            .platform_refusals
    };
    let mut unassigned = outcome("Homebrew list", vec![exact(&paths[0])]);
    unassigned.platform = None;
    assert_eq!(save(&mut db, unassigned).catalogue_unassigned, 1);
    let mut contradicts = outcome(GBA_HEADER, vec![exact(&paths[1])]);
    contradicts.platform = Some("NES".into());
    assert_eq!(save(&mut db, contradicts).catalogue_contradicts, 1);
    let mut several = outcome(GBA_HEADER, vec![exact(&paths[2])]);
    several.catalogue_names.push(NES_HEADER.into());
    several.platform = None;
    assert_eq!(save(&mut db, several).no_system_several_systems, 1);
    let arcade = arcade_audit(DatEcosystem::MAMEArcade, vec![exact(&paths[3])]);
    assert_eq!(save(&mut db, arcade).not_arcade, 1);
    // A catalogue for one system: only then is the match itself the reason.
    let weak = outcome(
        GBA_HEADER,
        vec![entry(
            &paths[4],
            AuditVerdict::Exact {
                game_name: "Listed".into(),
                rom_name: "Listed.gba".into(),
                algorithm: "MD5",
            },
        )],
    );
    let counts = save(&mut db, weak);
    assert_eq!(counts.no_system, 1);
    let reasons = counts.reasons();
    assert_eq!(reasons.len(), 1);
    assert!(reasons[0].1.contains("did not give it one"), "{reasons:?}");
    for id in 1..=5 {
        assert_eq!(current_platform(&db, id), None, "{id}");
    }
}

/// Owner decision 2026-11-26: until Platform Corrections has a preview and
/// Undo, the DAT page save only proposes corrections. NES games in the SNES
/// folder keep their system and get no result; the proposal names the
/// evidence; verification of games already on NES is unaffected.
#[test]
fn by_default_a_guessed_system_is_proposed_for_correction_not_changed() {
    let (_directory, mut db) = fixture();
    let misfiled = seed(&mut db, 1, None);
    set_platform_from(&mut db, 1, "SNES", "folder_alias");
    let on_nes = seed(&mut db, 2, None);
    set_platform_from(&mut db, 2, "NES", "folder_alias");
    let history = assignment_history(&db, 1);
    let (identities, platforms) = db
        .persist_library_dat_identities_and_platforms_from_audit(
            &nes_audit(vec![
                nes_exact(&misfiled, "SHA-1"),
                nes_exact(&on_nes, "SHA-1"),
            ]),
            1,
        )
        .unwrap();
    assert_eq!(platforms.corrected, 0);
    assert!(platforms.correction_reviews.is_empty());
    assert_eq!(platforms.proposed_corrections.len(), 1);
    let proposal = &platforms.proposed_corrections[0];
    assert_eq!(proposal.archive_id, 1);
    assert_eq!(proposal.current_platform, "SNES");
    assert_eq!(proposal.current_source, "folder_alias");
    assert!(is_canonical(
        Some(proposal.proposed_platform.clone()),
        "NES"
    ));
    assert_eq!(proposal.algorithm, "SHA-1");
    assert_eq!(proposal.provenance, PlatformProvenanceClass::Inferred);
    assert_eq!(current_platform(&db, 1).as_deref(), Some("SNES"));
    assert_eq!(current_source(&db, 1).as_deref(), Some("folder_alias"));
    assert_eq!(
        assignment_history(&db, 1),
        history,
        "nothing about the guess changed"
    );
    assert!(db.library_dat_identities_for_item(1).unwrap().is_empty());
    // The game already on NES is verified as before.
    assert_eq!(identities.inserted, 1);
    assert_eq!(db.library_dat_identities_for_item(2).unwrap().len(), 1);
}
