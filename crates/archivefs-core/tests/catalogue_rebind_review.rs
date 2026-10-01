//! Reviewed source rebind, source-level catalogue health, and migration 23.
//! Every database here is a private temporary copy; nothing touches a real
//! library.

use archivefs_core::catalogue_health::{
    REBIND_REVIEW_AGAIN, RebindReason, SourceHealthState, SourceRootBinding,
};
use archivefs_core::{Archive, Config, Database, scan_and_persist};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    db: Database,
    roots: Vec<PathBuf>,
}

impl Fixture {
    fn new(names: &[&str]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let roots: Vec<_> = names.iter().map(|n| temp.path().join(n)).collect();
        for root in &roots {
            fs::create_dir_all(root).unwrap();
        }
        let mut db = Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
        db.register_source_folders(&roots).unwrap();
        Self { temp, db, roots }
    }

    fn sql(&self) -> Connection {
        Connection::open(self.db.path()).unwrap()
    }

    fn source_id(&self, index: usize) -> i64 {
        self.db
            .list_source_folders()
            .unwrap()
            .iter()
            .find(|s| s.path == self.roots[index])
            .unwrap()
            .id
    }

    fn add(&mut self, source: usize, relative: &str) -> (i64, PathBuf) {
        let path = self.roots[source].join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"fixture bytes").unwrap();
        let archive = Archive::from_path_in_root(&path, &self.roots[source]).unwrap();
        let id = self
            .db
            .upsert_archive(self.source_id(source), &self.roots[source], &archive)
            .unwrap()
            .archive_id;
        (id, path)
    }

    fn config(&self) -> Config {
        Config {
            source_folders: self.roots.clone(),
            mount_root: self.temp.path().join("mounts"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        }
    }

    fn scan(&mut self) -> archivefs_core::ScanPersistSummary {
        let config = self.config();
        scan_and_persist(&mut self.db, &config, "fixture").unwrap()
    }

    fn flag(&self, id: i64) -> Option<String> {
        self.db
            .load_archives()
            .unwrap()
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
            .last_verified_missing_at
    }

    /// Make a source look like one scanned before storage continuity existed.
    fn make_historical(&self) {
        self.sql()
            .execute(
                "UPDATE source_folders SET last_successful_scan_at='legacy-scan'",
                [],
            )
            .unwrap();
    }

    fn health(&self, index: usize) -> archivefs_core::catalogue_health::SourceHealth {
        let id = self.source_id(index);
        self.db
            .source_health(&self.roots)
            .unwrap()
            .into_iter()
            .find(|h| h.source_id == id)
            .unwrap()
    }

    /// The folder is replaced by another one on the same disk: same path, new
    /// inode, so its binding no longer matches.
    fn replace_root(&self, index: usize, keep_as: &str) {
        fs::rename(&self.roots[index], self.temp.path().join(keep_as)).unwrap();
        fs::create_dir(&self.roots[index]).unwrap();
    }
}

/// Every table except the two the rebind is allowed to touch, as sorted text.
fn everything_but_bindings(connection: &Connection) -> BTreeMap<String, Vec<String>> {
    let tables: Vec<String> = connection
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' \
             AND name NOT IN ('source_scan_bindings','catalogue_health_epoch')",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .unwrap();
            let width = statement.column_count();
            let mut rows: Vec<String> = statement
                .query_map([], |r| {
                    Ok((0..width)
                        .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                        .collect::<Vec<_>>()
                        .join("|"))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            rows.sort();
            (table, rows)
        })
        .collect()
}

fn binding_rows(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM source_scan_bindings", [], |r| {
            r.get(0)
        })
        .unwrap()
}

// --- A migrated legacy source starts unbound, and nothing binds it for the user.

#[test]
fn migrated_legacy_source_starts_unbound_and_blocks_missing_authority() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    fs::remove_file(path).unwrap();
    f.make_historical();
    assert_eq!(binding_rows(&f.sql()), 0);
    let health = f.health(0);
    assert_eq!(health.state, SourceHealthState::RebindRequired);
    assert_eq!(health.rebind, Some(RebindReason::NeverBound));
    assert_eq!(health.generation, 0);
    assert!(!health.state.can_establish_missing());
    // The blocked scan neither binds nor marks anything Missing.
    let blocked = f.scan();
    assert_eq!(blocked.counts.archives_missing, 0);
    assert_eq!(f.flag(id), None);
    assert_eq!(binding_rows(&f.sql()), 0);
}

