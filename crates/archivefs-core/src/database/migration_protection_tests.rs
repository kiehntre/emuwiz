//! Slice 2 adversarial tests: migration entry-point bypasses, SQLite journal
//! and WAL states, and concurrent/identity-replacement upgrade behaviour.
//! Only private synthetic SQLite databases in unique temporary directories are
//! used; nothing here touches a real library database.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use rusqlite::Connection;

use super::backup_ownership::{UpgradeLock, receipt_path_for};
use super::{
    Database, MIGRATIONS, Migration, UPGRADE_STAGE_HOOK, UpgradeBackupState, apply_migrations,
    inspect_upgrade_backups, latest_known_version, open_connection, schema_version,
    upgrade_library_database_with_migrations,
};

const STEP_EIGHT: [Migration; 1] = [Migration {
    version: 8,
    description: "synthetic step eight",
    sql: "CREATE TABLE step_eight (id INTEGER PRIMARY KEY);",
}];

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "emuwiz-migration-protection-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

/// A synthetic schema-7 database: `items` rows plus bookkeeping, user_version 7.
fn make_schema_seven(path: &Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, description TEXT, \
             applied_at TEXT);
             CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO items (name) VALUES ('alpha'), ('beta'), ('gamma');
             PRAGMA user_version = 7;",
        )
        .unwrap();
    connection.close().unwrap();
}

