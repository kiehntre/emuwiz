use super::*;
use crate::attention::*;

fn fixture(count: usize) -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::open_or_create(directory.path().join("attention.sqlite3")).unwrap();
    db.connection.execute_batch("INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at,last_scan_status) VALUES(1,x'2f616273656e74','2026-01-01','2026-01-01','success');").unwrap();
    db.connection.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < ?1) INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) SELECT i,1,CAST(i AS BLOB),CAST('/absent/'||i AS BLOB),CAST(i AS BLOB),'zip','Game','Game '||(i%2),'2026-01-01','2026-01-02','2026-01-01','2026-01-02' FROM n", [count as i64]).unwrap();
    (directory, db)
}

#[test]
fn attention_catalogue_100k_is_bounded_read_only_and_does_not_walk_paths() {
    let (_directory, db) = fixture(100_000);
    let before = std::fs::read(db.path()).unwrap();
    let read_only = Database::open_read_only(db.path()).unwrap();
    let snapshot = read_only.attention_snapshot().unwrap();
    assert_eq!(snapshot.source_rows, 100_001);
    assert_eq!(snapshot.query_count, 8);
    assert!(snapshot.items().count() < 10);
    assert_eq!(
        snapshot
            .items()
            .find(|i| i.category == AttentionCategory::Identity)
            .unwrap()
            .affected_count,
        100_000
    );
    assert_eq!(
        snapshot
            .items()
            .find(|i| i.category == AttentionCategory::Duplicates)
            .unwrap()
            .affected_count,
        2
    );
    assert_eq!(read_only.connection.total_changes(), 0);
    assert_eq!(std::fs::read(db.path()).unwrap(), before);
    eprintln!(
        "ATTENTION_100K rows={} queries={} items={} query_ms={} page_size={}",
        snapshot.source_rows,
        snapshot.query_count,
        snapshot.items().count(),
        snapshot.query_millis,
        ATTENTION_PAGE_SIZE
    );
}

#[test]
fn attention_dat_mismatch_and_discovery_use_saved_state_and_auto_resolve() {
    let (_directory, db) = fixture(2);
    db.connection.execute_batch("INSERT INTO library_dat_identities(archive_id,dat_source_id,verification_state,completeness,audited_at,facts_json,created_at,updated_at) VALUES(1,'trusted-dat','conflicting','exhaustive','2026-01-02',x'7b7d','2026-01-02','2026-01-02'); INSERT INTO scan_runs(started_at,finished_at,triggered_by,status,skipped_unsupported_extension,skipped_ambiguous_platform) VALUES('2026-01-01','2026-01-02','test','completed',12,3);").unwrap();
    let snapshot = db.attention_snapshot().unwrap();
    assert!(
        snapshot
            .items()
            .any(|i| i.category == AttentionCategory::Dat && i.summary.contains("conflicts"))
    );
    assert!(
        snapshot
            .items()
            .any(|i| i.category == AttentionCategory::Unsupported
                && i.severity == AttentionSeverity::Warning
                && i.affected_count == 12)
    );
    assert!(
        snapshot
            .items()
            .any(|i| i.title.contains("ambiguous platform") && i.affected_count == 3)
    );
    db.connection.execute_batch("UPDATE library_dat_identities SET verification_state='verified_single_match'; INSERT INTO scan_runs(started_at,finished_at,triggered_by,status) VALUES('2026-01-03','2026-01-03','test','completed');").unwrap();
    let resolved = db.attention_snapshot().unwrap();
    assert!(!resolved.items().any(|i| matches!(
        i.category,
        AttentionCategory::Dat | AttentionCategory::Unsupported
    )));
}

#[test]
fn attention_missing_source_dedup_and_disabled_source_are_conservative() {
    let (_directory, db) = fixture(2);
    db.connection
        .execute_batch("UPDATE archives SET last_verified_missing_at='2026-01-02';")
        .unwrap();
    assert!(
        db.attention_snapshot()
            .unwrap()
            .items()
            .any(|i| i.id.starts_with("catalogue:missing:"))
    );
    db.connection.execute_batch("UPDATE source_folders SET last_scan_status='failed',last_scan_error='permission denied';").unwrap();
    let failed = db.attention_snapshot().unwrap();
    assert_eq!(
        failed
            .items()
            .filter(|i| i.category == AttentionCategory::Sources)
            .count(),
        1
    );
    db.connection
        .execute_batch("UPDATE source_folders SET removed_from_config_at='2026-01-03';")
        .unwrap();
    assert_eq!(
        db.attention_snapshot()
            .unwrap()
            .page(&AttentionFilters::default())
            .total,
        0
    );
}

#[test]
fn attention_dat_set_and_identity_share_one_review_and_refresh_staleness() {
    let (_directory, db) = fixture(2);
    db.connection.execute_batch("INSERT INTO library_dat_identities(archive_id,dat_source_id,verification_state,completeness,audited_at,facts_json,created_at,updated_at) VALUES(1,'dat','no_match','exhaustive','2026-01-02',x'7b7d','2026-01-02','2026-01-02'); INSERT INTO dat_set_audit_results(archive_id,archive_path,source_id,game_name,set_state_json,dependency_state_json,audited_at) VALUES(1,x'31','dat','game','\"incomplete\"','\"missing\"','2026-01-02');").unwrap();
    let snapshot = db.attention_snapshot().unwrap();
    let dat: Vec<_> = snapshot
        .items()
        .filter(|i| i.category == AttentionCategory::Dat)
        .collect();
    assert_eq!(dat.len(), 1);
    assert!(dat[0].summary.contains("No DAT entry matched"));
    assert!(dat[0].summary.contains("required set members"));
    db.connection.execute_batch("UPDATE library_dat_identities SET verification_state='verified_single_match',revision_marked_stale=1; UPDATE dat_set_audit_results SET set_state_json='\"complete\"',dependency_state_json='\"satisfied\"';").unwrap();
    assert!(
        db.attention_snapshot()
            .unwrap()
            .items()
            .any(|i| i.category == AttentionCategory::Dat && i.summary.contains("stale"))
    );
    db.connection
        .execute_batch("UPDATE library_dat_identities SET revision_marked_stale=0;")
        .unwrap();
    assert!(
        !db.attention_snapshot()
            .unwrap()
            .items()
            .any(|i| i.category == AttentionCategory::Dat)
    );
}