#[test]
fn nothing_rebinds_automatically() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let before = everything_but_bindings(&f.sql());
    // Reopening (what startup does), reviewing, previewing and scanning are all
    // read-only with respect to the binding.
    let path = f.db.path().to_path_buf();
    drop(f.db);
    f.db = Database::open_or_create(&path).unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::RebindRequired);
    f.db.review_source_rebind(f.source_id(0)).unwrap();
    archivefs_core::catalogue_health::preview_catalogue_health(&f.db, &f.roots).unwrap();
    f.scan();
    assert_eq!(binding_rows(&f.sql()), 0);
    assert_eq!(f.health(0).state, SourceHealthState::RebindRequired);
    let _ = before;
}

#[test]
fn a_never_scanned_source_has_nothing_to_review_and_binds_on_first_scan() {
    let mut f = Fixture::new(&["games"]);
    assert_eq!(f.health(0).state, SourceHealthState::NeedsScan);
    assert!(f.db.review_source_rebind(f.source_id(0)).is_err());
    f.add(0, "game.zip");
    f.scan();
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
    assert_eq!(f.health(0).generation, 1);
}

// --- Explicit, reviewed rebind.

#[test]
fn explicit_reviewed_rebind_succeeds_and_permits_a_later_scan_only() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    fs::remove_file(path).unwrap();
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    assert_eq!(review.reason, RebindReason::NeverBound);
    assert_eq!(review.generation, 0);
    assert_eq!(review.recorded, None);
    assert_eq!(review.archive_count, 1);
    assert_eq!(
        Some(&review.current),
        SourceRootBinding::inspect(&f.roots[0]).as_ref()
    );
    f.db.confirm_source_rebind(&review).unwrap();
    // The rebind alone marks nothing Missing and records no scan.
    assert_eq!(f.flag(id), None);
    let health = f.health(0);
    assert_eq!(health.state, SourceHealthState::NeedsScan);
    assert_eq!(health.generation, 1);
    assert!(!health.state.can_establish_missing());
    // Only the later complete scan establishes the Missing evidence.
    assert_eq!(f.scan().counts.archives_missing, 1);
    assert!(f.flag(id).is_some());
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
}

#[test]
fn rebind_changes_no_archive_identity_observation_or_missing_evidence() {
    let mut f = Fixture::new(&["games", "other"]);
    f.add(0, "a/game.zip");
    f.add(1, "b/other.zip");
    f.scan();
    f.replace_root(0, "preserved");
    let (id, _) = f.add(0, "a/game.zip");
    f.sql()
        .execute(
            "UPDATE archives SET last_verified_missing_at='old-evidence' WHERE id=?1",
            [id],
        )
        .unwrap();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    assert_eq!(review.reason, RebindReason::BackingChanged);
    let before = everything_but_bindings(&f.sql());
    let epoch_before: i64 = f
        .sql()
        .query_row("SELECT revision FROM catalogue_health_epoch", [], |r| {
            r.get(0)
        })
        .unwrap();
    f.db.confirm_source_rebind(&review).unwrap();
    assert_eq!(everything_but_bindings(&f.sql()), before);
    assert_eq!(f.flag(id).as_deref(), Some("old-evidence"));
    // The binding moved a generation; the epoch moved so previews go stale.
    assert_eq!(f.health(0).generation, review.generation + 1);
    let epoch_after: i64 = f
        .sql()
        .query_row("SELECT revision FROM catalogue_health_epoch", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(epoch_after > epoch_before);
}

// --- Refusals: anything that changed after review means review again.

fn refused(result: archivefs_core::Result<()>) {
    let error = result
        .expect_err("the stale review must be refused")
        .to_string();
    assert!(error.contains(REBIND_REVIEW_AGAIN), "{error}");
}

#[test]
fn unavailable_source_cannot_be_reviewed_or_confirmed() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    fs::rename(&f.roots[0], f.temp.path().join("unplugged")).unwrap();
    let error =
        f.db.review_source_rebind(f.source_id(0))
            .unwrap_err()
            .to_string();
    assert!(error.contains("cannot be reached"), "{error}");
    refused(f.db.confirm_source_rebind(&review));
    assert_eq!(binding_rows(&f.sql()), 0);
    assert_eq!(f.health(0).state, SourceHealthState::SourceUnavailable);
}

#[test]
fn changed_generation_refuses() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.scan();
    f.replace_root(0, "first");
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    assert_eq!(review.generation, 1);
    // Someone else rebinds the same source first.
    let other = f.db.review_source_rebind(f.source_id(0)).unwrap();
    f.db.confirm_source_rebind(&other).unwrap();
    refused(f.db.confirm_source_rebind(&review));
    assert_eq!(f.health(0).generation, 2);
}

