//! Copy-first recovery from an interrupted write: what prepare may touch,
//! what apply requires, and what a kill during either leaves behind.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::Database;
use super::journal_recovery::{
    JournalRecoveryState, apply_journal_recovery, prepare_journal_recovery,
    read_journal_recovery_receipt,
};

const ROWS: usize = 400;

fn journal_of(database: &Path) -> PathBuf {
    let mut name = database.as_os_str().to_os_string();
    name.push("-journal");
    PathBuf::from(name)
}

/// Creates a current-schema library at `live` and leaves a write open on
/// it that has already reached the database file: the journal holds the
/// pages it replaced and has a live header. Dropping the connection rolls
/// the write back.
fn cut_off_write(live: &Path) -> Connection {
    drop(Database::open_or_create(live).unwrap());
    let mut connection = Connection::open(live).unwrap();
    connection
        .execute(
            "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,x'2f726f6d73','now','now')",
            [],
        )
        .unwrap();
    let tx = connection.transaction().unwrap();
    for id in 1..=ROWS as i64 {
        let name = format!("game-{id:04}-{}", "x".repeat(600));
        tx.execute("INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) VALUES(?1,1,?2,?2,?2,'direct_game_image',?3,'committed','now','now','now','now')", rusqlite::params![id, format!("g{id}").as_bytes(), name]).unwrap();
    }
    tx.commit().unwrap();
    // A small page cache makes the transaction write to the database file
    // before it commits, which is when SQLite makes the journal live.
    connection
        .execute_batch(
            "PRAGMA cache_size=5; BEGIN;
             UPDATE archives SET normalized_name = 'never committed', display_name = hex(randomblob(300));",
        )
        .unwrap();
    let journal = journal_of(live);
    let mut head = [0_u8; 8];
    use std::io::Read as _;
    fs::File::open(&journal)
        .unwrap()
        .read_exact(&mut head)
        .unwrap();
    assert_ne!(head, [0_u8; 8], "the fixture must produce a live journal");
    connection
}

/// `<dir>/library.sqlite3` as a killed writer leaves it. With `reached`,
/// the database file holds part of the uncommitted write. Without it, the
/// journal had been made live but the write had not reached the database
/// yet: the journal then restores nothing.
fn interrupted_library(dir: &Path, reached: bool) -> PathBuf {
    let staging = dir.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let live = staging.join("library.sqlite3");
    let connection = cut_off_write(&live);
    let target = dir.join("library.sqlite3");
    fs::copy(journal_of(&live), journal_of(&target)).unwrap();
    if reached {
        fs::copy(&live, &target).unwrap();
        drop(connection);
    } else {
        drop(connection);
        assert!(!journal_of(&live).exists());
        fs::copy(&live, &target).unwrap();
    }
    fs::remove_dir_all(&staging).unwrap();
    target
}

fn library_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().unwrap().is_file())
        .filter(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .ends_with(".upgrade.lock")
        })
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

