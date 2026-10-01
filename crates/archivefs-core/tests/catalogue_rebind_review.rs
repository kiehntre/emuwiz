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
fn rebinding_a_never_bound_source_never_revalidates_old_coverage() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.scan();
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
    // The binding row is gone but coverage recorded for generation 1 remains.
    f.sql()
        .execute("DELETE FROM source_scan_bindings", [])
        .unwrap();
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    assert_eq!(review.generation, 0);
    f.db.confirm_source_rebind(&review).unwrap();
    // A fresh generation, not generation 1 again: the old proof does not count.
    assert_eq!(f.health(0).generation, 2);
    assert_eq!(f.health(0).state, SourceHealthState::NeedsScan);
    f.scan();
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

#[test]
fn scanning_one_source_does_not_degrade_another_sources_health() {
    use archivefs_core::{add_source_folder_at, scan_source_folder_at};
    let temp = tempfile::tempdir().unwrap();
    let (config, database) = (
        temp.path().join("config.toml"),
        temp.path().join("library.sqlite3"),
    );
    let folders: Vec<PathBuf> = ["one", "two"]
        .iter()
        .map(|n| {
            let folder = temp.path().join(n);
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("game.zip"), b"game").unwrap();
            folder
        })
        .collect();
    Database::open_or_create(&database).unwrap();
    for folder in &folders {
        add_source_folder_at(&config, &database, folder).unwrap();
    }
    scan_source_folder_at(&config, &database, &folders[0], "first").unwrap();
    // A targeted scan of the second source records NotAttempted for the first.
    scan_source_folder_at(&config, &database, &folders[1], "second").unwrap();
    let db = Database::open_or_create(&database).unwrap();
    let health = db.source_health(&folders).unwrap();
    assert_eq!(health.len(), 2);
    for entry in &health {
        assert_eq!(entry.state, SourceHealthState::Healthy, "{:?}", entry.path);
    }
    let not_attempted: i64 = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM scan_source_coverage WHERE state='\"not_attempted\"' \
             AND source_folder_id=(SELECT id FROM source_folders WHERE path=?1)",
            [folders[0].as_os_str().as_encoded_bytes()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        not_attempted > 0,
        "the scenario must actually record NotAttempted rows"
    );
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
            "DROP TABLE IF EXISTS source_review_required; DROP TABLE source_nested_boundaries; DROP TABLE source_scan_bindings; \
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

