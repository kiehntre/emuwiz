use super::*;
use rusqlite::params;

fn fixture() -> (tempfile::TempDir, Database, Vec<DatAuthoritySource>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open_or_create(dir.path().join("disposable.sqlite3")).unwrap();
    db.connection.execute("INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,x'01','now','now')", []).unwrap();
    let sources = vec![DatAuthoritySource {
        id: "dat".into(),
        name: "Imported DAT".into(),
        platform: Some("snes".into()),
        enabled: true,
        revision: Some("1".into()),
        ..Default::default()
    }];
    inventory(&db, "dat", &["A", "B"]);
    (dir, db, sources)
}

fn inventory(db: &Database, source: &str, names: &[&str]) {
    db.connection
        .execute(
            "INSERT INTO dat_expected_inventory_meta VALUES(?1,'1','\"no_intro\"',?2,0,'now','now')",
            params![source, names.len() as i64],
        )
        .unwrap();
    for name in names {
        db.connection.execute("INSERT INTO dat_expected_entries(dat_source_id,canonical_identity,display_name,metadata_json,created_at,updated_at) VALUES(?1,?2,?2,'{\"rom_count\":1}','now','now')", params![source,name]).unwrap();
    }
}

fn local(db: &Database, id: i64, state: &str, name: Option<&str>, candidates: &[&str]) {
    db.connection.execute("INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,size_bytes,first_seen_at,last_seen_at,created_at,updated_at) VALUES(?1,1,?2,?2,?2,'Zip','test','test',10,'now','now','now','now')", params![id, id.to_string().as_bytes()]).unwrap();
    db.connection.execute("INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) VALUES(?1,'snes','manual',1,'now')", [id]).unwrap();
    if state.is_empty() {
        return;
    }
    let facts = serde_json::json!({"canonical":{"canonical_dat_name":name},"ambiguous_candidates":candidates,"audited_hashes":{"size_bytes":10},"source":{"variant":"parent_clone"}}).to_string();
    db.connection.execute("INSERT INTO library_dat_identities(archive_id,dat_source_id,source_revision,verification_state,completeness,audited_at,facts_json,created_at,updated_at) VALUES(?1,'dat','1',?2,'exhaustive','now',?3,'now','now')", params![id,state,facts.as_bytes()]).unwrap();
}

#[test]
fn complete_platform_is_scoped_to_import_not_publisher_currency() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    local(&db, 2, "verified_single_match", Some("B"), &[]);
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].state, CompletenessState::Complete);
    assert_eq!(v.collections[0].counts.missing, Some(0));
    assert_eq!(v.authorities[0].freshness, AuthorityFreshness::Unknown);
}

#[test]
fn missing_and_extra_are_not_the_same_as_unverified() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    local(&db, 2, "no_match", None, &[]);
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].state, CompletenessState::Incomplete);
    assert_eq!(v.collections[0].counts.missing, Some(1));
    assert_eq!(v.collections[0].counts.extra, 1);
    local(&db, 3, "", None, &[]);
    assert_eq!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0]
            .counts
            .missing,
        None
    );
}

#[test]
fn ambiguous_candidates_are_not_missing() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "ambiguous_multiple_candidates", None, &["A", "B"]);
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].state, CompletenessState::Ambiguous);
    assert_eq!(v.collections[0].counts.ambiguous, 1);
    assert_eq!(v.collections[0].counts.pending_entries, Some(2));
    assert_eq!(v.collections[0].counts.missing, Some(0));
}

#[test]
fn no_dat_and_unlinked_dat_never_provide_denominator() {
    let (_dir, db, mut sources) = fixture();
    local(&db, 1, "", None, &[]);
    sources[0].platform = None;
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].state, CompletenessState::NoAuthority);
    assert_eq!(v.collections[0].counts.expected, None);
    assert!(!v.authorities[0].inventory_usable);
}

#[test]
fn stale_source_and_size_drift_refuse_completeness() {
    let (_dir, db, mut sources) = fixture();
    sources[0].revision = Some("2".into());
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.authorities[0].freshness, AuthorityFreshness::Stale);
    assert_eq!(v.collections[0].state, CompletenessState::PartialAuthority);
    sources[0].revision = Some("1".into());
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    db.connection
        .execute("UPDATE archives SET size_bytes=11", [])
        .unwrap();
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].counts.verified_local, 0);
    assert_eq!(v.collections[0].counts.missing, None);
}

