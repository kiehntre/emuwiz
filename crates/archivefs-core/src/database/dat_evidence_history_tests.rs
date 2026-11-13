//! Schema 27 (kept DAT evidence and re-check notes): fresh installs, the
//! upgrade from schema 26, failure and interruption, and the refusal of a
//! database that another build line took to "27".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{
    Database, MIGRATIONS, Migration, apply_migrations, open_connection,
    upgrade_library_database_with_migrations,
};

const LEGACY_FACTS: &str = r#"{"verification_state":{"state":"verified_single_match","algorithm":"sha1"},"source":{"source_id":"158","source_name":"Fixture","ecosystem":"no_intro","variant":"unknown","source_revision":"1","author":null,"catalogue_names":["Fixture"],"dat_path":"/nowhere/158.dat"},"canonical":{"canonical_dat_name":"Game","canonical_rom_name":"Game.gba","region":null,"revision":null},"hash_evidence":{"matched_algorithm":"sha1","matched_value":"aa","available_algorithms":["SHA-1"]},"ambiguous_candidates":[],"candidate_provenance":[],"matched_entries":[],"audited_hashes":{"size_bytes":4,"crc32":null,"md5":null,"sha1":"aa","sha256":null},"audited_at":"2026-10-08T00:50:48Z","completeness":"exhaustive"}"#;

/// A schema-26 library with saved (pre-proof) evidence and other user data.
fn make_schema_26(path: &Path, rows: usize) {
    let mut connection = open_connection(path).unwrap();
    apply_migrations(&mut connection, &MIGRATIONS[..26]).unwrap();
    connection
        .execute_batch("INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,x'2f726f6d73','now','now');")
        .unwrap();
    let tx = connection.transaction().unwrap();
    for id in 1..=rows as i64 {
        let name = format!("g{id}.gba");
        tx.execute("INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) VALUES(?1,1,?2,?3,?2,'direct_game_image',?4,?4,'now','now','now','now')", rusqlite::params![id, name.as_bytes(), format!("/roms/{name}").as_bytes(), name]).unwrap();
        tx.execute("INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) VALUES(?1,'Game Boy Advance','manual',1,'now')", [id]).unwrap();
        tx.execute("INSERT INTO library_dat_identities(archive_id,dat_source_id,dat_ecosystem,source_revision,verification_state,completeness,audited_at,facts_json,created_at,updated_at) VALUES(?1,'158','no_intro','1','verified_single_match','exhaustive','2026-10-08T00:50:48Z',?2,'then','then')", rusqlite::params![id, LEGACY_FACTS.as_bytes()]).unwrap();
    }
    tx.commit().unwrap();
    connection.close().unwrap();
}

fn tables(connection: &Connection) -> Vec<String> {
    connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Every row of every table, as text, keyed by table.
fn contents(path: &Path) -> BTreeMap<String, Vec<String>> {
    let connection =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    tables(&connection)
        .into_iter()
        .map(|table| {
            let columns: Vec<String> = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let select = columns
                .iter()
                .map(|c| format!("quote({c})"))
                .collect::<Vec<_>>()
                .join("||'|'||");
            let rows = connection
                .prepare(&format!("SELECT {select} FROM {table} ORDER BY 1"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            (table, rows)
        })
        .collect()
}

fn version(path: &Path) -> i64 {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

fn integrity(path: &Path) -> String {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap()
}

const NEW_TABLES: [&str; 2] = ["dat_recheck_attempts", "library_dat_identity_history"];

#[test]
fn a_fresh_library_has_the_new_tables_and_they_are_empty() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.sqlite3");
    let db = Database::open_or_create(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), 27);
    assert_eq!(db.replaced_dat_evidence_count().unwrap(), 0);
    assert!(db.dat_recheck_attempts().unwrap().is_empty());
    drop(db);
    let all = contents(&path);
    for table in NEW_TABLES {
        assert!(all[table].is_empty());
    }
}

#[test]
fn upgrading_26_to_27_changes_no_existing_row_and_verifies_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.sqlite3");
    make_schema_26(&path, 25);
    let before = contents(&path);
    let report = super::upgrade_library_database(&path).unwrap();
    assert_eq!((report.from_version, report.to_version), (26, 27));
    assert_eq!(report.applied_versions, vec![27]);
    assert_eq!(integrity(&path), "ok");
    let after = contents(&path);
    for (table, rows) in &before {
        if table == "schema_migrations" {
            assert_eq!(after[table].len(), rows.len() + 1);
            continue;
        }
        assert_eq!(&after[table], rows, "{table} changed");
    }
    let added: Vec<_> = after.keys().filter(|t| !before.contains_key(*t)).collect();
    assert_eq!(added, NEW_TABLES);
    for table in NEW_TABLES {
        assert!(after[table].is_empty(), "{table} must start empty");
    }
    // The verified backup is the untouched schema-26 library.
    assert_eq!(version(&report.backup_path), 26);
    assert_eq!(contents(&report.backup_path), before);
    // The saved legacy rows are still not current evidence after the upgrade.
    let db = Database::open_or_create(&path).unwrap();
    let status = db.saved_dat_evidence_status(&Default::default()).unwrap();
    assert_eq!(status.len(), 25);
    assert!(
        status
            .iter()
            .all(|row| row.status != crate::database::SavedDatEvidenceStatus::Current)
    );
    assert!(
        db.current_library_dat_evidence(&Default::default())
            .unwrap()
            .states
            .is_empty()
    );
}

#[test]
fn a_failing_step_27_leaves_the_schema_26_library_exactly_as_it_was() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.sqlite3");
    make_schema_26(&path, 5);
    // The second statement of the migration will fail; the first must not stay.
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE dat_recheck_attempts (squatter INTEGER);")
        .unwrap();
    let before = contents(&path);
    assert!(super::upgrade_library_database(&path).is_err());
    assert_eq!(version(&path), 26);
    assert_eq!(contents(&path), before);
    assert_eq!(integrity(&path), "ok");
}