#[test]
fn source_replaced_between_review_and_confirm_refuses() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    f.replace_root(0, "swapped-out");
    refused(f.db.confirm_source_rebind(&review));
    assert_eq!(binding_rows(&f.sql()), 0);
    // A fresh review of the replacement is a new, separate decision.
    let fresh = f.db.review_source_rebind(f.source_id(0)).unwrap();
    assert_ne!(fresh.current, review.current);
}

#[test]
fn changed_backing_filesystem_refuses() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let mut review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    // The folder now reports a different filesystem than the one reviewed (a
    // swapped disk presenting the same path), even with an identical inode.
    review.current.filesystem_id[0] ^= 0xff;
    refused(f.db.confirm_source_rebind(&review));
    assert_eq!(binding_rows(&f.sql()), 0);
}

#[test]
fn source_config_changed_between_review_and_confirm_refuses() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    f.sql()
        .execute(
            "UPDATE source_folders SET removed_from_config_at='2026-01-01T00:00:00Z'",
            [],
        )
        .unwrap();
    refused(f.db.confirm_source_rebind(&review));
    f.sql()
        .execute("UPDATE source_folders SET removed_from_config_at=NULL", [])
        .unwrap();
    let moved = f.temp.path().join("elsewhere");
    fs::create_dir(&moved).unwrap();
    f.sql()
        .execute(
            "UPDATE source_folders SET path=?1",
            [moved.as_os_str().as_encoded_bytes()],
        )
        .unwrap();
    refused(f.db.confirm_source_rebind(&review));
    assert_eq!(binding_rows(&f.sql()), 0);
}

#[test]
fn a_review_cannot_be_replayed_and_a_matching_source_needs_no_review() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    f.db.confirm_source_rebind(&review).unwrap();
    refused(f.db.confirm_source_rebind(&review));
    let error =
        f.db.review_source_rebind(f.source_id(0))
            .unwrap_err()
            .to_string();
    assert!(error.contains("nothing to review"), "{error}");
}

#[test]
fn a_wrong_recorded_binding_in_a_review_refuses() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.scan();
    f.replace_root(0, "first");
    let mut review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    review.recorded.as_mut().unwrap().inode ^= 1;
    refused(f.db.confirm_source_rebind(&review));
}

// --- Source-level health.

#[test]
fn partial_scan_cannot_establish_missing_and_is_reported_as_partial() {
    let mut f = Fixture::new(&["games", "other"]);
    let (id, path) = f.add(0, "game.zip");
    f.add(1, "other.zip");
    f.scan();
    fs::remove_file(path).unwrap();
    assert_eq!(f.flag(id), None);
    // Record a partial walk for the first source directly.
    let run: i64 = f
        .sql()
        .query_row("SELECT MAX(id) FROM scan_runs", [], |r| r.get(0))
        .unwrap();
    f.sql()
        .execute(
            "INSERT OR REPLACE INTO scan_source_coverage(scan_run_id,source_folder_id,state,\
             excluded_roots_json,diagnostic,root_identity_json,source_generation) \
             VALUES(?1,?2,'\"partial\"','[]',NULL,'null',(SELECT generation FROM \
             source_scan_bindings WHERE source_folder_id=?2))",
            rusqlite::params![run, f.source_id(0)],
        )
        .unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::PartialScan);
    assert!(!f.health(0).state.can_establish_missing());
    assert_eq!(f.flag(id), None);
}

#[test]
fn health_states_follow_the_backend_facts() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "dir/game.zip");
    f.scan();
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
    // An unproven remembered nested boundary withholds Missing authority.
    f.sql()
        .execute(
            "INSERT INTO source_nested_boundaries(source_folder_id,relative_path,binding_json) \
             VALUES(?1,?2,NULL)",
            rusqlite::params![f.source_id(0), b"dir".to_vec()],
        )
        .unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::CoverageIncomplete);
    f.sql()
        .execute("DELETE FROM source_nested_boundaries", [])
        .unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
    // A failed latest scan is not healthy.
    let run: i64 = f
        .sql()
        .query_row(
            "SELECT MAX(scan_run_id) FROM scan_source_coverage",
            [],
            |r| r.get(0),
        )
        .unwrap();
    f.sql()
        .execute(
            "UPDATE scan_source_coverage SET state='\"failed\"' WHERE scan_run_id=?1",
            [run],
        )
        .unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::CoverageIncomplete);
    // Coverage recorded for an older generation never counts for a newer one.
    f.sql()
        .execute(
            "UPDATE scan_source_coverage SET state='\"complete\"', source_generation=99",
            [],
        )
        .unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::NeedsScan);
}

