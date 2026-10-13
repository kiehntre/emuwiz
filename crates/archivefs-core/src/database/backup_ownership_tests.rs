//! Synthetic-database tests for upgrade-backup ownership, capacity and retention.
//! Only temporary SQLite files are used; no real library database is touched.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::backup_ownership::{
    BackupReservation, SourceIdentity, UpgradeLock, preflight, receipt_path_for,
};
use super::{
    MAX_RETAINED_UPGRADE_BACKUPS, Migration, UpgradeBackupState, inspect_upgrade_backups,
    upgrade_library_database_with_migrations,
};

const STEP_EIGHT: [Migration; 1] = [Migration {
    version: 8,
    description: "synthetic step eight",
    sql: "CREATE TABLE step_eight (id INTEGER PRIMARY KEY);",
}];

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "emuwiz-backup-ownership-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

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

fn file_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

fn listing(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut rows: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let bytes = fs::read(entry.path()).unwrap_or_default();
            (entry.file_name().to_string_lossy().into_owned(), bytes)
        })
        .collect();
    rows.sort();
    rows
}

fn upgrade(path: &Path) -> crate::Result<crate::DatabaseUpgradeReport> {
    upgrade_library_database_with_migrations(path, &STEP_EIGHT)
}

#[test]
fn successful_upgrade_binds_the_backup_to_a_complete_verified_receipt() {
    let root = temp_root("success");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);

    let report = upgrade(&db).unwrap();
    assert_eq!(report.from_version, 7);
    assert_eq!(report.to_version, 8);
    assert_eq!(
        report.backup_path,
        root.join("library.sqlite3.schema-7-before-8.backup")
    );
    assert_eq!(
        fs::metadata(&report.backup_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let receipt = receipt_path_for(&report.backup_path);
    assert_eq!(
        fs::metadata(&receipt).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let found = inspect_upgrade_backups(&db).unwrap();
    assert_eq!(found.len(), 1);
    match &found[0].state {
        UpgradeBackupState::Verified {
            from_version,
            to_version,
            len,
            sha256,
        } => {
            assert_eq!((*from_version, *to_version), (7, 8));
            assert_eq!(*len, fs::metadata(&report.backup_path).unwrap().len());
            assert_eq!(sha256, &report.backup_sha256);
        }
        other => panic!("expected Verified, got {other:?}"),
    }
    // the SQLite online-backup behaviour is unchanged: the snapshot holds schema 7
    let backup = Connection::open(&report.backup_path).unwrap();
    let version: i64 = backup
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 7);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_interrupted_reservation_is_incomplete_and_never_adopted_after_restart() {
    let root = temp_root("interrupted");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let identity = SourceIdentity::capture(&db).unwrap();
    // Simulate a crash: the name and in-progress receipt exist, nothing finished.
    let reservation = BackupReservation::reserve(&db, 7, 8, &identity).unwrap();
    let stale = reservation.backup_path.clone();
    std::mem::forget(reservation);
    fs::write(&stale, b"partial snapshot bytes").unwrap();
    let before = file_bytes(&stale);

    let incomplete = inspect_upgrade_backups(&db).unwrap();
    assert_eq!(incomplete[0].state, UpgradeBackupState::Incomplete);

    // "Restart": a new upgrade must not treat the old file as its backup.
    let report = upgrade(&db).unwrap();
    assert_ne!(report.backup_path, stale);
    assert!(report.backup_path.to_string_lossy().ends_with(".backup.1"));
    assert_eq!(
        file_bytes(&stale),
        before,
        "the incomplete backup is left untouched"
    );
    let states: Vec<_> = inspect_upgrade_backups(&db)
        .unwrap()
        .into_iter()
        .map(|item| (item.backup_path, item.state))
        .collect();
    assert!(
        states
            .iter()
            .any(|(p, s)| p == &stale && *s == UpgradeBackupState::Incomplete)
    );
    assert!(
        states
            .iter()
            .any(|(p, s)| p == &report.backup_path
                && matches!(s, UpgradeBackupState::Verified { .. }))
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_unowned_file_with_a_backup_name_is_not_adopted_replaced_or_trusted() {
    let root = temp_root("unowned");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let squatter = root.join("library.sqlite3.schema-7-before-8.backup");
    fs::write(&squatter, b"someone else's file").unwrap();

    let report = upgrade(&db).unwrap();
    assert_eq!(file_bytes(&squatter), b"someone else's file");
    assert_ne!(report.backup_path, squatter);
    let by_path: Vec<_> = inspect_upgrade_backups(&db).unwrap();
    assert!(
        by_path
            .iter()
            .any(|item| item.backup_path == squatter && item.state == UpgradeBackupState::Unowned)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_leftover_receipt_without_its_backup_is_skipped_not_overwritten() {
    let root = temp_root("stale-receipt");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let name = root.join("library.sqlite3.schema-7-before-8.backup");
    let stale_receipt = receipt_path_for(&name);
    fs::write(&stale_receipt, b"{\"leftover\":true}").unwrap();

    let report = upgrade(&db).unwrap();
    assert_eq!(file_bytes(&stale_receipt), b"{\"leftover\":true}");
    assert!(report.backup_path.to_string_lossy().ends_with(".backup.1"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_retention_cap_refuses_before_anything_is_created_or_changed() {
    let root = temp_root("cap");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    for index in 0..MAX_RETAINED_UPGRADE_BACKUPS {
        let suffix = if index == 0 {
            String::new()
        } else {
            format!(".{index}")
        };
        fs::write(
            root.join(format!("library.sqlite3.schema-7-before-8.backup{suffix}")),
            format!("retained {index}"),
        )
        .unwrap();
    }
    let before = listing(&root);

    let error = upgrade(&db).unwrap_err().to_string();
    assert!(error.contains("already retained"), "{error}");
    assert!(error.contains("Nothing was changed"), "{error}");
    // only the lock file may have been created; every existing byte is intact
    let after: Vec<_> = listing(&root)
        .into_iter()
        .filter(|(name, _)| !name.ends_with(".upgrade.lock"))
        .collect();
    assert_eq!(after, before);
    let connection = Connection::open(&db).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 7, "the database was not migrated");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn insufficient_capacity_is_refused_up_front() {
    let root = temp_root("capacity");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let error = preflight(&db, u64::MAX / 2).unwrap_err().to_string();
    assert!(error.contains("not enough free space"), "{error}");
    assert!(error.contains("Nothing was changed"), "{error}");
    preflight(&db, 1024).unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_replaced_database_identity_is_detected_before_migration() {
    let root = temp_root("identity");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let identity = SourceIdentity::capture(&db).unwrap();
    identity.recheck(&db).unwrap();

    let replacement = root.join("replacement.sqlite3");
    make_schema_seven(&replacement);
    fs::rename(&replacement, &db).unwrap();
    let error = identity.recheck(&db).unwrap_err().to_string();
    assert!(error.contains("replaced"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_symlinked_library_database_is_never_upgraded() {
    let root = temp_root("symlink-db");
    let real = root.join("real.sqlite3");
    make_schema_seven(&real);
    let link = root.join("library.sqlite3");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let error = upgrade(&link).unwrap_err().to_string();
    assert!(error.contains("not a regular file"), "{error}");
    assert!(inspect_upgrade_backups(&link).unwrap().is_empty());
    assert!(
        !root
            .join("library.sqlite3.schema-7-before-8.backup")
            .exists()
    );
    let _ = fs::remove_dir_all(root);
}

fn upgraded_fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = temp_root(name);
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let report = upgrade(&db).unwrap();
    (root, db, report.backup_path)
}

fn state_of(db: &Path, backup: &Path) -> UpgradeBackupState {
    inspect_upgrade_backups(db)
        .unwrap()
        .into_iter()
        .find(|item| item.backup_path == backup)
        .unwrap()
        .state
}

#[test]
fn missing_or_corrupted_ownership_evidence_is_never_verified() {
    let (root, db, backup) = upgraded_fixture("evidence");
    let receipt = receipt_path_for(&backup);
    let good_receipt = file_bytes(&receipt);
    let good_backup = file_bytes(&backup);

    fs::write(&receipt, b"not json at all").unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Inconsistent(_)
    ));

    fs::write(
        &receipt,
        String::from_utf8(good_receipt.clone())
            .unwrap()
            .replace("complete", "in_progress"),
    )
    .unwrap();
    assert_eq!(state_of(&db, &backup), UpgradeBackupState::Incomplete);

    fs::write(&receipt, &good_receipt).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Verified { .. }
    ));

    let mut flipped = good_backup.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0xff;
    fs::write(&backup, &flipped).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Inconsistent(_)
    ));
    fs::write(&backup, &good_backup).unwrap();

    fs::remove_file(&receipt).unwrap();
    assert_eq!(state_of(&db, &backup), UpgradeBackupState::Unowned);

    // a receipt for a different database file name is not accepted
    fs::write(
        &receipt,
        String::from_utf8(good_receipt)
            .unwrap()
            .replace(&hex_of("library.sqlite3"), &hex_of("other.sqlite3")),
    )
    .unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Inconsistent(_)
    ));
    let _ = fs::remove_dir_all(root);
}

fn hex_of(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn symlink_and_hardlink_substitution_are_inconsistent_not_verified() {
    let (root, db, backup) = upgraded_fixture("substitution");
    let receipt = receipt_path_for(&backup);
    let good_backup = file_bytes(&backup);

    // hardlink alias
    let alias = root.join("alias");
    fs::hard_link(&backup, &alias).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Inconsistent(_)
    ));
    fs::remove_file(&alias).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Verified { .. }
    ));

    // backup swapped for a symlink to an identical file elsewhere
    let elsewhere = root.join("elsewhere");
    fs::write(&elsewhere, &good_backup).unwrap();
    fs::remove_file(&backup).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &backup).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Inconsistent(_)
    ));
    fs::remove_file(&backup).unwrap();
    fs::write(&backup, &good_backup).unwrap();
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o600)).unwrap();

    // receipt swapped for a symlink
    let receipt_copy = root.join("receipt-copy");
    fs::copy(&receipt, &receipt_copy).unwrap();
    fs::remove_file(&receipt).unwrap();
    std::os::unix::fs::symlink(&receipt_copy, &receipt).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Inconsistent(_)
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn concurrent_upgrade_attempts_fail_closed() {
    let root = temp_root("concurrent");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let held = UpgradeLock::acquire(&db).unwrap();
    let before = file_bytes(&db);
    let error = upgrade(&db).unwrap_err().to_string();
    assert!(error.contains("already running"), "{error}");
    assert_eq!(file_bytes(&db), before);
    assert!(inspect_upgrade_backups(&db).unwrap().is_empty());
    drop(held);
    upgrade(&db).unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn inaccessible_recovery_material_is_reported_not_assumed_good() {
    if unsafe { libc::geteuid() } == 0 {
        return; // permission bits do not bind root
    }
    let (root, db, backup) = upgraded_fixture("inaccessible");
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o000)).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Inaccessible(_)
    ));
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o600)).unwrap();
    let receipt = receipt_path_for(&backup);
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o000)).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Inaccessible(_)
    ));
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        state_of(&db, &backup),
        UpgradeBackupState::Verified { .. }
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn inspection_is_read_only_and_grants_no_authority() {
    let (root, db, _backup) = upgraded_fixture("read-only");
    let before = listing(&root);
    let first = inspect_upgrade_backups(&db).unwrap();
    let second = inspect_upgrade_backups(&db).unwrap();
    assert_eq!(first, second);
    assert_eq!(listing(&root), before);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_failed_migration_keeps_a_verified_owned_backup() {
    let root = temp_root("failed-migration");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    let failing = [Migration {
        version: 8,
        description: "invalid",
        sql: "THIS IS DELIBERATELY INVALID SQL;",
    }];
    let error = upgrade_library_database_with_migrations(&db, &failing)
        .unwrap_err()
        .to_string();
    assert!(error.contains("recoverable from"), "{error}");
    let found = inspect_upgrade_backups(&db).unwrap();
    assert_eq!(found.len(), 1);
    assert!(matches!(
        found[0].state,
        UpgradeBackupState::Verified {
            from_version: 7,
            ..
        }
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn only_recognised_backup_names_count_toward_the_cap() {
    let root = temp_root("names");
    let db = root.join("library.sqlite3");
    make_schema_seven(&db);
    for name in [
        "library.sqlite3.schema-7-before-8.backup.receipt.json",
        "library.sqlite3.schema-x-before-8.backup",
        "library.sqlite3.schema-7-before-8.backup.tmp",
        "other.sqlite3.schema-7-before-8.backup",
        "library.sqlite3.schema-7-before-8.backup.abc",
        "notes.txt",
    ] {
        fs::write(root.join(name), b"x").unwrap();
    }
    assert!(inspect_upgrade_backups(&db).unwrap().is_empty());
    for index in 0..3 {
        let suffix = if index == 0 {
            String::new()
        } else {
            format!(".{index}")
        };
        fs::write(
            root.join(format!("library.sqlite3.schema-5-before-6.backup{suffix}")),
            b"x",
        )
        .unwrap();
    }
    assert_eq!(inspect_upgrade_backups(&db).unwrap().len(), 3);
    let _ = fs::remove_dir_all(root);
}
