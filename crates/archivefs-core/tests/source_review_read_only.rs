//! Public review/preview APIs must never create or upgrade a library database.
//! Every file and database in these regressions is a private synthetic fixture.

archivefs_core::install_test_environment!();

use archivefs_core::{
    Archive, Database, preview_source_platform_assignment_at, review_source_rebind_at,
};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

type ReviewApi = fn(&Path, &Path) -> archivefs_core::Result<()>;

const APIS: [(&str, ReviewApi); 2] = [
    ("rebind review", |db, source| {
        review_source_rebind_at(db, source).map(|_| ())
    }),
    ("platform preview", |db, source| {
        preview_source_platform_assignment_at(db, source).map(|_| ())
    }),
];

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>, SystemTime)> {
    let mut files: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let metadata = fs::metadata(&path).unwrap();
            let bytes = if metadata.is_file() {
                fs::read(&path).unwrap()
            } else {
                Vec::new()
            };
            (path, bytes, metadata.modified().unwrap())
        })
        .collect();
    files.sort();
    files
}

fn missing_database_is_untouched(name: &str, call: ReviewApi) {
    for nested in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let db = if nested {
            temp.path().join("missing/parents/library.sqlite3")
        } else {
            temp.path().join("library.sqlite3")
        };
        let before = snapshot(temp.path());
        let error = call(&db, &temp.path().join("source")).unwrap_err();
        assert!(snapshot(temp.path()) == before, "{name} created files");
        assert!(!db.exists(), "{name} created the missing database");
        assert!(
            error.to_string().contains("does not exist"),
            "{name}: {error}"
        );
    }
}

#[test]
fn rebind_review_does_not_create_a_missing_database_or_its_parents() {
    missing_database_is_untouched(APIS[0].0, APIS[0].1);
}

#[test]
fn platform_preview_does_not_create_a_missing_database_or_its_parents() {
    missing_database_is_untouched(APIS[1].0, APIS[1].1);
}

#[test]
fn valid_database_reviews_return_real_results_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("library.sqlite3");
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let member = source.join("unknown.zip");
    fs::write(&member, b"synthetic archive bytes").unwrap();
    let mut db = Database::open_or_create(&db_path).unwrap();
    let source_id = db
        .register_source_folders(std::slice::from_ref(&source))
        .unwrap()[0]
        .id;
    let archive = Archive::from_path_in_root(&member, &source).unwrap();
    db.upsert_archive(source_id, &source, &archive).unwrap();
    // A historical, unbound source has a genuine storage review to display.
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute(
            "UPDATE source_folders SET last_successful_scan_at='historical-scan'",
            [],
        )
        .unwrap();
    connection.close().unwrap();
    let expected_review = db.review_source_rebind(source_id).unwrap();
    db.close().unwrap();
    let before = snapshot(temp.path());
    for _ in 0..2 {
        assert_eq!(
            review_source_rebind_at(&db_path, &source).unwrap(),
            expected_review
        );
        assert_eq!(
            preview_source_platform_assignment_at(&db_path, &source).unwrap(),
            1
        );
        assert!(
            snapshot(temp.path()) == before,
            "review or preview wrote files"
        );
        assert_eq!(fs::read(&member).unwrap(), b"synthetic archive bytes");
    }
}

#[test]
fn outdated_database_reviews_refuse_without_upgrade_or_backup() {
    for (name, call) in APIS {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("library.sqlite3");
        let connection = Connection::open(&db).unwrap();
        connection.execute_batch("CREATE TABLE precious(value TEXT); INSERT INTO precious VALUES ('preserve me'); PRAGMA user_version = 7;").unwrap();
        connection.close().unwrap();
        let before = snapshot(temp.path());
        let error = call(&db, &temp.path().join("source")).unwrap_err();
        assert!(error.to_string().contains("schema"), "{name}: {error}");
        assert!(
            snapshot(temp.path()) == before,
            "{name} upgraded or created artifacts"
        );
        let read_only =
            Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let version: i64 = read_only
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 7);
        let preserved: String = read_only
            .query_row("SELECT value FROM precious", [], |row| row.get(0))
            .unwrap();
        assert_eq!(preserved, "preserve me");
    }
}
