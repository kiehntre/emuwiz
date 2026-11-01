//! Schema 26 to 27, on disposable libraries only: what the upgrade keeps, the
//! backup it makes, what an interruption leaves, and how to go back.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{
    Config, Database, MIGRATIONS, UPGRADE_STAGE_HOOK, inspect_upgrade_backups,
    open_or_create_with_migrations, prepare_database_restore, scan_and_persist,
    upgrade_library_database,
};
use crate::UpgradeBackupState;

const SCHEMA_26: usize = 26;

/// Every row of every table, as text, leaving out what schema 27 adds.
fn all_rows(path: &Path) -> BTreeMap<String, Vec<String>> {
    let c = Connection::open(path).unwrap();
    let tables: Vec<String> = c
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type='table' \
             AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'game_collection%'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut all = BTreeMap::new();
    for table in tables {
        let columns: Vec<String> = c
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let quoted: Vec<String> = columns
            .iter()
            .map(|column| format!("quote(\"{column}\")"))
            .collect();
        let filter = if table == "schema_migrations" {
            " WHERE version <= 26"
        } else {
            ""
        };
        let select = format!(
            "SELECT {} FROM \"{table}\"{filter} ORDER BY 1",
            quoted.join("||'|'||")
        );
        let rows = c
            .prepare(&select)
            .unwrap()
            .query_map([], |r| r.get::<_, Option<String>>(0))
            .unwrap()
            .map(|row| row.unwrap().unwrap_or_default())
            .collect();
        all.insert(table, rows);
    }
    all
}

fn user_version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn has_collection_tables(path: &Path) -> bool {
    Connection::open(path)
        .unwrap()
        .prepare("SELECT 1 FROM game_collections")
        .is_ok()
}

struct Library {
    _temp: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
}

/// A real scanned catalogue, with history and metadata rows, taken back to
/// schema 26 by removing exactly what migration 27 added.
fn schema_26_library() -> Library {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    for name in ["Alpha.zip", "Beta.zip", "Gamma.zip"] {
        fs::write(source.join(name), name.as_bytes()).unwrap();
    }
    let path = root.join("library.sqlite3");
    let config = Config {
        source_folders: vec![source],
        mount_root: root.join("mount"),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    };
    let mut database = Database::open_or_create(&path).unwrap();
    scan_and_persist(&mut database, &config, "fixture").unwrap();
    database.close().unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "INSERT INTO discovery_import_operations VALUES('fixture','committed','old',NULL,x'7b7d');
         INSERT INTO discovery_import_records
           SELECT 'fixture', id, source_folder_id, relative_path, absolute_path_cached, x'7b7d', 0
           FROM archives;
         INSERT INTO platform_aliases (alias, normalized_alias, platform, created_at, updated_at)
           VALUES ('My Folder', 'my folder', 'SNES', 'old', 'old');
         INSERT INTO platform_assignments (archive_id, platform, source, assigned_at)
           SELECT id, 'SNES', 'user', 'old' FROM archives;
         UPDATE archives SET last_known_health='Healthy';
         DROP TABLE game_collection_items; DROP TABLE game_collections;
         DELETE FROM schema_migrations WHERE version=27;
         PRAGMA user_version=26;",
    )
    .unwrap();
    drop(c);
    // Organisation and rename history is kept in journals beside the database.
    let journals = root.join("rename-transactions");
    fs::create_dir_all(&journals).unwrap();
    fs::write(journals.join("history.json"), b"{\"kept\":true}").unwrap();
    Library {
        _temp: temp,
        root,
        path,
    }
}

fn journal_bytes(root: &Path) -> Vec<u8> {
    fs::read(root.join("rename-transactions/history.json")).unwrap()
}

struct StageGuard;
impl StageGuard {
    fn fail_at(stage: &'static str) -> Self {
        UPGRADE_STAGE_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |at, _| {
                if at == stage {
                    Err("simulated interruption".into())
                } else {
                    Ok(())
                }
            }))
        });
        Self
    }
}
impl Drop for StageGuard {
    fn drop(&mut self) {
        UPGRADE_STAGE_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

#[test]
fn the_fixture_really_holds_the_records_being_protected() {
    let library = schema_26_library();
    let rows = all_rows(&library.path);
    for table in [
        "archives",
        "source_folders",
        "scan_runs",
        "platform_assignments",
        "platform_aliases",
        "discovery_import_operations",
        "discovery_import_records",
    ] {
        assert!(!rows[table].is_empty(), "{table} is empty");
    }
    assert_eq!(rows["archives"].len(), 3);
}

#[test]
fn upgrading_keeps_every_row_of_every_table_and_the_history_beside_it() {
    let library = schema_26_library();
    let before = all_rows(&library.path);
    let journal = journal_bytes(&library.root);
    let report = upgrade_library_database(&library.path).unwrap();
    assert_eq!((report.from_version, report.to_version), (26, 27));
    assert_eq!(report.applied_versions, vec![27]);
    assert_eq!(all_rows(&library.path), before);
    assert_eq!(journal_bytes(&library.root), journal);
    let database = Database::open_read_only(&library.path).unwrap();
    assert_eq!(database.load_archives().unwrap().len(), 3);
    let collections = database.load_game_collections().unwrap();
    assert_eq!(collections.len(), 1);
    assert!(collections[0].items.is_empty());
}

#[test]
fn the_backup_is_complete_verified_and_made_before_the_schema_changes() {
    let library = schema_26_library();
    let before = all_rows(&library.path);
    let database_at_backup = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = database_at_backup.clone();
    UPGRADE_STAGE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |stage, path| {
            if stage == "after-backup" {
                seen.set(user_version(path));
                assert!(
                    !has_collection_tables(path),
                    "the backup precedes any change"
                );
            }
            Ok(())
        }))
    });
    let report = upgrade_library_database(&library.path);
    UPGRADE_STAGE_HOOK.with(|slot| *slot.borrow_mut() = None);
    let report = report.unwrap();
    assert_eq!(database_at_backup.get(), 26);
    assert_eq!(user_version(&report.backup_path), 26);
    assert!(!has_collection_tables(&report.backup_path));
    assert_eq!(all_rows(&report.backup_path), before);
    let integrity: String = Connection::open(&report.backup_path)
        .unwrap()
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    assert!(matches!(
        inspect_upgrade_backups(&library.path).unwrap()[0].state,
        UpgradeBackupState::Verified {
            from_version: 26,
            to_version: 27,
            ..
        }
    ));
}