#[test]
fn independent_old_complete_run_cannot_override_new_partial_attempt() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    let old = f.scan().scan_run_id;
    let latest = f.scan().scan_run_id;
    f.sql()
        .execute(
            "UPDATE scan_source_coverage SET state='\"partial\"' WHERE scan_run_id=?1",
            [latest],
        )
        .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::PartialScan);
    let source = f.source_id(0);
    let result = f.db.mark_unseen_archives_missing(old, source, &[]);
    eprintln!(
        "old complete run result={result:?}, missing={:?}",
        f.flag(id)
    );
    assert!(
        f.flag(id).is_none(),
        "non-Healthy source acquired NEW Missing evidence"
    );
}
#[test]
fn independent_old_rebind_api_cannot_resurrect_old_generation() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    let old = f.scan().scan_run_id;
    f.sql()
        .execute("DELETE FROM source_scan_bindings", [])
        .unwrap();
    let source = f.source_id(0);
    f.db.rebind_source_after_review(source, 0, SourceRootBinding::inspect(&f.roots[0]).unwrap())
        .unwrap();
    fs::remove_file(path).unwrap();
    let result = f.db.mark_unseen_archives_missing(old, source, &[]);
    eprintln!(
        "legacy rebind generation={}, result={result:?}, missing={:?}",
        f.health(0).generation,
        f.flag(id)
    );
    assert!(
        f.flag(id).is_none(),
        "pre-rebind coverage authorized Missing"
    );
}
#[test]
fn independent_review_refuses_configuration_aba() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    f.db.register_source_folders(&[]).unwrap();
    f.db.register_source_folders(&f.roots).unwrap();
    let result = f.db.confirm_source_rebind(&review);
    eprintln!(
        "configuration removed/readded: result={result:?}, bindings={}",
        binding_rows(&f.sql())
    );
    assert!(
        result.is_err(),
        "review survived source disappearance/reappearance"
    );
}
#[test]
fn independent_review_refuses_source_role_change() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    f.db.set_source_role(&f.roots[0], archivefs_core::SourceRole::ArcadeRomset)
        .unwrap();
    let result = f.db.confirm_source_rebind(&review);
    eprintln!(
        "role changed: result={result:?}, bindings={}",
        binding_rows(&f.sql())
    );
    assert!(
        result.is_err(),
        "review survived source configuration change"
    );
}
#[test]
fn independent_migrated_catalogue_without_success_timestamp_needs_review() {
    let mut f = Fixture::new(&["games"]);
    let (id, _) = f.add(0, "game.zip");
    let path = f.db.path().to_path_buf();
    drop(f.db);
    downgrade_to_21(&Connection::open(&path).unwrap());
    f.db = Database::open_or_create(&path).unwrap();
    f.replace_root(0, "old-storage");
    let result = f.scan();
    eprintln!(
        "migrated historical rows, no successful timestamp: bindings={}, missing={:?}, scan_missing={}",
        binding_rows(&f.sql()),
        f.flag(id),
        result.counts.archives_missing
    );
    assert!(
        f.flag(id).is_none(),
        "migration with catalogue history silently bound replacement storage and marked Missing"
    );
}
#[test]
fn independent_targeted_attempt_combinations() {
    use archivefs_core::{add_source_folder_at, scan_source_folder_at};
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let database = temp.path().join("library.sqlite3");
    let folders: Vec<PathBuf> = ["A", "B", "C"]
        .iter()
        .map(|n| temp.path().join(n))
        .collect();
    Database::open_or_create(&database).unwrap();
    for folder in &folders {
        fs::create_dir(folder).unwrap();
        fs::write(folder.join("game.zip"), b"game").unwrap();
        add_source_folder_at(&config, &database, folder).unwrap();
        scan_source_folder_at(&config, &database, folder, "initial").unwrap();
    }
    let sql = Connection::open(&database).unwrap();
    sql.execute("UPDATE scan_source_coverage SET state='\"partial\"' WHERE source_folder_id=(SELECT id FROM source_folders WHERE path=?1) AND state='\"complete\"'", [folders[2].as_os_str().as_encoded_bytes()]).unwrap();
    let mut db = Database::open_or_create(&database).unwrap();
    let states = |db: &Database| {
        db.source_health(&folders)
            .unwrap()
            .iter()
            .map(|h| h.state)
            .collect::<Vec<_>>()
    };
    for _ in 0..2 {
        scan_source_folder_at(&config, &database, &folders[0], "target").unwrap();
    }
    assert_eq!(
        states(&db),
        vec![
            SourceHealthState::Healthy,
            SourceHealthState::Healthy,
            SourceHealthState::PartialScan
        ]
    );
    fs::rename(&folders[0], temp.path().join("A-away")).unwrap();
    let _ = scan_source_folder_at(&config, &database, &folders[0], "failed-target");
    assert_eq!(
        states(&db)[1..],
        [SourceHealthState::Healthy, SourceHealthState::PartialScan]
    );
    fs::rename(temp.path().join("A-away"), &folders[0]).unwrap();
    fs::rename(&folders[1], temp.path().join("B-old")).unwrap();
    fs::create_dir(&folders[1]).unwrap();
    let b = db
        .list_source_folders()
        .unwrap()
        .iter()
        .find(|s| s.path == folders[1])
        .unwrap()
        .id;
    let review = db.review_source_rebind(b).unwrap();
    db.confirm_source_rebind(&review).unwrap();
    scan_source_folder_at(&config, &database, &folders[0], "target-after-rebind").unwrap();
    assert_eq!(
        states(&db)[1..],
        [SourceHealthState::NeedsScan, SourceHealthState::PartialScan]
    );
    let full = Config {
        source_folders: folders.clone(),
        mount_root: temp.path().join("mounts"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    scan_and_persist(&mut db, &full, "full").unwrap();
    assert_eq!(states(&db), vec![SourceHealthState::Healthy; 3]);
}
#[test]
fn independent_new_confirm_rejects_old_coverage_at_write_boundary() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    let old = f.scan().scan_run_id;
    f.sql()
        .execute("DELETE FROM source_scan_bindings", [])
        .unwrap();
    let source = f.source_id(0);
    let review = f.db.review_source_rebind(source).unwrap();
    f.db.confirm_source_rebind(&review).unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(f.health(0).generation, 2);
    assert!(f.db.mark_unseen_archives_missing(old, source, &[]).is_err());
    assert!(f.flag(id).is_none());
}
#[test]
#[ignore = "run alone with the process-local fault shim"]
fn independent_rebind_postwrite_identity_change_rolls_back() {
    assert_eq!(std::env::var("EMUWIZ_INJECT_FAULTS").as_deref(), Ok("1"));
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    let before = everything_but_bindings(&f.sql());
    unsafe {
        std::env::set_var("EMUWIZ_FAULT_PATH", &f.roots[0]);
        std::env::set_var("EMUWIZ_FAULT_ROOT", &f.roots[0]);
        std::env::set_var("EMUWIZ_FAULT_PROBE_NUMBER", "2");
    }
    let result = f.db.confirm_source_rebind(&review);
    unsafe {
        std::env::remove_var("EMUWIZ_FAULT_PATH");
        std::env::remove_var("EMUWIZ_FAULT_ROOT");
        std::env::remove_var("EMUWIZ_FAULT_PROBE_NUMBER");
    }
    assert_ne!(
        SourceRootBinding::inspect(&f.roots[0]).as_ref(),
        Some(&review.current),
        "fault must fire"
    );
    refused(result);
    assert_eq!(binding_rows(&f.sql()), 0);
    assert_eq!(everything_but_bindings(&f.sql()), before);
}

// =====================================================================================
// Authority repairs: one rule - no new Missing evidence unless the source is presently
// Healthy for the exact current binding, generation and latest actual scan attempt.
// =====================================================================================

/// A catalogue migrated from before storage continuity existed: rows, no binding, no
/// successful-scan timestamp, then reopened so migration 23 runs for real.
fn migrated(names: &[&str]) -> (Fixture, Vec<(i64, PathBuf)>) {
    let mut f = Fixture::new(names);
    let rows: Vec<_> = (0..names.len()).map(|i| f.add(i, "game.zip")).collect();
    let path = f.db.path().to_path_buf();
    drop(f.db);
    downgrade_to_21(&Connection::open(&path).unwrap());
    f.db = Database::open_or_create(&path).unwrap();
    (f, rows)
}

fn review_required_rows(f: &Fixture) -> i64 {
    f.sql()
        .query_row("SELECT COUNT(*) FROM source_review_required", [], |r| {
            r.get(0)
        })
        .unwrap()
}

/// A later scan outcome recorded for a source, as a real scan records it.
fn record_attempt(f: &mut Fixture, index: usize, state: &str) -> i64 {
    let run = f.db.start_scan_run("later-attempt", None).unwrap();
    f.sql()
        .execute(
            "INSERT INTO scan_source_coverage(scan_run_id,source_folder_id,state,\
             excluded_roots_json,root_identity_json,source_generation) \
             VALUES(?1,?2,?3,'[]','null',(SELECT generation FROM source_scan_bindings \
             WHERE source_folder_id=?2))",
            rusqlite::params![
                run,
                f.source_id(index),
                serde_json::to_string(state).unwrap()
            ],
        )
        .unwrap();
    run
}

// --- Blocker 1: a migrated source is never bound by a first scan ----------------

#[test]
fn migration_records_which_sources_need_review_and_binds_none() {
    let (f, _) = migrated(&["games", "other"]);
    assert_eq!(review_required_rows(&f), 2);
    assert_eq!(binding_rows(&f.sql()), 0);
    for index in 0..2 {
        assert_eq!(f.health(index).state, SourceHealthState::RebindRequired);
        assert_eq!(f.health(index).rebind, Some(RebindReason::NeverBound));
    }
}

#[test]
fn a_migrated_source_with_rows_and_no_timestamp_never_auto_binds() {
    // Each variation of "what is at the old path now" must fail closed.
    type Setup = fn(&Fixture);
    let scenarios: [(&str, Setup); 5] = [
        ("different storage", |f| {
            f.replace_root(0, "old-storage");
            fs::write(f.roots[0].join("other.zip"), b"different").unwrap();
        }),
        ("apparently identical storage", |_| {}),
        ("empty replacement directory", |f| {
            f.replace_root(0, "old-storage")
        }),
        ("unavailable then available", |f| {
            let away = f.temp.path().join("away");
            fs::rename(&f.roots[0], &away).unwrap();
            fs::rename(&away, &f.roots[0]).unwrap();
        }),
        ("scanned twice", |_| {}),
    ];
    for (label, setup) in scenarios {
        let (mut f, rows) = migrated(&["games"]);
        setup(&f);
        let (id, path) = &rows[0];
        let _ = fs::remove_file(path);
        for _ in 0..2 {
            let result = f.scan();
            assert_eq!(result.counts.archives_missing, 0, "{label}");
        }
        assert_eq!(
            binding_rows(&f.sql()),
            0,
            "{label}: a scan auto-bound the source"
        );
        assert_eq!(f.flag(*id), None, "{label}: Missing evidence was written");
        assert_eq!(
            f.health(0).state,
            SourceHealthState::RebindRequired,
            "{label}"
        );
        assert_eq!(review_required_rows(&f), 1, "{label}");
    }
}

#[test]
fn a_source_unavailable_during_its_first_post_migration_scan_still_needs_review() {
    let (mut f, rows) = migrated(&["games"]);
    let away = f.temp.path().join("away");
    fs::rename(&f.roots[0], &away).unwrap();
    f.scan();
    fs::rename(&away, &f.roots[0]).unwrap();
    fs::remove_file(&rows[0].1).unwrap();
    assert_eq!(f.scan().counts.archives_missing, 0);
    assert_eq!(binding_rows(&f.sql()), 0);
    assert_eq!(f.flag(rows[0].0), None);
}

#[test]
fn only_a_reviewed_rebind_makes_a_migrated_source_eligible_and_it_clears_the_requirement() {
    let (mut f, rows) = migrated(&["games"]);
    fs::remove_file(&rows[0].1).unwrap();
    assert_eq!(f.scan().counts.archives_missing, 0);
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    assert_eq!(review.reason, RebindReason::NeverBound);
    f.db.confirm_source_rebind(&review).unwrap();
    assert_eq!(review_required_rows(&f), 0);
    assert_eq!(
        f.flag(rows[0].0),
        None,
        "rebind alone marks nothing missing"
    );
    assert_eq!(f.scan().counts.archives_missing, 1);
    assert!(f.flag(rows[0].0).is_some());
}

#[test]
fn a_source_added_after_migration_still_binds_on_its_first_scan() {
    let (mut f, _) = migrated(&["games"]);
    let fresh = f.temp.path().join("fresh");
    fs::create_dir(&fresh).unwrap();
    fs::write(fresh.join("new.zip"), b"new").unwrap();
    f.roots.push(fresh);
    let config = f.config();
    f.db.register_source_folders(&f.roots).unwrap();
    scan_and_persist(&mut f.db, &config, "after migration").unwrap();
    assert_eq!(f.health(1).state, SourceHealthState::Healthy);
    assert_eq!(f.health(0).state, SourceHealthState::RebindRequired);
    assert_eq!(binding_rows(&f.sql()), 1);
}

#[test]
fn a_fresh_source_whose_first_scan_found_the_drive_offline_binds_on_a_later_scan() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    let away = f.temp.path().join("away");
    fs::rename(&f.roots[0], &away).unwrap();
    f.scan();
    assert_eq!(binding_rows(&f.sql()), 0);
    fs::rename(&away, &f.roots[0]).unwrap();
    f.scan();
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
}