#[test]
fn a_rebound_source_is_not_healthy_until_a_new_scan_covers_the_new_generation() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.scan();
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
    f.replace_root(0, "preserved");
    assert_eq!(f.health(0).state, SourceHealthState::RebindRequired);
    assert_eq!(f.health(0).rebind, Some(RebindReason::BackingChanged));
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    f.db.confirm_source_rebind(&review).unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::NeedsScan);
    f.scan();
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
}

// --- Migration 21 -> 22 -> 23 on a private copy.

fn downgrade_to_21(connection: &Connection) {
    let triggers: Vec<String> = connection
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'catalogue_epoch_%'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for trigger in triggers {
        connection
            .execute_batch(&format!("DROP TRIGGER \"{trigger}\""))
            .unwrap();
    }
    connection
        .execute_batch(
            "DROP TABLE source_nested_boundaries; DROP TABLE source_scan_bindings; \
             DROP TABLE catalogue_health_epoch; DELETE FROM schema_migrations WHERE version=23; \
             DROP TABLE scan_source_coverage; DELETE FROM schema_migrations WHERE version=22; \
             PRAGMA user_version=21;",
        )
        .unwrap();
}

#[test]
fn migration_23_preserves_history_binds_nothing_and_is_idempotent() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.scan();
    let path = f.db.path().to_path_buf();
    drop(f.db);
    downgrade_to_21(&Connection::open(&path).unwrap());
    let history = everything_but_bindings(&Connection::open(&path).unwrap());

    let db = Database::open_or_create(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), 23);
    let sql = Connection::open(&path).unwrap();
    // Historical tables are preserved row for row; the new ones start empty.
    let after = everything_but_bindings(&sql);
    for (table, rows) in history.iter().filter(|(t, _)| *t != "schema_migrations") {
        assert_eq!(after.get(table), Some(rows), "table {table} changed");
    }
    // The migration ledger only gains exactly versions 22 and 23.
    let versions = |connection: &Connection| -> Vec<i64> {
        connection
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(versions(&sql), (1..=23).collect::<Vec<_>>());
    assert_eq!(binding_rows(&sql), 0, "migration must not bind any source");
    assert_eq!(
        sql.query_row("SELECT COUNT(*) FROM source_nested_boundaries", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    assert_eq!(
        sql.query_row("SELECT revision FROM catalogue_health_epoch", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    // Integrity and foreign keys are clean.
    assert_eq!(
        sql.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        sql.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none(),
        true
    );
    // The epoch triggers are live: a catalogue mutation bumps the revision.
    sql.execute("UPDATE archives SET updated_at=updated_at", [])
        .unwrap();
    assert_eq!(
        sql.query_row("SELECT revision FROM catalogue_health_epoch", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    // A legacy source that has history is unbound and needs review.
    sql.execute(
        "UPDATE source_folders SET last_successful_scan_at='legacy-scan'",
        [],
    )
    .unwrap();
    let health = db.source_health(&f.roots).unwrap();
    assert_eq!(health[0].state, SourceHealthState::RebindRequired);

    // Idempotent: reopening a migrated database changes nothing.
    drop(db);
    let reopened = everything_but_bindings(&Connection::open(&path).unwrap());
    let db = Database::open_or_create(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), 23);
    assert_eq!(
        everything_but_bindings(&Connection::open(&path).unwrap()),
        reopened
    );
    assert_eq!(binding_rows(&Connection::open(&path).unwrap()), 0);
}

#[test]
fn a_failed_migration_leaves_the_copy_at_its_old_version() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.scan();
    let path = f.db.path().to_path_buf();
    drop(f.db);
    let sql = Connection::open(&path).unwrap();
    downgrade_to_21(&sql);
    // A pre-existing object with a migration's name makes migration 22 fail.
    sql.execute_batch("CREATE TABLE scan_source_coverage (poison INTEGER)")
        .unwrap();
    drop(sql);
    let before = everything_but_bindings(&Connection::open(&path).unwrap());
    assert!(Database::open_or_create(&path).is_err());
    let sql = Connection::open(&path).unwrap();
    assert_eq!(
        sql.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        21
    );
    assert_eq!(everything_but_bindings(&sql), before);
    assert!(
        sql.query_row(
            "SELECT name FROM sqlite_master WHERE name='source_scan_bindings'",
            [],
            |r| r.get::<_, String>(0)
        )
        .is_err(),
        "no half-applied migration 23"
    );
}