#[test]
fn recorded_invalid_source_never_claims_completeness() {
    let (_dir, db, mut sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    local(&db, 2, "verified_single_match", Some("B"), &[]);
    sources[0].validation_problem = Some("Backup DAT failed parsing at last validation.".into());
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].state, CompletenessState::PartialAuthority);
    assert_eq!(v.collections[0].counts.expected, None);
}

#[test]
fn duplicate_archives_do_not_fill_distinct_entries() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    local(&db, 2, "verified_single_match", Some("A"), &[]);
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].counts.matched, Some(1));
    assert_eq!(v.collections[0].counts.missing, Some(1));
}

#[test]
fn incomplete_inventory_and_overlapping_sources_are_not_summed() {
    let (_dir, db, mut sources) = fixture();
    inventory(&db, "dat2", &["A", "B"]);
    let mut second = sources[0].clone();
    second.id = "dat2".into();
    sources.push(second);
    assert_eq!(
        db.dat_authority_dashboard(&sources)
            .unwrap()
            .collections
            .len(),
        2
    );
    db.connection.execute("UPDATE dat_expected_inventory_meta SET duplicate_names_skipped=1 WHERE dat_source_id='dat'",[]).unwrap();
    assert_eq!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0].state,
        CompletenessState::PartialAuthority
    );
}

#[test]
fn bios_gap_comes_only_from_persisted_dependency_verdict() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    db.connection.execute("INSERT INTO dat_set_audit_results(id,archive_id,archive_path,source_id,game_name,platform,set_state_json,dependency_state_json,audited_at,dat_revision) VALUES(1,1,x'01','dat','A','snes','\"complete\"','\"missing\"','now','1')",[]).unwrap();
    db.connection.execute("INSERT INTO dat_set_audit_dependencies(result_id,dependency_kind,dependency_outcome,target_json) VALUES(1,'bios','missing','\"firmware\"')",[]).unwrap();
    assert_eq!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0]
            .counts
            .bios_missing,
        Some(1)
    );
    db.connection
        .execute("UPDATE dat_set_audit_results SET stale=1", [])
        .unwrap();
    assert_eq!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0]
            .counts
            .bios_missing,
        None
    );
}

#[test]
fn refresh_add_remove_unique_id_rename_no_invented_hash_change() {
    let (_dir, db, _sources) = fixture();
    inventory(&db, "new", &["C", "D"]);
    db.connection.execute("UPDATE dat_expected_entries SET metadata_json='{\"dat_game_id\":\"stable\"}' WHERE canonical_identity IN ('A','C')",[]).unwrap();
    let diff = db.compare_dat_authorities("dat", "new", true).unwrap();
    assert_eq!((diff.added, diff.removed, diff.renamed), (1, 1, Some(1)));
    assert_eq!(
        db.compare_dat_authorities("dat", "new", false)
            .unwrap()
            .renamed,
        None
    );
    assert_eq!(diff.hash_changed, None);
    assert_eq!(diff.bios_requirements_changed, None);
    assert!(db.compare_dat_authorities("absent", "new", false).is_err());
}

#[test]
fn opening_dashboard_and_comparison_are_read_only() {
    let (dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    db.close().unwrap();
    let path = dir.path().join("disposable.sqlite3");
    let before = std::fs::read(&path).unwrap();
    let db = Database::open_read_only(&path).unwrap();
    db.dat_authority_dashboard(&sources).unwrap();
    db.compare_dat_authorities("dat", "dat", false).unwrap();
    assert_eq!(db.connection.total_changes(), 0);
    db.close().unwrap();
    assert_eq!(before, std::fs::read(path).unwrap());
}

#[test]
fn multi_member_entries_require_set_and_dependency_proof() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    local(&db, 2, "verified_single_match", Some("B"), &[]);
    db.connection.execute("UPDATE dat_expected_entries SET metadata_json='{\"rom_count\":2}' WHERE canonical_identity='A'",[]).unwrap();
    let view = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(view.collections[0].counts.matched, Some(1));
    assert_eq!(view.collections[0].counts.pending_entries, Some(1));
    assert_ne!(view.collections[0].state, CompletenessState::Complete);
    db.connection.execute("INSERT INTO dat_set_audit_results(id,archive_id,archive_path,source_id,game_name,platform,set_state_json,dependency_state_json,audited_at,dat_revision) VALUES(1,1,x'01','dat','A','snes','\"complete\"','\"not_applicable\"','now','1')",[]).unwrap();
    assert_eq!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0].state,
        CompletenessState::Complete
    );
    db.connection
        .execute("UPDATE dat_set_audit_results SET dat_revision='old'", [])
        .unwrap();
    assert_ne!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0].state,
        CompletenessState::Complete
    );
}

