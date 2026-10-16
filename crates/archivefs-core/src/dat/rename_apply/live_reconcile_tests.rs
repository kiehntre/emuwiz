//! Recovery reconciliation must never rewrite a transaction an operation in
//! this process is still running (DAT Sources reconciles on every frame while
//! its own Quick Rename apply runs). Private temporary fixtures only.
#![cfg(target_os = "linux")]

use super::*;
use crate::dat::rename_apply::executor::set_test_before_rename_hook;
use crate::safe_read::TrustedRoots;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

struct Fixture {
    temp: tempfile::TempDir,
    journal_dir: PathBuf,
    transaction: RenameTransaction,
    approved: BTreeSet<String>,
}

fn fixture(count: usize) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let roms = temp.path().join("roms");
    std::fs::create_dir(&roms).unwrap();
    let journal_dir = temp.path().join("journal");
    std::fs::create_dir(&journal_dir).unwrap();
    let mut entries = Vec::new();
    let mut approved = BTreeSet::new();
    for n in 0..count {
        let source = roms.join(format!("game{n}.bin"));
        std::fs::write(&source, format!("synthetic game {n}")).unwrap();
        approved.insert(source.to_string_lossy().into_owned());
        let entry: TransactionEntry = serde_json::from_value(serde_json::json!({
            "source_path": source,
            "destination_path": roms.join(format!("Game {n} (Europe).bin")),
            "original_basename": format!("game{n}.bin"),
            "proposed_basename": format!("Game {n} (Europe).bin"),
            "identity": capture_identity(&source).unwrap()
        }))
        .unwrap();
        entries.push(entry);
    }
    let transaction = RenameTransaction {
        transaction_id: new_transaction_id(1),
        plan_generation: 1,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
        created_at_unix: 1,
        source_scan_root: roms.to_string_lossy().into_owned(),
        state: TransactionState::Planned,
        entries,
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    Fixture {
        temp,
        journal_dir,
        transaction,
        approved,
    }
}

fn apply(f: &mut Fixture) -> Result<ApplyOutcome, ApplyError> {
    let cancel = AtomicBool::new(false);
    apply_transaction(&mut ApplyExecution {
        transaction: &mut f.transaction,
        approved_paths: f.approved.clone(),
        current_generation: 1,
        trusted: TrustedRoots::from_paths([f.temp.path()]),
        journal_dir: f.journal_dir.clone(),
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &cancel,
        directory_policy: DirectoryPolicy::SameDirectory,
        allow_symlink_source: false,
    })
}

thread_local! {
    static PROBE_JOURNAL: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static PROBE_SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Runs exactly where a frame's reconcile would land: after the apply has
/// journalled the entry as in flight and before its rename.
fn reconcile_mid_apply(_source: &Path) {
    let path = PROBE_JOURNAL.with(|p| p.borrow().clone()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut on_disk = read_journal(&path).unwrap();
    let in_flight = on_disk
        .entries
        .iter()
        .any(|entry| entry.state == EntryState::Applying);
    let result = reconcile_recovery(&mut on_disk, path.parent().unwrap());
    let after = std::fs::read(&path).unwrap();
    PROBE_SEEN.with(|seen| {
        seen.borrow_mut().push(format!(
            "in_flight={in_flight} refused={} unchanged={}",
            result.is_err(),
            before == after
        ))
    });
}

#[test]
fn reconcile_during_the_apply_refuses_and_leaves_the_live_journal_alone() {
    let mut f = fixture(1);
    let path = journal_path(&f.journal_dir, &f.transaction.transaction_id).unwrap();
    PROBE_JOURNAL.with(|p| *p.borrow_mut() = Some(path.clone()));
    PROBE_SEEN.with(|seen| seen.borrow_mut().clear());
    set_test_before_rename_hook(Some(reconcile_mid_apply));
    let outcome = apply(&mut f).expect("the apply completes");
    set_test_before_rename_hook(None);
    assert_eq!(
        PROBE_SEEN.with(|seen| seen.borrow().clone()),
        vec!["in_flight=true refused=true unchanged=true".to_string()],
        "a reconcile racing the live apply must not rewrite its journal"
    );
    assert_eq!(outcome.transaction.state, TransactionState::Applied);
    let persisted = read_journal(&path).unwrap();
    assert_eq!(persisted.state, TransactionState::Applied);
    assert!(
        persisted
            .entries
            .iter()
            .all(|entry| entry.state == EntryState::Applied),
        "an entry the apply renamed is never journalled as skipped"
    );
    assert!(!is_transaction_live(&f.transaction.transaction_id));
}

#[test]
fn an_interrupted_transaction_is_reconciled_once_nothing_runs_it() {
    let mut f = fixture(1);
    let entry = &mut f.transaction.entries[0];
    entry.state = EntryState::Applying;
    let source = entry.source_path.clone();
    f.transaction.state = TransactionState::Applying;
    write_journal(&f.journal_dir, &f.transaction).unwrap();
    {
        let _live = mark_transaction_live(&f.transaction.transaction_id);
        let mut copy = f.transaction.clone();
        assert!(reconcile_recovery(&mut copy, &f.journal_dir).is_err());
        assert_eq!(copy, f.transaction, "a refused reconcile changes nothing");
    }
    // Ordinary legacy recovery still works once the operation is gone.
    let issues = reconcile_recovery(&mut f.transaction, &f.journal_dir).unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(f.transaction.entries[0].state, EntryState::Skipped);
    assert!(source.exists());
    let persisted =
        read_journal(&journal_path(&f.journal_dir, &f.transaction.transaction_id).unwrap())
            .unwrap();
    assert_eq!(
        persisted, f.transaction,
        "the legacy journal write still lands"
    );
}

#[test]
fn live_marks_nest_and_are_released_even_by_a_panic() {
    let id = new_transaction_id(2);
    let outer = mark_transaction_live(&id);
    let inner = mark_transaction_live(&id);
    drop(inner);
    assert!(is_transaction_live(&id), "the outer operation still runs");
    drop(outer);
    assert!(!is_transaction_live(&id));
    let moved = id.clone();
    let panicked = std::thread::spawn(move || {
        let _live = mark_transaction_live(&moved);
        panic!("synthetic operation failure");
    })
    .join();
    assert!(panicked.is_err());
    assert!(
        !is_transaction_live(&id),
        "a panicking operation does not pin its mark"
    );
}

/// The GUI shape: one thread applies, another reconciles everything in the
/// journal folder in a loop, as the DAT Sources page does on every frame.
#[test]
fn a_concurrent_recovery_poll_never_breaks_or_rewrites_a_running_apply() {
    for _round in 0..5 {
        let mut f = fixture(24);
        let dir = f.journal_dir.clone();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let poller = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let (mut found, _) = find_rollbackable_transactions(&dir);
                    for transaction in &mut found {
                        let _ = reconcile_recovery(transaction, &dir);
                    }
                }
            })
        };
        let outcome = apply(&mut f);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        poller.join().unwrap();
        let outcome = outcome.expect("the apply's own journal writes are never refused");
        assert_eq!(outcome.transaction.state, TransactionState::Applied);
        let persisted =
            read_journal(&journal_path(&f.journal_dir, &f.transaction.transaction_id).unwrap())
                .unwrap();
        assert_eq!(persisted.state, TransactionState::Applied);
        assert!(
            persisted
                .entries
                .iter()
                .all(|e| e.state == EntryState::Applied)
        );
    }
}