// --- Blocker 2: superseded coverage has no authority -------------------------------

#[test]
fn old_complete_coverage_loses_authority_to_any_later_actual_attempt() {
    for later in ["partial", "failed", "unavailable", "skipped"] {
        let mut f = Fixture::new(&["games"]);
        let (id, path) = f.add(0, "game.zip");
        let old = f.scan().scan_run_id;
        record_attempt(&mut f, 0, later);
        fs::remove_file(path).unwrap();
        let source = f.source_id(0);
        let result = f.db.mark_unseen_archives_missing(old, source, &[]);
        assert!(
            result.is_err(),
            "{later}: superseded coverage was accepted: {result:?}"
        );
        assert_eq!(f.flag(id), None, "{later}: Missing evidence was written");
        assert_ne!(f.health(0).state, SourceHealthState::Healthy, "{later}");
    }
}

#[test]
fn a_later_not_attempted_record_does_not_supersede_the_latest_actual_attempt() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    let old = f.scan().scan_run_id;
    record_attempt(&mut f, 0, "not_attempted");
    record_attempt(&mut f, 0, "removed");
    fs::remove_file(path).unwrap();
    assert_eq!(f.health(0).state, SourceHealthState::Healthy);
    let source = f.source_id(0);
    assert_eq!(
        f.db.mark_unseen_archives_missing(old, source, &[]).unwrap(),
        1
    );
    assert!(f.flag(id).is_some());
}