fn foreign_27(path: &Path, record: bool) {
    make_schema_26(path, 3);
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch("CREATE TABLE game_collections (id INTEGER PRIMARY KEY);")
        .unwrap();
    if record {
        connection
            .execute_batch("INSERT INTO schema_migrations(version,description,applied_at) VALUES(27,'favourites and user collections','now');")
            .unwrap();
    }
    connection.pragma_update(None, "user_version", 27).unwrap();
}

#[test]
fn a_library_taken_to_27_by_another_build_line_is_refused_and_untouched() {
    for record in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.sqlite3");
        foreign_27(&path, record);
        let bytes = std::fs::read(&path).unwrap();
        let expected = if record {
            "different EmuWiz build"
        } else {
            "no record of schema 27"
        };
        for error in [
            Database::open_or_create(&path).err().unwrap().to_string(),
            Database::open_read_only(&path).err().unwrap().to_string(),
        ] {
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("Nothing was changed"), "{error}");
        }
        // Nor can a later build migrate past the foreign step.
        let later = [
            Migration {
                version: 27,
                description: MIGRATIONS[26].description,
                sql: MIGRATIONS[26].sql,
            },
            Migration {
                version: 28,
                description: "synthetic step 28",
                sql: "CREATE TABLE step_28 (id INTEGER PRIMARY KEY);",
            },
        ];
        let error = upgrade_library_database_with_migrations(&path, &later)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert_eq!(version(&path), 27);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

// ---- Process kill during the upgrade --------------------------------------

const KILL_PATH: &str = "EMUWIZ_TEST_UPGRADE27_KILL_PATH";

#[test]
#[ignore = "child process of the upgrade kill test; does nothing on its own"]
fn kill_child_runs_the_upgrade() {
    let Some(path) = std::env::var_os(KILL_PATH).map(PathBuf::from) else {
        return;
    };
    std::fs::write(path.with_extension("started"), b"").unwrap();
    super::upgrade_library_database(&path).unwrap();
}

#[test]
fn killing_the_upgrade_at_any_moment_leaves_schema_26_or_a_whole_schema_27() {
    let (mut at_26, mut at_27, mut journals) = (0, 0, 0);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    // How long an uninterrupted upgrade takes here.
    let full_upgrade = {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.sqlite3");
        make_schema_26(&path, 400);
        let started = std::time::Instant::now();
        super::upgrade_library_database(&path).unwrap();
        started.elapsed()
    };
    for round in 0..24u64 {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.sqlite3");
        make_schema_26(&path, 400);
        let before = contents(&path);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "database::dat_evidence_history_tests::kill_child_runs_the_upgrade",
                "--ignored",
                "--test-threads=1",
            ])
            .env(KILL_PATH, &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = path.with_extension("started");
        while !started.exists() && child.try_wait().unwrap().is_none() {
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        if round % 4 == 3 {
            // Kill the moment the migration is seen committed, before the
            // upgrade has finished whatever follows it.
            while child.try_wait().unwrap().is_none() {
                let seen =
                    Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                        .and_then(|c| {
                            c.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                        });
                if seen == Ok(27) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
        } else {
            // Spread the kills across the upgrade (verified backup, then
            // the migration).
            std::thread::sleep(std::time::Duration::from_micros(
                seed % (full_upgrade.as_micros() as u64 * (round + 2) / 20).max(1),
            ));
        }
        child.kill().unwrap();
        child.wait().unwrap();

        // A killed writer may leave SQLite's rollback journal, which EmuWiz
        // refuses to open over. The copy-first recovery must then give back
        // a whole database; anything else it reports as nothing to recover.
        if path.with_file_name("library.sqlite3-journal").exists() {
            match super::prepare_journal_recovery(&path) {
                Ok(receipt) => {
                    journals += 1;
                    super::apply_journal_recovery(&path, &receipt.recovery_dir).unwrap();
                }
                Err(error) => assert!(
                    error.to_string().contains("no interrupted write"),
                    "{error}"
                ),
            }
        }
        assert_eq!(integrity(&path), "ok");
        let after = contents(&path);
        match version(&path) {
            26 => {
                at_26 += 1;
                assert_eq!(
                    after, before,
                    "a killed upgrade changed a schema-26 library"
                );
                // The interrupted upgrade can simply be run again.
                let report = super::upgrade_library_database(&path).unwrap();
                assert_eq!((report.from_version, report.to_version), (26, 27));
            }
            27 => {
                at_27 += 1;
                Database::open_or_create(&path).expect("a whole schema 27 opens");
            }
            other => panic!("schema {other} after a killed upgrade"),
        }
        let done = contents(&path);
        for (table, rows) in &before {
            if table != "schema_migrations" {
                assert_eq!(&done[table], rows, "{table} changed");
            }
        }
        for table in NEW_TABLES {
            assert!(done[table].is_empty());
        }
        assert_eq!(version(&path), 27);
    }
    eprintln!(
        "upgrade kill test: {at_26} left schema 26, {at_27} reached 27, {journals} rollback journals"
    );
    assert!(at_26 > 0, "no kill landed before the migration committed");
    assert!(at_27 > 0, "no kill landed after the migration committed");
}