#[test]
fn an_upgrade_interrupted_before_writing_changes_nothing_and_can_be_repeated() {
    for stage in ["after-lock", "after-backup"] {
        let library = schema_26_library();
        let original = fs::read(&library.path).unwrap();
        {
            let _guard = StageGuard::fail_at(stage);
            assert!(upgrade_library_database(&library.path).is_err(), "{stage}");
        }
        assert_eq!(fs::read(&library.path).unwrap(), original, "{stage}");
        assert_eq!(user_version(&library.path), 26);
        assert!(Database::open_read_only(&library.path).is_err());
        let retried = upgrade_library_database(&library.path);
        assert!(retried.is_ok(), "{stage}: {:?}", retried.err());
        assert_eq!(user_version(&library.path), 27);
    }
}

#[test]
fn a_failure_inside_the_migration_rolls_back_whole_and_can_be_repeated() {
    let library = schema_26_library();
    let c = Connection::open(&library.path).unwrap();
    c.execute_batch(
        "CREATE TRIGGER reject_27 BEFORE INSERT ON schema_migrations WHEN NEW.version=27 \
         BEGIN SELECT RAISE(ABORT,'simulated failure'); END;",
    )
    .unwrap();
    drop(c);
    let before = all_rows(&library.path);
    let error = upgrade_library_database(&library.path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("recoverable from"), "{error}");
    assert_eq!(user_version(&library.path), 26);
    assert!(!has_collection_tables(&library.path));
    assert_eq!(all_rows(&library.path), before);
    let found = inspect_upgrade_backups(&library.path).unwrap();
    assert!(matches!(
        found[0].state,
        UpgradeBackupState::Verified { .. }
    ));
    Connection::open(&library.path)
        .unwrap()
        .execute_batch("DROP TRIGGER reject_27;")
        .unwrap();
    upgrade_library_database(&library.path).unwrap();
    assert_eq!(user_version(&library.path), 27);
    assert_eq!(all_rows(&library.path), before);
}

#[test]
fn a_new_build_never_upgrades_a_schema_26_library_by_itself() {
    let library = schema_26_library();
    let original = fs::read(&library.path).unwrap();
    assert!(Database::open_read_only(&library.path).is_err());
    assert!(Database::open_existing(&library.path).is_err());
    assert!(Database::open_or_create(&library.path).is_err());
    assert_eq!(fs::read(&library.path).unwrap(), original);
}

#[test]
fn a_schema_26_build_refuses_an_upgraded_library() {
    let library = schema_26_library();
    upgrade_library_database(&library.path).unwrap();
    let error = open_or_create_with_migrations(&library.path, &MIGRATIONS[..SCHEMA_26])
        .err()
        .unwrap()
        .to_string();
    assert!(!error.is_empty());
    assert_eq!(user_version(&library.path), 27);
}

/// The documented way back: close EmuWiz and put the backup in place. It
/// returns the exact schema 26 library, so favourites made since are gone.
#[test]
fn rolling_back_to_the_backup_restores_schema_26_and_loses_later_favourites() {
    let library = schema_26_library();
    let before = all_rows(&library.path);
    let report = upgrade_library_database(&library.path).unwrap();
    let mut database = Database::open_existing(&library.path).unwrap();
    let favourites = database.favourites_collection_id().unwrap();
    let game = database.load_archives().unwrap()[0].id;
    database.add_game_to_collection(favourites, game).unwrap();
    database.close().unwrap();

    // The built-in Restore only accepts a backup at the build's own schema.
    assert!(prepare_database_restore(&library.path, &report.backup_path).is_err());

    let kept = library.root.join("library.schema-27-favourites.sqlite3");
    fs::copy(&library.path, &kept).unwrap();
    fs::copy(&report.backup_path, &library.path).unwrap();

    assert_eq!(user_version(&library.path), 26);
    assert!(!has_collection_tables(&library.path));
    assert_eq!(all_rows(&library.path), before);
    open_or_create_with_migrations(&library.path, &MIGRATIONS[..SCHEMA_26]).unwrap();

    // Upgrading again starts with empty Favourites; the set-aside copy is the
    // only remaining record of them.
    upgrade_library_database(&library.path).unwrap();
    let again = Database::open_read_only(&library.path).unwrap();
    assert!(again.load_game_collections().unwrap()[0].items.is_empty());
    let saved = Database::open_read_only(&kept).unwrap();
    assert_eq!(saved.load_game_collections().unwrap()[0].items.len(), 1);
}