#[test]
fn only_the_latest_actual_attempt_may_write_even_when_both_are_complete() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    let first = f.scan().scan_run_id;
    let second = f.scan().scan_run_id;
    fs::remove_file(path).unwrap();
    let source = f.source_id(0);
    assert!(
        f.db.mark_unseen_archives_missing(first, source, &[])
            .is_err()
    );
    assert_eq!(f.flag(id), None);
    assert_eq!(
        f.db.mark_unseen_archives_missing(second, source, &[])
            .unwrap(),
        1
    );
}

#[test]
fn missing_evidence_requires_an_accepted_binding_at_the_write_boundary() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    let run = f.scan().scan_run_id;
    f.sql()
        .execute("DELETE FROM source_scan_bindings", [])
        .unwrap();
    fs::remove_file(path).unwrap();
    let source = f.source_id(0);
    assert!(f.db.mark_unseen_archives_missing(run, source, &[]).is_err());
    assert_eq!(f.flag(id), None);
}

// --- Blocker 3: every rebind allocates a generation above all historical coverage --

/// Both ways of committing a rebind, so neither can diverge from the other.
fn rebind_every_way(f: &mut Fixture, index: usize, via_review: bool) {
    let source = f.source_id(index);
    if via_review {
        let review = f.db.review_source_rebind(source).unwrap();
        f.db.confirm_source_rebind(&review).unwrap();
    } else {
        let generation: i64 = f
            .sql()
            .query_row(
                "SELECT COALESCE((SELECT generation FROM source_scan_bindings WHERE source_folder_id=?1),0)",
                [source],
                |r| r.get(0),
            )
            .unwrap();
        let binding = SourceRootBinding::inspect(&f.roots[index]).unwrap();
        f.db.rebind_source_after_review(source, generation, binding)
            .unwrap();
    }
}