#[test]
fn arcade_never_uses_flat_file_proof_and_absent_files_do_not_match() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    local(&db, 2, "verified_single_match", Some("B"), &[]);
    let ecosystem = serde_json::to_string(&crate::dat::model::DatEcosystem::MAMEArcade).unwrap();
    db.connection
        .execute(
            "UPDATE dat_expected_inventory_meta SET ecosystem=?1",
            [ecosystem],
        )
        .unwrap();
    assert_eq!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0]
            .counts
            .matched,
        Some(0)
    );
    db.connection
        .execute("UPDATE archives SET last_verified_missing_at='now'", [])
        .unwrap();
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].counts.local, 0);
    assert_eq!(v.collections[0].counts.missing, Some(2));
}

#[test]
fn negative_set_proof_overrides_single_member_match_without_order_dependence() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    local(&db, 2, "verified_single_match", Some("B"), &[]);
    db.connection.execute("INSERT INTO dat_set_audit_results(id,archive_id,archive_path,source_id,game_name,platform,set_state_json,dependency_state_json,audited_at,dat_revision) VALUES(1,1,x'01','dat','A','snes','\"incomplete\"','\"unsupported\"','now','1')",[]).unwrap();
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].counts.matched, Some(1));
    assert_eq!(v.collections[0].counts.pending_entries, Some(1));
    assert_ne!(v.collections[0].state, CompletenessState::Complete);
    local(&db, 3, "verified_single_match", Some("A"), &[]);
    db.connection.execute("INSERT INTO dat_set_audit_results(id,archive_id,archive_path,source_id,game_name,platform,set_state_json,dependency_state_json,audited_at,dat_revision) VALUES(2,3,x'03','dat','A','snes','\"complete\"','\"not_applicable\"','now','1')",[]).unwrap();
    assert_eq!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0].state,
        CompletenessState::Complete
    );
    db.connection.execute("UPDATE dat_set_audit_results SET set_state_json=CASE id WHEN 1 THEN '\"complete\"' ELSE '\"incomplete\"' END, dependency_state_json=CASE id WHEN 1 THEN '\"not_applicable\"' ELSE '\"unsupported\"' END",[]).unwrap();
    assert_eq!(
        db.dat_authority_dashboard(&sources).unwrap().collections[0].state,
        CompletenessState::Complete
    );
}

#[test]
fn older_set_identity_cannot_fill_another_current_entry() {
    let (_dir, db, sources) = fixture();
    local(&db, 1, "verified_single_match", Some("A"), &[]);
    db.connection.execute("UPDATE dat_expected_entries SET metadata_json='{\"rom_count\":2}' WHERE canonical_identity='B'",[]).unwrap();
    db.connection.execute("INSERT INTO dat_set_audit_results(id,archive_id,archive_path,source_id,game_name,platform,set_state_json,dependency_state_json,audited_at,dat_revision) VALUES(1,1,x'01','dat','B','snes','\"complete\"','\"not_applicable\"','now','1')",[]).unwrap();
    let v = db.dat_authority_dashboard(&sources).unwrap();
    assert_eq!(v.collections[0].counts.matched, Some(1));
    assert_eq!(v.collections[0].counts.missing, Some(1));
    assert_ne!(v.collections[0].state, CompletenessState::Complete);
}

#[test]
#[ignore = "explicit 100k disposable performance measurement"]
fn performance_100k_catalogue_and_inventory() {
    let (_dir, db, sources) = fixture();
    let tx = db.connection.unchecked_transaction().unwrap();
    for id in 1..=100_000 {
        local(
            &db,
            id,
            "verified_single_match",
            Some(&format!("Game {id}")),
            &[],
        );
    }
    db.connection
        .execute("DELETE FROM dat_expected_entries", [])
        .unwrap();
    db.connection.execute("INSERT INTO dat_expected_entries(dat_source_id,canonical_identity,display_name,metadata_json,created_at,updated_at) SELECT 'dat','Game ' || id,'Game ' || id,'{\"rom_count\":1}','now','now' FROM archives",[]).unwrap();
    db.connection
        .execute(
            "UPDATE dat_expected_inventory_meta SET entry_count=100000",
            [],
        )
        .unwrap();
    tx.commit().unwrap();
    for run in 1..=5 {
        let start = std::time::Instant::now();
        let v = db.dat_authority_dashboard(&sources).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(v.collections[0].counts.matched, Some(100000));
        assert_eq!(v.collections[0].state, CompletenessState::Complete);
        println!(
            "DAT_DASHBOARD_100K run={run} elapsed_ms={:.1}",
            elapsed.as_secs_f64() * 1000.0
        );
    }
}