fn committed_rows(database: &Path) -> i64 {
    Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM archives WHERE normalized_name = 'committed'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn prepare_changes_nothing_in_the_library_and_the_guard_still_refuses() {
    let temp = tempfile::tempdir().unwrap();
    let database = interrupted_library(temp.path(), true);
    let before = library_files(temp.path());
    let refusal = Database::open_or_create(&database)
        .err()
        .unwrap()
        .to_string();
    assert!(refusal.contains("database-recover-journal"), "{refusal}");

    let receipt = prepare_journal_recovery(&database).unwrap();
    assert_eq!(receipt.state, JournalRecoveryState::Prepared);
    assert!(receipt.journal_restored_content);
    assert!(receipt.opens_as_current_library);
    assert_eq!(receipt.schema_version, 27);
    // The live database and journal are byte for byte what they were, and
    // EmuWiz still will not open them.
    assert_eq!(library_files(temp.path()), before);
    assert!(Database::open_or_create(&database).is_err());
    assert!(Database::open_read_only(&database).is_err());
    // The preserved set is the live pair; the recovered copy is whole.
    let original = receipt.recovery_dir.join("original");
    assert_eq!(
        fs::read(original.join("library.sqlite3")).unwrap(),
        fs::read(&database).unwrap()
    );
    assert_eq!(
        fs::read(original.join("library.sqlite3-journal")).unwrap(),
        fs::read(journal_of(&database)).unwrap()
    );
    let recovered = receipt.recovery_dir.join("recovered/library.sqlite3");
    assert_eq!(committed_rows(&recovered), ROWS as i64);
    assert_eq!(
        read_journal_recovery_receipt(&receipt.recovery_dir).unwrap(),
        receipt
    );
}

#[test]
fn apply_gives_back_the_last_committed_library_and_matches_the_checked_copy() {
    let temp = tempfile::tempdir().unwrap();
    let database = interrupted_library(temp.path(), true);
    let prepared = prepare_journal_recovery(&database).unwrap();
    let applied = apply_journal_recovery(&database, &prepared.recovery_dir).unwrap();
    assert_eq!(applied.state, JournalRecoveryState::Applied);
    assert!(!journal_of(&database).exists());
    assert_eq!(
        fs::read(&database).unwrap(),
        fs::read(prepared.recovery_dir.join("recovered/library.sqlite3")).unwrap()
    );
    let db = Database::open_or_create(&database).unwrap();
    assert_eq!(db.load_archives().unwrap().len(), ROWS);
    drop(db);
    assert_eq!(committed_rows(&database), ROWS as i64);
    // The interrupted originals are still there, and nothing can be re-applied.
    let original = prepared.recovery_dir.join("original");
    assert_eq!(
        super::journal_recovery::read_journal_recovery_receipt(&prepared.recovery_dir)
            .unwrap()
            .database
            .bytes,
        fs::metadata(original.join("library.sqlite3"))
            .unwrap()
            .len()
    );
    let again = apply_journal_recovery(&database, &prepared.recovery_dir)
        .err()
        .unwrap()
        .to_string();
    assert!(
        again.contains("no interrupted write") || again.contains("already"),
        "{again}"
    );
}

#[test]
fn a_leftover_journal_that_restores_nothing_is_reported_as_such() {
    let temp = tempfile::tempdir().unwrap();
    let database = interrupted_library(temp.path(), false);
    let receipt = prepare_journal_recovery(&database).unwrap();
    assert!(!receipt.journal_restored_content, "{receipt:?}");
    assert_eq!(receipt.recovered.sha256, receipt.database.sha256);
    apply_journal_recovery(&database, &receipt.recovery_dir).unwrap();
    assert_eq!(committed_rows(&database), ROWS as i64);
}

#[test]
fn apply_refuses_when_the_library_is_not_the_checked_set() {
    for tamper in ["database", "journal", "preserved", "other-folder"] {
        let temp = tempfile::tempdir().unwrap();
        let database = interrupted_library(temp.path(), true);
        let receipt = prepare_journal_recovery(&database).unwrap();
        let mut recovery_dir = receipt.recovery_dir.clone();
        let append = |path: &Path| {
            let mut bytes = fs::read(path).unwrap();
            bytes.extend_from_slice(b"x");
            fs::write(path, bytes).unwrap();
        };
        match tamper {
            "database" => append(&database),
            "journal" => append(&journal_of(&database)),
            "preserved" => append(&recovery_dir.join("original/library.sqlite3")),
            _ => {
                // A folder with no finished receipt is never a recovery.
                recovery_dir = temp.path().join("not-a-recovery");
                fs::create_dir(&recovery_dir).unwrap();
            }
        }
        let before = (
            fs::read(&database).unwrap(),
            fs::read(journal_of(&database)).unwrap(),
        );
        let error = apply_journal_recovery(&database, &recovery_dir)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("Nothing was changed") || error.contains("no finished recovery receipt"),
            "{tamper}: {error}"
        );
        assert_eq!(
            before,
            (
                fs::read(&database).unwrap(),
                fs::read(journal_of(&database)).unwrap()
            ),
            "{tamper}"
        );
        assert!(Database::open_or_create(&database).is_err(), "{tamper}");
    }
}

#[test]
fn prepare_refuses_what_it_cannot_establish() {
    // Nothing interrupted.
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("library.sqlite3");
    drop(Database::open_or_create(&database).unwrap());
    let error = prepare_journal_recovery(&database)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("no interrupted write"), "{error}");

    // A writer that is still running: its journal is live, and so is it.
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("library.sqlite3");
    let writer = cut_off_write(&database);
    let error = prepare_journal_recovery(&database)
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("writing the library database right now"),
        "{error}"
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
    drop(writer);

    // A write-ahead log beside an interrupted library.
    let temp = tempfile::tempdir().unwrap();
    let database = interrupted_library(temp.path(), true);
    fs::write(temp.path().join("library.sqlite3-wal"), b"wal").unwrap();
    let before = library_files(temp.path());
    let error = prepare_journal_recovery(&database)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("write-ahead log"), "{error}");
    assert_eq!(library_files(temp.path()), before);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), before.len() + 1);
}

// ---- Process kill during prepare and during apply -------------------------

