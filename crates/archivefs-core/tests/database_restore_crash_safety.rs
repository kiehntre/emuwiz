//! Database restore under competing writers, a second restoring process and
//! a restore process killed mid-way. Everything runs on disposable databases
//! in a temporary directory; the production library is never touched.
//!
//! The cross-process cases re-run this test binary as a child with
//! `EMUWIZ_RESTORE_TEST_ROLE` set, so the child really is another OS process
//! with its own SQLite connections and locks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use archivefs_core::{
    Database, DatabaseRestoreState, InterruptedRestoreOutcome, assess_interrupted_restore,
    database_restore_fault_injection as fault, finalise_interrupted_restore,
    interrupted_restore_receipts, prepare_database_restore, read_database_restore_receipt,
    restore_database,
};

const CONFIRMATION: &str = "RESTORE DATABASE";
const ROLE: &str = "EMUWIZ_RESTORE_TEST_ROLE";
const DB: &str = "EMUWIZ_RESTORE_TEST_DB";
const LOG: &str = "EMUWIZ_RESTORE_TEST_LOG";
const STAGE: &str = "EMUWIZ_RESTORE_TEST_STAGE";

fn seed(path: &Path, marker: &str) {
    let database = Database::open_or_create(path).unwrap();
    database.close().unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS restore_marker(value TEXT NOT NULL);
             DELETE FROM restore_marker;
             INSERT INTO restore_marker(value) VALUES ('{marker}');
             CREATE TABLE IF NOT EXISTS writer_log(id INTEGER PRIMARY KEY, n INTEGER NOT NULL);"
        ))
        .unwrap();
}

fn marker(path: &Path) -> String {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("SELECT value FROM restore_marker", [], |r| r.get(0))
        .unwrap()
}

fn writer_ids(path: &Path) -> Vec<i64> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = connection
        .prepare("SELECT n FROM writer_log ORDER BY n")
        .unwrap();
    statement
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<i64>>>()
        .unwrap_or_default()
}

fn quick_check(path: &Path) -> String {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))
        .unwrap()
}

fn child(role: &str, env: &[(&str, &Path)]) -> std::process::Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg(format!("child_{role}"))
        .arg("--nocapture")
        .env(ROLE, role)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    for (key, value) in env {
        command.env(key, value);
    }
    command.spawn().unwrap()
}

/// The child role bodies live in ordinary `#[test]` functions that return
/// immediately unless the role variable selects them.
fn role() -> Option<String> {
    std::env::var(ROLE).ok()
}