#[test]
fn a_rebind_takes_a_generation_above_every_historical_generation_by_either_path() {
    for via_review in [true, false] {
        let mut f = Fixture::new(&["games"]);
        let (id, path) = f.add(0, "game.zip");
        let first = f.scan().scan_run_id;
        let second = f.scan().scan_run_id;
        // Several historical generations exist in coverage; the binding row is lost.
        f.sql()
            .execute(
                "UPDATE scan_source_coverage SET source_generation=5 WHERE scan_run_id=?1",
                [first],
            )
            .unwrap();
        f.sql()
            .execute(
                "UPDATE scan_source_coverage SET source_generation=7 WHERE scan_run_id=?1",
                [second],
            )
            .unwrap();
        f.sql()
            .execute("DELETE FROM source_scan_bindings", [])
            .unwrap();
        rebind_every_way(&mut f, 0, via_review);
        assert_eq!(f.health(0).generation, 8, "via_review={via_review}");
        assert_eq!(f.health(0).state, SourceHealthState::NeedsScan);
        fs::remove_file(path).unwrap();
        let source = f.source_id(0);
        for old in [first, second] {
            assert!(
                f.db.mark_unseen_archives_missing(old, source, &[]).is_err(),
                "via_review={via_review}: coverage from a historical generation regained authority"
            );
        }
        assert_eq!(f.flag(id), None);
    }
}