const KILL_ENV: &str = "EMUWIZ_TEST_JOURNAL_RECOVERY_KILL";

#[test]
#[ignore = "child process of the recovery kill test; does nothing on its own"]
fn kill_child_runs_a_recovery_step() {
    let Some(spec) = std::env::var_os(KILL_ENV) else {
        return;
    };
    let spec = spec.to_string_lossy().into_owned();
    let (step, rest) = spec.split_once('|').unwrap();
    let (database, recovery_dir) = rest.split_once('|').unwrap();
    fs::write(Path::new(database).with_extension("started"), b"").unwrap();
    match step {
        "prepare" => drop(prepare_journal_recovery(Path::new(database)).unwrap()),
        _ => drop(apply_journal_recovery(Path::new(database), Path::new(recovery_dir)).unwrap()),
    }
}

fn run_killed(step: &str, database: &Path, recovery_dir: &Path, after: std::time::Duration) {
    let started = database.with_extension("started");
    let _ = fs::remove_file(&started);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "database::journal_recovery_tests::kill_child_runs_a_recovery_step",
            "--ignored",
            "--test-threads=1",
        ])
        .env(
            KILL_ENV,
            format!("{step}|{}|{}", database.display(), recovery_dir.display()),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    while !started.exists() && child.try_wait().unwrap().is_none() {
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    std::thread::sleep(after);
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn killing_prepare_or_apply_never_harms_the_library_and_recovery_still_finishes() {
    let mut seed = 0x1234_5678_9abc_def1u64;
    let mut next = |limit_micros: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        std::time::Duration::from_micros(seed % limit_micros.max(1))
    };
    // How long the two steps take here, uninterrupted.
    let (prepare_time, apply_time) = {
        let temp = tempfile::tempdir().unwrap();
        let database = interrupted_library(temp.path(), true);
        let started = std::time::Instant::now();
        let receipt = prepare_journal_recovery(&database).unwrap();
        let prepare_time = started.elapsed();
        let started = std::time::Instant::now();
        apply_journal_recovery(&database, &receipt.recovery_dir).unwrap();
        (prepare_time, started.elapsed())
    };
    let (mut unfinished_prepares, mut interrupted_applies, mut finished_by_child) = (0, 0, 0);
    for _ in 0..16 {
        let temp = tempfile::tempdir().unwrap();
        let database = interrupted_library(temp.path(), true);
        let pair = |database: &Path| {
            (
                fs::read(database).unwrap(),
                fs::read(journal_of(database)).unwrap(),
            )
        };
        let interrupted = pair(&database);

        // A killed prepare: the library pair is untouched, and a folder
        // without a receipt can never be applied.
        run_killed(
            "prepare",
            &database,
            Path::new("-"),
            next(prepare_time.as_micros() as u64 * 3 / 2),
        );
        assert_eq!(
            pair(&database),
            interrupted,
            "a killed prepare changed the library"
        );
        for entry in fs::read_dir(temp.path()).unwrap().map(Result::unwrap) {
            if entry.file_type().unwrap().is_dir()
                && read_journal_recovery_receipt(&entry.path()).is_err()
            {
                unfinished_prepares += 1;
                assert!(apply_journal_recovery(&database, &entry.path()).is_err());
                assert_eq!(pair(&database), interrupted);
            }
        }
        let receipt = prepare_journal_recovery(&database).unwrap();
        assert_eq!(pair(&database), interrupted);

        // A killed apply: the library is either still interrupted (and can
        // be applied again) or is exactly the checked copy. Never a third
        // state, and the guard keeps refusing until it is whole.
        run_killed(
            "apply",
            &database,
            &receipt.recovery_dir,
            next(apply_time.as_micros() as u64 * 3 / 2),
        );
        let checked = fs::read(receipt.recovery_dir.join("recovered/library.sqlite3")).unwrap();
        if journal_of(&database).exists() {
            interrupted_applies += 1;
            assert!(Database::open_or_create(&database).is_err());
            // The journal a rollback replays is unchanged until it is done.
            assert_eq!(fs::read(journal_of(&database)).unwrap(), interrupted.1);
            let applied = apply_journal_recovery(&database, &receipt.recovery_dir).unwrap();
            assert_eq!(applied.state, JournalRecoveryState::Applied);
        } else {
            finished_by_child += 1;
        }
        assert_eq!(fs::read(&database).unwrap(), checked);
        assert!(!journal_of(&database).exists());
        assert_eq!(committed_rows(&database), ROWS as i64);
        Database::open_or_create(&database).unwrap();
        // The interrupted originals outlive everything.
        let original = receipt.recovery_dir.join("original");
        assert_eq!(
            (
                fs::read(original.join("library.sqlite3")).unwrap(),
                fs::read(original.join("library.sqlite3-journal")).unwrap()
            ),
            interrupted
        );
    }
    eprintln!(
        "recovery kill test: {unfinished_prepares} unfinished prepares, {interrupted_applies} applies interrupted, {finished_by_child} applies finished before the kill"
    );
}

// ---- Independent review tests (engine/recovery review of 8abe9b90), unmodified ----

/// J1. `apply_journal_recovery` skips the "live database is the preserved
/// one" check whenever the receipt says `applying` (journal_recovery.rs
/// 643-653). An apply can be left in that state by a kill, or by SQLite
/// returning "busy" after the receipt was written. If the database file is
/// then replaced by anything else while the journal is left behind (a backup
/// copied over it, for example), a second apply must still refuse, because
/// the journal no longer belongs to that database.
#[test]
fn review_an_interrupted_apply_still_refuses_a_database_it_never_checked() {
    let temp = tempfile::tempdir().unwrap();
    let database = interrupted_library(temp.path(), true);
    let receipt = prepare_journal_recovery(&database).unwrap();

    // The state an apply leaves when it stops right after its first receipt
    // write and before SQLite touched anything.
    let receipt_path = receipt.recovery_dir.join("receipt.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    value["state"] = serde_json::Value::String("applying".into());
    fs::write(&receipt_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert_eq!(
        read_journal_recovery_receipt(&receipt.recovery_dir)
            .unwrap()
            .state,
        JournalRecoveryState::Applying
    );

    // Another, healthy library is put where the interrupted one was. The old
    // journal is still beside it.
    let other_dir = temp.path().join("other");
    fs::create_dir(&other_dir).unwrap();
    let other = other_dir.join("library.sqlite3");
    drop(Database::open_or_create(&other).unwrap());
    fs::copy(&other, &database).unwrap();
    let foreign = fs::read(&database).unwrap();
    assert_ne!(
        foreign,
        fs::read(receipt.recovery_dir.join("original/library.sqlite3")).unwrap()
    );
    assert_ne!(
        foreign,
        fs::read(receipt.recovery_dir.join("recovered/library.sqlite3")).unwrap()
    );

    let result = apply_journal_recovery(&database, &receipt.recovery_dir);
    let outcome = result
        .as_ref()
        .map(|r| format!("{:?}", r.state))
        .map_err(|e| e.to_string());
    assert_eq!(
        fs::read(&database).unwrap(),
        foreign,
        "apply let SQLite roll a journal back onto a database it never checked ({outcome:?})"
    );
    assert!(result.is_err(), "{outcome:?}");
}

/// J2. A kill between SQLite deleting the journal and the final receipt
/// write leaves: journal gone, database equal to the checked copy, receipt
/// `applying`. Running apply again (which the documentation tells the person
/// to do) should recognise the finished rollback and close the receipt, and
/// must not change the database.
#[test]
fn review_an_apply_killed_after_sqlite_finished_can_be_finalised() {
    let temp = tempfile::tempdir().unwrap();
    let database = interrupted_library(temp.path(), true);
    let receipt = prepare_journal_recovery(&database).unwrap();
    apply_journal_recovery(&database, &receipt.recovery_dir).unwrap();
    let recovered = fs::read(&database).unwrap();
    assert_eq!(
        recovered,
        fs::read(receipt.recovery_dir.join("recovered/library.sqlite3")).unwrap()
    );
    assert!(!journal_of(&database).exists());

    // Put the receipt back to what it was just before the last write.
    let receipt_path = receipt.recovery_dir.join("receipt.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    value["state"] = serde_json::Value::String("applying".into());
    value["applied_at"] = serde_json::Value::Null;
    fs::write(&receipt_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let again = apply_journal_recovery(&database, &receipt.recovery_dir);
    let outcome = again
        .as_ref()
        .map(|r| format!("{:?}", r.state))
        .map_err(|e| e.to_string());
    assert_eq!(
        fs::read(&database).unwrap(),
        recovered,
        "a repeated apply changed a recovered library"
    );
    assert_eq!(committed_rows(&database), ROWS as i64);
    let state = read_journal_recovery_receipt(&receipt.recovery_dir)
        .unwrap()
        .state;
    assert!(
        again.is_ok() && state == JournalRecoveryState::Applied,
        "the receipt is left saying the library may be part-way through a rollback: \
         apply returned {outcome:?}, receipt state {state:?}"
    );
}