/// A second process that commits one row per transaction as fast as it can
/// and records every commit SQLite acknowledged. It stops at the first error.
#[test]
fn child_writer() {
    if role().as_deref() != Some("writer") {
        return;
    }
    let db = PathBuf::from(std::env::var_os(DB).unwrap());
    let log = PathBuf::from(std::env::var_os(LOG).unwrap());
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    let mut acknowledged = Vec::new();
    let started = Instant::now();
    let mut n = 0_i64;
    // Bursts of commits with short idle gaps, like a scan persisting batches.
    'bursts: while started.elapsed() < Duration::from_secs(20) {
        for _ in 0..20 {
            n += 1;
            match connection.execute("INSERT INTO writer_log(n) VALUES (?1)", [n]) {
                Ok(_) => acknowledged.push(n.to_string()),
                Err(error) => {
                    acknowledged.push(format!("err {n} {error}"));
                    break 'bursts;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    fs::write(&log, acknowledged.join("\n")).unwrap();
}

/// A second process that runs a complete restore of the same plan.
#[test]
fn child_restorer() {
    if role().as_deref() != Some("restorer") {
        return;
    }
    let db = PathBuf::from(std::env::var_os(DB).unwrap());
    let log = PathBuf::from(std::env::var_os(LOG).unwrap());
    let backup = db.with_file_name("selected.backup");
    let plan = prepare_database_restore(&db, &backup).unwrap();
    let outcome = match restore_database(&plan, CONFIRMATION) {
        Ok(result) => format!("ok {}", result.receipt.state.label()),
        Err(error) => format!("err {error}"),
    };
    fs::write(&log, outcome).unwrap();
}

/// A restore process that blocks at a named stage until it is killed.
#[test]
fn child_restorer_stuck() {
    if role().as_deref() != Some("restorer_stuck") {
        return;
    }
    let db = PathBuf::from(std::env::var_os(DB).unwrap());
    let log = PathBuf::from(std::env::var_os(LOG).unwrap());
    let stage = std::env::var(STAGE).unwrap();
    let backup = db.with_file_name("selected.backup");
    let plan = prepare_database_restore(&db, &backup).unwrap();
    let signal = log.clone();
    let _guard = fault::install(move |s| {
        if s == stage {
            fs::write(&signal, s).unwrap();
            std::thread::sleep(Duration::from_secs(600));
        }
        Ok(())
    });
    let _ = restore_database(&plan, CONFIRMATION);
}

/// A process with a tiny descriptor budget that runs repeated restore and
/// rollback cycles: any leaked or retained descriptor would exhaust it.
#[test]
fn child_fd_limited() {
    if role().as_deref() != Some("fd_limited") {
        return;
    }
    let db = PathBuf::from(std::env::var_os(DB).unwrap());
    let log = PathBuf::from(std::env::var_os(LOG).unwrap());
    let backup = db.with_file_name("selected.backup");
    let limit = libc::rlimit {
        rlim_cur: 40,
        rlim_max: 40,
    };
    // SAFETY: plain setrlimit on this process with a fully initialised struct.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) };
    assert_eq!(
        rc,
        0,
        "setrlimit failed: {}",
        std::io::Error::last_os_error()
    );
    let mut outcome = String::new();
    for cycle in 0..15 {
        let step = (|| -> Result<(), String> {
            let plan = prepare_database_restore(&db, &backup).map_err(|e| e.to_string())?;
            let result = restore_database(&plan, CONFIRMATION).map_err(|e| e.to_string())?;
            archivefs_core::rollback_database_restore(&result.receipt, CONFIRMATION)
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(error) = step {
            outcome = format!("err cycle {cycle}: {error}");
            break;
        }
        let open = fs::read_dir("/proc/self/fd")
            .map(|d| d.count())
            .unwrap_or(usize::MAX);
        outcome = format!("ok cycles={} open_descriptors={open}", cycle + 1);
    }
    fs::write(&log, outcome).unwrap();
}

#[test]
fn restores_under_a_tiny_descriptor_limit_never_exhaust_it() {
    if role().is_some() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("library.sqlite3");
    let backup = root.path().join("selected.backup");
    seed(&live, "live");
    seed(&backup, "backup");
    let log = root.path().join("fd.log");
    let mut limited = child("fd_limited", &[(DB, &live), (LOG, &log)]);
    assert!(limited.wait().unwrap().success());
    let outcome = fs::read_to_string(&log).unwrap();
    assert!(outcome.starts_with("ok cycles=15"), "{outcome}");
    eprintln!("descriptor-limit measurement: {outcome}");
    assert_eq!(marker(&live), "live");
    assert_eq!(quick_check(&live), "ok");
    // Nothing of the superseded databases survives except the retained
    // emergency backups; no staging file is left.
    let names: Vec<String> = fs::read_dir(root.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains("staging")), "{names:?}");
    assert_eq!(
        names
            .iter()
            .filter(|n| n.contains("before-restore"))
            .count(),
        15
    );
}

fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn competing_writer_process_never_loses_an_acknowledged_commit() {
    if role().is_some() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("library.sqlite3");
    let backup = root.path().join("selected.backup");
    seed(&live, "live");
    seed(&backup, "backup");
    let log = root.path().join("writer.log");
    let mut writer = child("writer", &[(DB, &live), (LOG, &log)]);
    // Let the writer commit a few rows, then restore while it keeps going.
    std::thread::sleep(Duration::from_millis(300));
    // A review taken while the writer is mid-burst is stale by the time the
    // restore re-checks it, and a restore started mid-burst is refused as
    // busy; both are truthful refusals. Retry as a user would.
    let mut outcome = None;
    for _ in 0..200 {
        let plan = match prepare_database_restore(&live, &backup) {
            Ok(plan) => plan,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
        };
        match restore_database(&plan, CONFIRMATION) {
            Ok(result) => {
                outcome = Some(result);
                break;
            }
            Err(error) => {
                let text = error.to_string();
                assert!(
                    text.contains("Nothing was changed") || text.contains("changed since"),
                    "unexpected refusal: {text}"
                );
                assert_eq!(quick_check(&live), "ok");
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = outcome.expect("restore should eventually win a gap between writer transactions");
    assert_eq!(result.receipt.state, DatabaseRestoreState::Completed);
    assert!(writer.wait().unwrap().success());
    let acknowledged: Vec<i64> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter_map(|line| line.parse().ok())
        .collect();
    assert!(
        !acknowledged.is_empty(),
        "the writer must have committed rows"
    );
    // Every acknowledged commit is in the emergency backup: none was lost.
    let preserved = writer_ids(&result.emergency_backup_path);
    let lost: Vec<i64> = acknowledged
        .iter()
        .copied()
        .filter(|id| !preserved.contains(id))
        .collect();
    assert!(
        lost.is_empty(),
        "acknowledged commits missing from the emergency backup: {lost:?}"
    );
    // The writer's first failure was loud, and the restored database holds
    // only the backup's content.
    let errors: Vec<String> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("err"))
        .map(str::to_string)
        .collect();
    assert!(errors.len() <= 1, "{errors:?}");
    if let Some(error) = errors.first() {
        assert!(
            error.contains("locked") || error.contains("readonly") || error.contains("busy"),
            "{error}"
        );
    }
    assert_eq!(marker(&live), "backup");
    assert!(writer_ids(&live).is_empty());
    assert_eq!(quick_check(&live), "ok");
    assert_eq!(quick_check(&result.emergency_backup_path), "ok");
}

#[test]
fn second_process_restoring_concurrently_is_refused_or_serialised_never_corrupting() {
    if role().is_some() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("library.sqlite3");
    let backup = root.path().join("selected.backup");
    seed(&live, "live");
    seed(&backup, "backup");
    let plan = prepare_database_restore(&live, &backup).unwrap();
    // Hold this process's restore at the point just before the exchange and
    // let a second process try the same restore meanwhile.
    let log = root.path().join("restorer.log");
    let live_for_child = live.clone();
    let log_for_child = log.clone();
    let _guard = fault::install(move |stage| {
        if stage == fault::STAGE_BEFORE_EXCHANGE {
            let mut other = child("restorer", &[(DB, &live_for_child), (LOG, &log_for_child)]);
            assert!(other.wait().unwrap().success());
        }
        Ok(())
    });
    let result = restore_database(&plan, CONFIRMATION).unwrap();
    assert_eq!(result.receipt.state, DatabaseRestoreState::Completed);
    let other = fs::read_to_string(&log).unwrap();
    assert!(
        other.starts_with("err") && other.contains("already running"),
        "the second process must be refused while the first holds the exclusion: {other}"
    );
    assert_eq!(marker(&live), "backup");
    assert_eq!(quick_check(&live), "ok");
    // Exactly one restore receipt reached Completed; the refused one is Failed
    // with the original intact.
    let mut states: Vec<(DatabaseRestoreState, Option<bool>)> = fs::read_dir(root.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.to_string_lossy().contains(".restore-") && p.extension().is_some_and(|e| e == "json")
        })
        .map(|p| {
            let r = read_database_restore_receipt(&p).unwrap();
            (r.state, r.original_intact)
        })
        .collect();
    states.sort_by_key(|(s, _)| s.label());
    assert_eq!(
        states,
        vec![
            (DatabaseRestoreState::Completed, Some(false)),
            (DatabaseRestoreState::Failed, Some(true))
        ]
    );
}

#[test]
fn restore_process_killed_at_every_stage_is_recoverable_after_restart() {
    if role().is_some() {
        return;
    }
    for stage in fault::STAGES {
        let root = tempfile::tempdir().unwrap();
        let live = root.path().join("library.sqlite3");
        let backup = root.path().join("selected.backup");
        seed(&live, "live");
        seed(&backup, "backup");
        let original_bytes = fs::read(&live).unwrap();
        let signal = root.path().join("stage.signal");
        let stage_path = PathBuf::from(stage);
        let mut stuck = child(
            "restorer_stuck",
            &[(DB, &live), (LOG, &signal), (STAGE, &stage_path)],
        );
        assert!(
            wait_for_file(&signal, Duration::from_secs(60)),
            "{stage}: child never reached the stage"
        );
        // SIGKILL: no destructors, no receipt updates, locks released by the
        // kernel. This is the power-loss model for the application process.
        stuck.kill().unwrap();
        stuck.wait().unwrap();

        // "Restart": the next EmuWiz finds the receipt and classifies it.
        let receipts = interrupted_restore_receipts(&live);
        assert_eq!(receipts.len(), 1, "{stage}: {receipts:?}");
        let assessment = assess_interrupted_restore(&receipts[0]).unwrap();
        let exchanged = !matches!(
            *stage,
            fault::STAGE_AFTER_EXCLUSION
                | fault::STAGE_AFTER_EMERGENCY_BACKUP
                | fault::STAGE_AFTER_STAGING
                | fault::STAGE_BEFORE_EXCHANGE
        );
        if exchanged {
            assert_eq!(
                assessment.outcome,
                InterruptedRestoreOutcome::ReplacementVerified {
                    modified_since: false
                },
                "{stage}: {}",
                assessment.summary
            );
            assert_eq!(marker(&live), "backup", "{stage}");
            assert!(assessment.emergency_backup_verified, "{stage}");
        } else {
            assert_eq!(
                assessment.outcome,
                InterruptedRestoreOutcome::OriginalIntact {
                    modified_since: false
                },
                "{stage}: {}",
                assessment.summary
            );
            assert_eq!(
                fs::read(&live).unwrap(),
                original_bytes,
                "{stage}: original bytes must be untouched"
            );
        }
        assert_eq!(quick_check(&live), "ok", "{stage}");
        // No sidecar, no temporary receipt, and the database opens normally:
        // the dead process's locks are gone.
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut name = live.as_os_str().to_os_string();
            name.push(suffix);
            assert!(!Path::new(&name).exists(), "{stage}: {suffix} left behind");
        }
        let names: Vec<String> = fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.contains(".tmp-")),
            "{stage}: {names:?}"
        );
        Database::open_or_create(&live).unwrap().close().unwrap();
        // Finalising records the truth without touching any database file.
        let before: Vec<(String, Vec<u8>)> = names
            .iter()
            .filter(|n| !n.ends_with(".json"))
            .map(|n| (n.clone(), fs::read(root.path().join(n)).unwrap()))
            .collect();
        let finalised = finalise_interrupted_restore(&receipts[0], CONFIRMATION).unwrap();
        let expected_state = if exchanged {
            DatabaseRestoreState::Completed
        } else {
            DatabaseRestoreState::Failed
        };
        assert_eq!(finalised.receipt.state, expected_state, "{stage}");
        for (name, bytes) in before {
            assert_eq!(
                fs::read(root.path().join(&name)).unwrap(),
                bytes,
                "{stage}: finalise changed {name}"
            );
        }
        assert!(interrupted_restore_receipts(&live).is_empty(), "{stage}");
    }
}