#[test]
fn rebinding_an_existing_binding_also_clears_every_historical_generation() {
    for via_review in [true, false] {
        let mut f = Fixture::new(&["games"]);
        let (id, path) = f.add(0, "game.zip");
        let old = f.scan().scan_run_id;
        // Coverage recorded under a generation higher than the binding's own.
        f.sql()
            .execute("UPDATE scan_source_coverage SET source_generation=9", [])
            .unwrap();
        f.replace_root(0, "preserved");
        rebind_every_way(&mut f, 0, via_review);
        assert_eq!(f.health(0).generation, 10, "via_review={via_review}");
        fs::remove_file(path).ok();
        let source = f.source_id(0);
        assert!(f.db.mark_unseen_archives_missing(old, source, &[]).is_err());
        assert_eq!(f.flag(id), None);
    }
}

#[test]
fn a_removed_and_readded_source_cannot_reuse_old_coverage_after_rebind() {
    for via_review in [true, false] {
        let mut f = Fixture::new(&["games"]);
        let (id, path) = f.add(0, "game.zip");
        let old = f.scan().scan_run_id;
        f.db.register_source_folders(&[]).unwrap();
        f.db.register_source_folders(&f.roots).unwrap();
        f.replace_root(0, "preserved");
        rebind_every_way(&mut f, 0, via_review);
        fs::remove_file(path).ok();
        let source = f.source_id(0);
        assert!(f.db.mark_unseen_archives_missing(old, source, &[]).is_err());
        assert_eq!(f.flag(id), None);
        assert!(f.health(0).generation > 1);
    }
}

#[test]
fn rebind_after_a_partial_or_complete_old_scan_never_revives_either() {
    for (later, via_review) in [
        ("partial", true),
        ("partial", false),
        ("complete", true),
        ("complete", false),
    ] {
        let mut f = Fixture::new(&["games"]);
        let (id, path) = f.add(0, "game.zip");
        let complete = f.scan().scan_run_id;
        let latest = record_attempt(&mut f, 0, later);
        f.replace_root(0, "preserved");
        rebind_every_way(&mut f, 0, via_review);
        fs::remove_file(path).ok();
        let source = f.source_id(0);
        for run in [complete, latest] {
            // Refused outright, or nothing written (a non-complete run never could).
            let result = f.db.mark_unseen_archives_missing(run, source, &[]);
            assert!(
                !matches!(result, Ok(written) if written > 0),
                "{later}/{via_review}: run {run} kept its authority after the rebind"
            );
        }
        assert_eq!(f.flag(id), None);
        assert_eq!(f.health(0).state, SourceHealthState::NeedsScan);
    }
}

#[test]
fn generation_allocation_is_checked_and_refuses_instead_of_wrapping() {
    for via_review in [true, false] {
        let mut f = Fixture::new(&["games"]);
        f.add(0, "game.zip");
        f.scan();
        f.sql()
            .execute(
                "UPDATE scan_source_coverage SET source_generation=?1",
                [i64::MAX],
            )
            .unwrap();
        f.sql()
            .execute("DELETE FROM source_scan_bindings", [])
            .unwrap();
        let source = f.source_id(0);
        let result = if via_review {
            let review = f.db.review_source_rebind(source).unwrap();
            f.db.confirm_source_rebind(&review)
        } else {
            f.db.rebind_source_after_review(
                source,
                0,
                SourceRootBinding::inspect(&f.roots[0]).unwrap(),
            )
        };
        let error = result
            .expect_err("an exhausted generation space must refuse")
            .to_string();
        assert!(error.contains("exhausted"), "{error}");
        assert_eq!(
            binding_rows(&f.sql()),
            0,
            "nothing may be written on overflow"
        );
    }
}

#[test]
fn the_legacy_rebind_api_is_not_a_second_authority_path() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.scan();
    let source = f.source_id(0);
    f.replace_root(0, "preserved");
    let binding = SourceRootBinding::inspect(&f.roots[0]).unwrap();
    // Wrong generation, wrong storage, and a removed source are all refused.
    refused(f.db.rebind_source_after_review(source, 99, binding.clone()));
    let mut other = binding.clone();
    other.filesystem_id[0] ^= 0xff;
    refused(f.db.rebind_source_after_review(source, 1, other));
    assert_eq!(f.health(0).generation, 1, "refused rebinds changed nothing");
    f.sql()
        .execute("UPDATE source_folders SET removed_from_config_at='x'", [])
        .unwrap();
    refused(f.db.rebind_source_after_review(source, 1, binding));
    let generation: i64 = f
        .sql()
        .query_row("SELECT generation FROM source_scan_bindings", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(generation, 1);
}