/// A genuinely old database built from the real migration chain.
fn make_real_old_database(path: &Path, steps: usize) {
    let mut connection = open_connection(path).unwrap();
    apply_migrations(&mut connection, &MIGRATIONS[..steps]).unwrap();
    connection.close().unwrap();
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut rows: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).unwrap_or_default(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn user_version(path: &Path) -> i64 {
    let connection =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

fn upgrade(path: &Path) -> crate::Result<crate::DatabaseUpgradeReport> {
    upgrade_library_database_with_migrations(path, &STEP_EIGHT)
}

struct HookGuard;
impl HookGuard {
    fn install(hook: impl Fn(&str, &Path) -> std::result::Result<(), String> + 'static) -> Self {
        UPGRADE_STAGE_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        Self
    }
}
impl Drop for HookGuard {
    fn drop(&mut self) {
        UPGRADE_STAGE_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

fn backup_artifacts(dir: &Path) -> Vec<String> {
    snapshot(dir)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.contains(".backup"))
        .collect()
}

// ---- 1. direct open cannot bypass protected migration ---------------------

#[test]
fn direct_open_of_an_outdated_database_does_not_migrate_or_back_up() {
    let root = temp_root("direct-open");
    let db = root.join("library.sqlite3");
    make_real_old_database(&db, 7);
    let before = snapshot(&root);
    assert_eq!(user_version(&db), 7);

    let error = Database::open_or_create(&db)
        .err()
        .expect("outdated open must be refused");
    let text = error.to_string();
    assert!(text.contains("upgrade"), "{text}");

    assert_eq!(user_version(&db), 7, "schema must be unchanged");
    assert_eq!(snapshot(&root), before, "no file may be created or changed");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn newer_or_ambiguous_databases_are_refused_clearly() {
    let root = temp_root("ambiguous");
    let newer = root.join("newer.sqlite3");
    make_real_old_database(&newer, MIGRATIONS.len());
    Connection::open(&newer)
        .unwrap()
        .pragma_update(None, "user_version", latest_known_version(MIGRATIONS) + 5)
        .unwrap();
    let before = snapshot(&root);
    assert!(Database::open_or_create(&newer).is_err());
    assert_eq!(snapshot(&root), before);

    // user_version 0 but real objects: not a fresh database, not migratable.
    let unversioned = root.join("unversioned.sqlite3");
    Connection::open(&unversioned)
        .unwrap()
        .execute_batch("CREATE TABLE precious (id INTEGER); INSERT INTO precious VALUES (1);")
        .unwrap();
    let before = snapshot(&root);
    let error = Database::open_or_create(&unversioned)
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("unversioned") || error.contains("refus"),
        "{error}"
    );
    assert_eq!(snapshot(&root), before);
    let _ = fs::remove_dir_all(root);
}

// ---- 9. current-version compatibility -------------------------------------

#[test]
fn current_version_databases_still_open_create_and_reopen() {
    let root = temp_root("current");
    let db = root.join("nested").join("library.sqlite3");
    let created = Database::open_or_create(&db).unwrap();
    assert_eq!(
        created.schema_version().unwrap(),
        latest_known_version(MIGRATIONS)
    );
    created.close().unwrap();
    let reopened = Database::open_or_create(&db).unwrap();
    assert_eq!(
        reopened.schema_version().unwrap(),
        latest_known_version(MIGRATIONS)
    );
    reopened.close().unwrap();
    assert!(Database::open_read_only(&db).is_ok());

    // WAL-mode current database with committed, un-checkpointed frames.
    let wal = root.join("wal.sqlite3");
    let database = Database::open_or_create(&wal).unwrap();
    database.close().unwrap();
    let connection = Connection::open(&wal).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection.close().unwrap();
    assert!(Database::open_or_create(&wal).is_ok());
    let _ = fs::remove_dir_all(root);
}

// ---- 3. concurrency ---------------------------------------------------------

#[test]
fn concurrent_creators_serialise_and_each_get_a_complete_schema() {
    let root = temp_root("concurrent-create");
    let db = Arc::new(root.join("library.sqlite3"));
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let (db, barrier) = (db.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                Database::open_or_create(&*db).map(|database| database.schema_version().unwrap())
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(
            handle.join().unwrap().unwrap(),
            latest_known_version(MIGRATIONS)
        );
    }
    let connection = Connection::open(&*db).unwrap();
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(rows as usize, MIGRATIONS.len());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn open_or_create_cannot_migrate_while_an_upgrade_holds_the_lock() {
    let root = temp_root("lock-held");
    let db = root.join("library.sqlite3");
    make_real_old_database(&db, 7);
    let _lock = UpgradeLock::acquire(&db).unwrap();
    assert!(Database::open_or_create(&db).is_err());
    assert_eq!(user_version(&db), 7);
    assert!(upgrade(&db).is_err(), "a second upgrade must not run");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn two_racing_upgrades_perform_exactly_one_migration() {
    let root = temp_root("racing-upgrades");
    let db = Arc::new(root.join("library.sqlite3"));
    make_schema_seven(&db);
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let (db, barrier) = (db.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                upgrade_library_database_with_migrations(&db, &STEP_EIGHT).is_ok()
            })
        })
        .collect();
    let successes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|ok| *ok)
        .count();
    assert_eq!(successes, 1);
    assert_eq!(user_version(&db), 8);
    let backups: Vec<_> = backup_artifacts(&root)
        .into_iter()
        .filter(|name| !name.ends_with(".receipt.json"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    let _ = fs::remove_dir_all(root);
}

// ---- 4. identity replacement + 10. interruption ------------------------------

fn replace_with_other_schema_seven(path: &Path) {
    let other = path.with_extension("other");
    make_schema_seven(&other);
    Connection::open(&other)
        .unwrap()
        .execute("INSERT INTO items (name) VALUES ('impostor')", [])
        .unwrap();
    fs::rename(&other, path).unwrap();
}

#[test]
fn replacement_after_the_lock_is_taken_is_refused_before_any_backup() {
    let root = temp_root("replace-after-lock");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let _guard = HookGuard::install(|stage, path| {
        if stage == "after-lock" {
            replace_with_other_schema_seven(path);
        }
        Ok(())
    });
    let error = upgrade(&db).err().expect("replacement must be refused");
    assert!(error.to_string().contains("replaced"), "{error}");
    assert_eq!(user_version(&db), 7);
    assert!(backup_artifacts(&root).is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn replacement_after_the_backup_is_refused_and_evidence_retained() {
    let root = temp_root("replace-after-backup");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let _guard = HookGuard::install(|stage, path| {
        if stage == "after-backup" {
            replace_with_other_schema_seven(path);
        }
        Ok(())
    });
    let error = upgrade(&db).err().expect("replacement must be refused");
    assert!(error.to_string().contains("replaced"), "{error}");
    assert_eq!(user_version(&db), 7, "the replacement must not be migrated");
    let found = inspect_upgrade_backups(&db).unwrap();
    assert_eq!(found.len(), 1);
    assert!(matches!(
        found[0].state,
        UpgradeBackupState::Verified { .. }
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_interrupted_upgrade_keeps_the_original_and_verified_backup_evidence() {
    let root = temp_root("interrupted");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let original = fs::read(&db).unwrap();
    let _guard = HookGuard::install(|stage, _| {
        if stage == "after-backup" {
            Err("simulated interruption".into())
        } else {
            Ok(())
        }
    });
    let error = upgrade(&db).err().unwrap().to_string();
    assert!(error.contains("recoverable from"), "{error}");
    assert_eq!(fs::read(&db).unwrap(), original);
    let found = inspect_upgrade_backups(&db).unwrap();
    assert_eq!(found.len(), 1);
    assert!(matches!(
        found[0].state,
        UpgradeBackupState::Verified { .. }
    ));
    assert!(receipt_path_for(&found[0].backup_path).is_file());
    let _ = fs::remove_dir_all(root);
}

// ---- 5/6. rollback journal and WAL state --------------------------------------

/// Builds `<dir>/library.sqlite3` plus a genuinely hot rollback journal: a
/// write transaction spills modified pages to the database file while the
/// journal still holds the originals, and both files are copied mid-flight.
fn make_hot_journal_database(dir: &Path) -> PathBuf {
    let staging = dir.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let live = staging.join("library.sqlite3");
    make_schema_seven(&live);
    let connection = Connection::open(&live).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA synchronous=OFF; PRAGMA cache_size=5;
             CREATE TABLE blobs (id INTEGER PRIMARY KEY, body TEXT);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<400)
             INSERT INTO blobs (body) SELECT hex(zeroblob(1500)) FROM n;
             PRAGMA user_version = 7;",
        )
        .unwrap();
    connection
        .execute_batch("BEGIN; UPDATE blobs SET body = hex(randomblob(1500));")
        .unwrap();
    let journal = staging.join("library.sqlite3-journal");
    assert!(
        fs::metadata(&journal)
            .map(|m| m.len() > 512)
            .unwrap_or(false),
        "fixture must produce a populated rollback journal"
    );
    let target = dir.join("library.sqlite3");
    fs::copy(&live, &target).unwrap();
    fs::copy(&journal, dir.join("library.sqlite3-journal")).unwrap();
    drop(connection);
    target
}

#[test]
fn a_hot_rollback_journal_is_refused_everywhere_and_never_recovered() {
    let root = temp_root("hot-journal");
    let db = make_hot_journal_database(&root);
    let journal = root.join("library.sqlite3-journal");
    let before_db = fs::read(&db).unwrap();
    let before_journal = fs::read(&journal).unwrap();

    // Read-only inspection must not perform recovery writes.
    let _ = super::check_database_health(&db);
    let _ = super::diagnose_database(&db);
    assert!(Database::open_read_only(&db).is_err());
    assert_eq!(fs::read(&db).unwrap(), before_db);
    assert_eq!(fs::read(&journal).unwrap(), before_journal);

    // Neither the upgrade nor a direct open may recover it silently.
    let error = upgrade(&db).err().unwrap().to_string();
    assert!(error.contains("journal"), "{error}");
    let error = Database::open_or_create(&db).err().unwrap().to_string();
    assert!(error.contains("journal"), "{error}");
    assert_eq!(fs::read(&db).unwrap(), before_db);
    assert_eq!(fs::read(&journal).unwrap(), before_journal);
    assert!(backup_artifacts(&root).is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_zeroed_non_hot_journal_does_not_block_a_normal_upgrade() {
    let root = temp_root("zero-journal");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    fs::write(root.join("library.sqlite3-journal"), vec![0_u8; 600]).unwrap();
    upgrade(&db).unwrap();
    assert_eq!(user_version(&db), 8);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_garbage_rollback_journal_is_refused_not_ignored() {
    let root = temp_root("garbage-journal");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    fs::write(root.join("library.sqlite3-journal"), vec![0x5a_u8; 4096]).unwrap();
    let before = snapshot(&root);
    assert!(upgrade(&db).is_err());
    assert!(Database::open_or_create(&db).is_err());
    // (user_version cannot be read through SQLite here: the journal would
    // demand recovery, which is exactly what must not happen.)
    assert_eq!(
        snapshot(&root)
            .into_iter()
            .filter(|(name, _)| !name.ends_with(".upgrade.lock"))
            .collect::<Vec<_>>(),
        before
    );
    let _ = fs::remove_dir_all(root);
}

/// Schema-7 WAL database with committed frames left in the WAL: copies of the
/// main file and WAL taken while the writer is still open (no -shm copied).
fn make_wal_database(dir: &Path) -> PathBuf {
    let staging = dir.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let live = staging.join("library.sqlite3");
    make_schema_seven(&live);
    let connection = Connection::open(&live).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .unwrap();
    connection
        .execute("INSERT INTO items (name) VALUES ('only-in-wal')", [])
        .unwrap();
    let target = dir.join("library.sqlite3");
    fs::copy(&live, &target).unwrap();
    fs::copy(
        staging.join("library.sqlite3-wal"),
        dir.join("library.sqlite3-wal"),
    )
    .unwrap();
    drop(connection);
    target
}

#[test]
fn a_valid_wal_is_preserved_through_the_online_backup() {
    let root = temp_root("wal-valid");
    let db = make_wal_database(&root);
    let report = upgrade(&db).unwrap();
    let backup = Connection::open_with_flags(
        &report.backup_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let names: i64 = backup
        .query_row(
            "SELECT COUNT(*) FROM items WHERE name='only-in-wal'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(names, 1, "committed WAL content must be in the backup");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn damaged_missing_or_inconsistent_wal_state_is_refused() {
    type Damage = fn(&Path);
    let cases: [(&str, Damage); 5] = [
        ("flipped-frame-byte", |dir| {
            let wal = dir.join("library.sqlite3-wal");
            let mut bytes = fs::read(&wal).unwrap();
            let index = 32 + 24 + 100;
            bytes[index] ^= 0xff;
            fs::write(&wal, bytes).unwrap();
        }),
        ("torn-trailing-frame", |dir| {
            let wal = dir.join("library.sqlite3-wal");
            let bytes = fs::read(&wal).unwrap();
            fs::write(&wal, &bytes[..bytes.len() - 100]).unwrap();
        }),
        ("bad-header-magic", |dir| {
            let wal = dir.join("library.sqlite3-wal");
            let mut bytes = fs::read(&wal).unwrap();
            bytes[0] ^= 0xff;
            fs::write(&wal, bytes).unwrap();
        }),
        ("truncated-header", |dir| {
            fs::write(
                dir.join("library.sqlite3-wal"),
                [0x37_u8, 0x7f, 0x06, 0x82, 0, 0],
            )
            .unwrap();
        }),
        ("shm-without-wal", |dir| {
            fs::remove_file(dir.join("library.sqlite3-wal")).unwrap();
            fs::write(dir.join("library.sqlite3-shm"), vec![0_u8; 32768]).unwrap();
        }),
    ];
    for (name, damage) in cases {
        let root = temp_root(name);
        let db = make_wal_database(&root);
        damage(&root);
        let before = snapshot(&root);
        let error = upgrade(&db)
            .err()
            .unwrap_or_else(|| panic!("{name}: accepted"));
        assert!(
            error.to_string().to_lowercase().contains("wal")
                || error.to_string().to_lowercase().contains("shm"),
            "{name}: {error}"
        );
        assert!(backup_artifacts(&root).is_empty(), "{name}");
        let after: Vec<_> = snapshot(&root)
            .into_iter()
            .filter(|(file, _)| !file.ends_with(".upgrade.lock"))
            .collect();
        let before: Vec<_> = before
            .into_iter()
            .filter(|(file, _)| !file.ends_with(".upgrade.lock"))
            .collect();
        assert_eq!(after, before, "{name}: files must be untouched");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn a_wal_beside_a_rollback_mode_database_is_inconsistent() {
    let root = temp_root("wal-mode-mismatch");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    fs::write(root.join("library.sqlite3-wal"), vec![0_u8; 4096]).unwrap();
    // Zero-filled content is not a valid WAL header either.
    let error = upgrade(&db).err().unwrap().to_string().to_lowercase();
    assert!(error.contains("wal"), "{error}");
    assert_eq!(user_version(&db), 7);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn schema_version_helper_is_usable_on_the_synthetic_fixture() {
    let root = temp_root("fixture");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let connection = Connection::open(&db).unwrap();
    assert_eq!(schema_version(&connection).unwrap(), 7);
    let _ = fs::remove_dir_all(root);
}
