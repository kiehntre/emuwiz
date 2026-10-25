//! A library database made by the published v0.9.0 release (not one rebuilt
//! from today's migration list) must upgrade through the protected upgrade
//! that the GUI's "Scan and upgrade library…" uses, keep every row, and leave
//! a verified backup of the exact original bytes. Ordinary opens refuse it
//! without writing.

use archivefs_core::{Database, upgrade_library_database};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

const FIXTURE: &str = include_str!("fixtures/published_v0_9_0/library.sql");
const HISTORICAL_TABLES: &[&str] = &[
    "archives",
    "archive_scan_observations",
    "discovery_details",
    "platform_assignments",
    "scan_fingerprints",
    "scan_runs",
    "source_folders",
];

fn sha256(path: &Path) -> String {
    Sha256::digest(fs::read(path).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn rows(path: &Path, table: &str) -> Vec<String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let columns: Vec<String> = connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    // Compare the columns v0.9.0 had, as text, in a stable order.
    let select = columns
        .iter()
        .map(|c| format!("quote(\"{c}\")"))
        .collect::<Vec<_>>()
        .join(" || '|' || ");
    connection
        .prepare(&format!("SELECT {select} FROM \"{table}\" ORDER BY rowid"))
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn columns(path: &Path, table: &str) -> Vec<String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn a_published_v0_9_0_database_upgrades_with_a_verified_backup_and_keeps_every_row() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.sqlite3");
    Connection::open(&path)
        .unwrap()
        .execute_batch(FIXTURE)
        .unwrap();
    let original_sha = sha256(&path);
    let before: Vec<(&str, Vec<String>, Vec<String>)> = HISTORICAL_TABLES
        .iter()
        .map(|t| (*t, columns(&path, t), rows(&path, t)))
        .collect();
    assert_eq!(
        before
            .iter()
            .find(|(t, ..)| *t == "archives")
            .unwrap()
            .2
            .len(),
        5
    );

    // Normal startup and opens never migrate.
    let refused = Database::open_or_create(&path)
        .err()
        .expect("schema 16 is refused");
    assert!(refused.to_string().contains("uses schema 16"), "{refused}");
    assert_eq!(
        sha256(&path),
        original_sha,
        "a refused open wrote to the database"
    );

    let report = upgrade_library_database(&path).unwrap();
    assert_eq!(report.from_version, 16);
    let latest = report.to_version;
    assert_eq!(report.applied_versions, (17..=latest).collect::<Vec<_>>());
    assert_eq!(
        Database::open_or_create(&path)
            .unwrap()
            .schema_version()
            .unwrap(),
        latest
    );

    // The backup is a consistent copy of the original schema and data.
    assert!(report.backup_path.is_file());
    assert_eq!(sha256(&report.backup_path), report.backup_sha256);
    let backup_version: i64 =
        Connection::open_with_flags(&report.backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
    assert_eq!(backup_version, 16);
    for (table, old_columns, old_rows) in &before {
        assert_eq!(
            &columns(&report.backup_path, table),
            old_columns,
            "backup {table} columns"
        );
        assert_eq!(
            &rows(&report.backup_path, table),
            old_rows,
            "backup {table} rows"
        );
    }

    // Every v0.9.0 column and row survives the migration chain.
    for (table, old_columns, old_rows) in &before {
        let new_columns = columns(&path, table);
        for column in old_columns {
            assert!(new_columns.contains(column), "{table}.{column} was dropped");
        }
        assert_eq!(
            rows(&path, table).len(),
            old_rows.len(),
            "{table} row count changed"
        );
    }
    let integrity: String = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");

    // Running the upgrade again never re-migrates or changes the database.
    let after_sha = sha256(&path);
    if let Ok(again) = upgrade_library_database(&path) {
        assert!(
            again.applied_versions.is_empty(),
            "{:?}",
            again.applied_versions
        );
    }
    assert_eq!(sha256(&path), after_sha);
}