// --- Blocker 4: a review binds the configuration it reviewed ------------------------

#[test]
fn a_review_is_refused_after_any_authority_relevant_change() {
    type Change = fn(&mut Fixture);
    let changes: [(&str, Change); 5] = [
        ("source removed and re-added", |f| {
            f.db.register_source_folders(&[]).unwrap();
            f.db.register_source_folders(&f.roots).unwrap();
        }),
        ("role changed", |f| {
            f.db.set_source_role(&f.roots[0], archivefs_core::SourceRole::ArcadeRomset)
                .unwrap();
        }),
        ("platform assigned", |f| {
            f.sql()
                .execute(
                    "UPDATE source_folders SET assigned_platform='Nintendo Entertainment System'",
                    [],
                )
                .unwrap();
        }),
        ("another rebind committed", |f| {
            let again = f.db.review_source_rebind(f.source_id(0)).unwrap();
            f.db.confirm_source_rebind(&again).unwrap();
        }),
        ("a scan ran", |f| {
            // A scan of the source is refused (it needs review) but still records an
            // attempt; any catalogue write moves the epoch.
            f.scan();
            f.sql()
                .execute("UPDATE archives SET updated_at=updated_at", [])
                .unwrap();
        }),
    ];
    for (label, change) in changes {
        let mut f = Fixture::new(&["games"]);
        f.add(0, "game.zip");
        f.make_historical();
        let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
        change(&mut f);
        let before = binding_rows(&f.sql());
        let result = f.db.confirm_source_rebind(&review);
        assert!(result.is_err(), "{label}: a stale review was accepted");
        if label != "another rebind committed" {
            assert_eq!(binding_rows(&f.sql()), before, "{label}");
        }
    }
}

#[test]
fn read_only_use_does_not_invalidate_a_review() {
    let mut f = Fixture::new(&["games"]);
    f.add(0, "game.zip");
    f.make_historical();
    let review = f.db.review_source_rebind(f.source_id(0)).unwrap();
    // Everything the GUI does while a review is on screen: health, previews, a
    // snapshot-style reopen, and a fresh review of the same source.
    f.db.source_health(&f.roots).unwrap();
    archivefs_core::catalogue_health::preview_catalogue_health(&f.db, &f.roots).unwrap();
    f.db.load_archives().unwrap();
    f.db.list_source_folders().unwrap();
    let path = f.db.path().to_path_buf();
    drop(f.db);
    f.db = Database::open_or_create(&path).unwrap();
    assert_eq!(f.db.review_source_rebind(f.source_id(0)).unwrap(), review);
    // Display-only state lives in config.toml / GUI state, not in the catalogue, so
    // it cannot move the epoch. Any catalogue write is treated as authority-relevant
    // (see `confirm_source_rebind`): reviews are confirmed straight away.
    f.db.confirm_source_rebind(&review).unwrap();
}

// --- Self-audit: attacks that must all fail closed ----------------------------------

#[test]
fn self_audit_every_old_generation_is_dead_after_a_rebind() {
    let mut f = Fixture::new(&["games"]);
    let (id, path) = f.add(0, "game.zip");
    let mut runs = vec![f.scan().scan_run_id];
    for _ in 0..3 {
        f.replace_root(0, &format!("kept-{}", runs.len()));
        fs::write(f.roots[0].join("game.zip"), b"fixture bytes").unwrap();
        rebind_every_way(&mut f, 0, runs.len() % 2 == 0);
        runs.push(f.scan().scan_run_id);
    }
    // Rebind once more; now try every earlier run's coverage.
    f.replace_root(0, "kept-final");
    rebind_every_way(&mut f, 0, true);
    fs::remove_file(f.roots[0].join("game.zip")).ok();
    fs::remove_file(path).ok();
    let source = f.source_id(0);
    for run in runs {
        assert!(
            f.db.mark_unseen_archives_missing(run, source, &[]).is_err(),
            "run {run}"
        );
    }
    assert_eq!(f.flag(id), None);
}
