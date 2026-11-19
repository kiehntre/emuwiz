//! Schema 30: exact skipped-file totals per scan run, source and reason, on
//! disposable libraries only.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{
    Config, Database, inspect_upgrade_backups, scan_and_persist, upgrade_library_database,
};
use crate::SkipReasonCounts;
use crate::attention::{AttentionCategory, AttentionSeverity};

fn config(sources: &[PathBuf], root: &Path) -> Config {
    Config {
        configured_source_roots: Some(sources.to_vec()),
        source_folders: sources.to_vec(),
        mount_root: root.join("mount"),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    }
}

fn rom(seed: usize) -> Vec<u8> {
    (0..2048usize)
        .map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed as u8))
        .collect()
}

/// `count` loose Atari 2600 cartridges, each byte-identical to the only file
/// of a ZIP beside it.
fn identical_copies(folder: &Path, count: usize) {
    fs::create_dir_all(folder).unwrap();
    for index in 0..count {
        let bytes = rom(index);
        let mut writer = zip::ZipWriter::new(
            fs::File::create(folder.join(format!("Game {index}.zip"))).unwrap(),
        );
        writer
            .start_file(
                format!("Game {index} (USA).bin"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap();
        fs::write(folder.join(format!("Game {index} (USA).bin")), &bytes).unwrap();
    }
}

fn source_id(database: &Database, path: &Path) -> i64 {
    database
        .connection
        .query_row(
            "SELECT id FROM source_folders WHERE path = ?1",
            [path.as_os_str().as_encoded_bytes()],
            |row| row.get(0),
        )
        .unwrap()
}

fn discovery_items(
    database: &Database,
) -> Vec<(String, AttentionCategory, AttentionSeverity, u64)> {
    let snapshot = database.attention_snapshot().unwrap();
    let mut items: Vec<_> = snapshot
        .items()
        .filter(|item| item.id.starts_with("discovery:"))
        .map(|item| {
            (
                item.id.clone(),
                item.category,
                item.severity,
                item.affected_count,
            )
        })
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

/// Run 211's failure, generalised: more than 16,384 skipped files, the
/// identical copies met only after every bounded sample is full, two sources,
/// and a rescan. Every total stays exact, per source and per run, and every
/// report reads the same stored figures.
#[test]
fn totals_stay_exact_past_every_sample_limit_across_sources_and_rescans() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("games-a");
    let second = root.path().join("games-b");
    fs::create_dir_all(&first).unwrap();
    // A source's own files are walked before its subfolders, so every
    // identical copy below is met after the 16,384 fingerprint budget and the
    // 1,000-entry skipped-file sample are both full.
    let unsupported = crate::MAX_RETAINED_NON_ARCHIVE_FINGERPRINTS + 116;
    for index in 0..unsupported {
        fs::write(first.join(format!("note{index}.xyz")), b"x").unwrap();
    }
    identical_copies(&first.join("atari2600"), 20);
    // The second source: two unsupported files, one ambiguous `.bin`.
    fs::create_dir_all(second.join("misc")).unwrap();
    fs::write(second.join("readme.xyz"), b"r").unwrap();
    fs::write(second.join("misc/notes.xyz"), b"n").unwrap();
    fs::write(second.join("misc/data.bin"), rom(99)).unwrap();

    let config = config(&[first.clone(), second.clone()], root.path());
    let mut database = Database::open_or_create(root.path().join("library.sqlite3")).unwrap();
    let summary = scan_and_persist(&mut database, &config, "test").unwrap();
    let expected_first = SkipReasonCounts {
        unsupported_extension: unsupported as u64,
        identical_copy: 20,
        ..Default::default()
    };
    let expected_second = SkipReasonCounts {
        unsupported_extension: 2,
        ambiguous_platform: 1,
        ..Default::default()
    };
    let mut expected_run = expected_first;
    expected_run.merge(&expected_second);

    // Stored per source, and per run.
    let by_source = database
        .scan_run_skip_counts_by_source(summary.scan_run_id)
        .unwrap();
    assert_eq!(by_source[&source_id(&database, &first)], expected_first);
    assert_eq!(by_source[&source_id(&database, &second)], expected_second);
    assert_eq!(
        database.scan_run_skip_counts(summary.scan_run_id).unwrap(),
        Some(expected_run)
    );
    // The scan report carries the stored figures, which agree with the coarse
    // historical columns they refine.
    assert_eq!(summary.counts.skip_reasons, Some(expected_run));
    assert_eq!(
        summary.counts.skipped_unsupported_extension as u64,
        expected_run.total() - expected_run.ambiguous_platform
    );
    assert_eq!(
        summary.counts.skipped_ambiguous_platform as u64,
        expected_run.ambiguous_platform
    );
    // The samples stayed bounded while the totals did not depend on them.
    assert!(summary.skipped_files.len() <= crate::MAX_RETAINED_SKIPPED_FILES);
    let fingerprints: i64 = database
        .connection
        .query_row(
            "SELECT COUNT(*) FROM scan_fingerprints WHERE non_archive_kind IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(fingerprints <= 2 * crate::MAX_RETAINED_NON_ARCHIVE_FINGERPRINTS as i64);
    // Only the ZIPs became games.
    assert_eq!(summary.counts.archives_added, 20);
    // Attention: one item per reason, duplicates apart and informational.
    assert_eq!(
        discovery_items(&database),
        [
            (
                "discovery:1".to_string(),
                AttentionCategory::Unsupported,
                AttentionSeverity::Warning,
                unsupported as u64 + 2
            ),
            (
                "discovery:2".to_string(),
                AttentionCategory::Identity,
                AttentionSeverity::Warning,
                1
            ),
            (
                "discovery:3".to_string(),
                AttentionCategory::Duplicates,
                AttentionSeverity::Info,
                20
            ),
        ]
    );

    // A rescan stores its own totals; the first run's stay as recorded.
    fs::remove_file(second.join("readme.xyz")).unwrap();
    let rescan = scan_and_persist(&mut database, &config, "test").unwrap();
    assert_ne!(rescan.scan_run_id, summary.scan_run_id);
    let mut expected_rescan = expected_run;
    expected_rescan.unsupported_extension -= 1;
    assert_eq!(rescan.counts.skip_reasons, Some(expected_rescan));
    assert_eq!(
        database.scan_run_skip_counts(summary.scan_run_id).unwrap(),
        Some(expected_run)
    );
    assert_eq!(rescan.counts.archives_added, 0);
    assert_eq!(rescan.counts.archives_missing, 0);
}

/// Every row of every table except the new (empty) one, as text.
fn all_rows(path: &Path) -> std::collections::BTreeMap<String, Vec<String>> {
    let c = Connection::open(path).unwrap();
    let tables: Vec<String> = c
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' \
             AND name <> 'scan_run_skip_counts' AND name <> 'schema_migrations'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    tables
        .into_iter()
        .map(|table| {
            let columns: Vec<String> = c
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let projection = columns
                .iter()
                .map(|column| format!("quote(\"{column}\")"))
                .collect::<Vec<_>>()
                .join(" || '|' || ");
            let mut rows: Vec<String> = c
                .prepare(&format!("SELECT {projection} FROM \"{table}\""))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows.sort();
            (table, rows)
        })
        .collect()
}

fn insert_run(database: &Database, id: i64, status: &str, unsupported: i64, ambiguous: i64) {
    database
        .connection
        .execute(
            "INSERT INTO scan_runs(id,started_at,finished_at,triggered_by,status,skipped_unsupported_extension,skipped_ambiguous_platform) VALUES(?1,'2026-02-01T10:00:00Z','2026-02-01T11:00:00Z','test',?2,?3,?4)",
            rusqlite::params![id, status, unsupported, ambiguous],
        )
        .unwrap();
}

fn store(database: &mut Database, run: i64, source: i64, counts: SkipReasonCounts) {
    database
        .record_scan_skip_counts(run, source, &counts)
        .unwrap();
}

/// Attention names every reason from the stored totals, adds sources
/// together, ignores runs that did not complete, and labels a run without a
/// breakdown honestly instead of calling its whole figure "unsupported".
#[test]
fn attention_uses_the_latest_completed_runs_stored_totals_for_every_reason() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open_or_create(directory.path().join("library.sqlite3")).unwrap();
    database.connection.execute_batch(
        "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at,last_scan_status) VALUES(1,x'2f61','2026-01-01','2026-01-01','success'),(2,x'2f62','2026-01-01','2026-01-01','success');",
    ).unwrap();

    // A run made before schema 30: no stored breakdown.
    insert_run(&database, 1, "completed", 12, 3);
    let legacy = discovery_items(&database);
    assert_eq!(legacy.len(), 2, "{legacy:?}");
    let snapshot = database.attention_snapshot().unwrap();
    let coarse = snapshot
        .items()
        .find(|item| item.id == "discovery:1")
        .unwrap();
    assert_eq!(coarse.affected_count, 12);
    assert!(
        coarse.title.contains("breakdown not recorded"),
        "{}",
        coarse.title
    );
    assert!(
        !coarse.title.contains("unsupported formats"),
        "{}",
        coarse.title
    );
    assert!(
        !snapshot
            .items()
            .any(|item| item.category == AttentionCategory::Duplicates)
    );

    // A completed run with a breakdown for two sources.
    insert_run(&database, 2, "completed", 0, 0);
    store(
        &mut database,
        2,
        1,
        SkipReasonCounts {
            unsupported_extension: 5,
            unsupported_variant: 2,
            invalid_content: 1,
            ambiguous_platform: 4,
            identical_copy: 300,
        },
    );
    store(
        &mut database,
        2,
        2,
        SkipReasonCounts {
            unsupported_extension: 1,
            identical_copy: 56,
            ..Default::default()
        },
    );
    // Interrupted and failed runs afterwards, with partial totals, never count.
    insert_run(&database, 3, "running", 0, 0);
    store(
        &mut database,
        3,
        1,
        SkipReasonCounts {
            identical_copy: 9_999,
            ..Default::default()
        },
    );
    insert_run(&database, 4, "failed", 0, 0);
    store(
        &mut database,
        4,
        2,
        SkipReasonCounts {
            unsupported_extension: 7_777,
            ..Default::default()
        },
    );
    use AttentionCategory as C;
    use AttentionSeverity as S;
    assert_eq!(
        discovery_items(&database),
        [
            ("discovery:1".to_string(), C::Unsupported, S::Warning, 6),
            ("discovery:2".to_string(), C::Identity, S::Warning, 4),
            ("discovery:3".to_string(), C::Duplicates, S::Info, 356),
            ("discovery:4".to_string(), C::Unsupported, S::Warning, 2),
            ("discovery:5".to_string(), C::Unsupported, S::Warning, 1),
        ]
    );
    let snapshot = database.attention_snapshot().unwrap();
    let duplicates = snapshot
        .items()
        .find(|item| item.id == "discovery:3")
        .unwrap();
    assert!(
        !duplicates.title.contains("unsupported"),
        "{}",
        duplicates.title
    );
    // A stored zero is a measurement: no item, and no fallback to the columns.
    assert!(
        !snapshot
            .items()
            .any(|item| item.title.contains("breakdown not recorded"))
    );
}

/// A schema-29 library upgrades by adding the table only: every existing row
/// is kept, older runs read as "not recorded" (never zero), the verified
/// backup is the schema-29 file to go back to, and the next scan records.
#[test]
fn a_schema_29_library_upgrades_keeps_older_runs_unrecorded_and_can_go_back() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("games");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("notes.xyz"), b"x").unwrap();
    identical_copies(&source.join("atari2600"), 2);
    let path = root.path().join("library.sqlite3");
    let config = config(std::slice::from_ref(&source), root.path());
    {
        let mut database = Database::open_or_create(&path).unwrap();
        scan_and_persist(&mut database, &config, "fixture").unwrap();
        database.close().unwrap();
    }
    // Take the library back to exactly what a schema-29 build leaves.
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "DROP TABLE scan_run_skip_counts; DELETE FROM schema_migrations WHERE version=30; PRAGMA user_version=29;",
        )
        .unwrap();
    let schema_29_rows = all_rows(&path);
    assert!(
        Database::open_read_only(&path).is_err(),
        "schema 29 is not current"
    );

    let report = upgrade_library_database(&path).unwrap();
    assert_eq!((report.from_version, report.to_version), (29, 30));
    assert_eq!(report.applied_versions, vec![30]);
    // Every existing row is kept, and going back is the verified backup: a
    // schema-29 library without the table, holding exactly the same rows.
    assert_eq!(all_rows(&path), schema_29_rows);
    assert_eq!(all_rows(&report.backup_path), schema_29_rows);
    let backup = Connection::open(&report.backup_path).unwrap();
    let version: i64 = backup
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 29);
    let has_table: bool = backup
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='scan_run_skip_counts')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!has_table);
    drop(backup);
    assert!(matches!(
        inspect_upgrade_backups(&path).unwrap()[0].state,
        crate::UpgradeBackupState::Verified {
            from_version: 29,
            to_version: 30,
            ..
        }
    ));

    let mut database = Database::open_or_create(&path).unwrap();
    let old_run: i64 = database
        .connection
        .query_row("SELECT MAX(id) FROM scan_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(database.scan_run_skip_counts(old_run).unwrap(), None);
    let history = database.recent_scan_runs(10).unwrap();
    assert_eq!(history[0].counts.as_ref().unwrap().skip_reasons, None);
    assert!(
        database
            .attention_snapshot()
            .unwrap()
            .items()
            .any(|item| item.title.contains("breakdown not recorded"))
    );
    // Nothing in the catalogue changed, and the next scan records a breakdown.
    let games: i64 = database
        .connection
        .query_row("SELECT COUNT(*) FROM archives", [], |row| row.get(0))
        .unwrap();
    assert_eq!(games, 2);
    let next = scan_and_persist(&mut database, &config, "test").unwrap();
    assert_eq!(
        next.counts.skip_reasons,
        Some(SkipReasonCounts {
            unsupported_extension: 1,
            identical_copy: 2,
            ..Default::default()
        })
    );
    let history = database.recent_scan_runs(10).unwrap();
    assert_eq!(
        history[0].counts.as_ref().unwrap().skip_reasons,
        next.counts.skip_reasons
    );
}
