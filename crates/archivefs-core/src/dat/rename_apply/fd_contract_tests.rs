use super::*;
use crate::dat::rename_apply::{journal_path, read_journal};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;

fn fixture(label: &str) -> Fixture {
    let f = Fixture::new(label);
    fs::rename(f.source(), f.root.join("old.bin")).unwrap();
    fs::create_dir(f.root.join("journals")).unwrap();
    f
}
fn save(f: &Fixture, tx: &mut RenameTransaction) -> io::Result<()> {
    write_journal(&f.root.join("journals"), tx).map_err(io::Error::other)
}
fn run(f: &Fixture, action: &str, edge: Edge, fault: Fault) -> RenameTransaction {
    let mut tx = enrolled_transaction(f);
    contract::undo_entry(
        &f.root,
        &mut tx,
        0,
        &mut |tx| save(f, tx),
        &mut |at| {
            if edge == at {
                external_at(&f.root, action);
            }
            Ok(())
        },
        fault,
    )
    .unwrap();
    assert_eq!(
        read_journal(&journal_path(&f.root.join("journals"), "contract1i").unwrap()).unwrap(),
        tx
    );
    tx
}
fn control(action: &str) -> Fixture {
    let f = Fixture::new("paired-control-");
    fs::rename(f.source(), f.root.join("old.bin")).unwrap();
    assert_eq!(ordinary(&f).state, TransactionState::RolledBack);
    external_at(&f.root, action);
    f
}

#[test]
fn deletion_salvages_new_inode_while_ordinary_baseline_loses_a() {
    let control = control("delete-dest");
    assert!(!control.target().exists());
    let f = fixture("salvage-");
    let tx = run(&f, "delete-dest", Edge::BeforeUnlink, Fault::None);
    let v = contract::evidence(&tx, 0).unwrap().unwrap();
    assert_eq!(v.phase, Phase::Salvaged);
    let (_, copy) = contract::paths(&f.root, &tx, 0).unwrap();
    let copied = capture_identity(&copy).unwrap();
    assert_ne!(copied.ino, f.a.ino);
    assert_eq!(fs::read(copy).unwrap(), b"AAAA");
    assert_eq!(v.salvage_identity, Some(copied));
    assert_eq!(tx.state, TransactionState::RollingBack);
}
#[test]
fn replacement_preserves_b_and_salvages_a_without_overwrite() {
    let control = control("replace-dest");
    assert!(identity_matches(
        &control.b,
        &capture_identity(&control.target()).unwrap()
    ));
    let f = fixture("replace-");
    let tx = run(&f, "replace-dest", Edge::BeforeUnlink, Fault::None);
    assert_eq!(
        contract::evidence(&tx, 0).unwrap().unwrap().phase,
        Phase::Salvaged
    );
    assert!(identity_matches(
        &f.b,
        &capture_identity(&f.target()).unwrap()
    ));
    assert_eq!(
        fs::read(contract::paths(&f.root, &tx, 0).unwrap().1).unwrap(),
        b"AAAA"
    );
}
#[test]
fn external_rename_retains_original_identity_and_does_not_claim_deletion() {
    let control = control("rename-dest");
    assert!(identity_matches(
        &control.a,
        &capture_identity(&control.root.join("t.bin")).unwrap()
    ));
    let f = fixture("relocated-");
    let tx = run(&f, "rename-dest", Edge::BeforeUnlink, Fault::None);
    assert!(identity_matches(
        &f.a,
        &capture_identity(&f.root.join("t.bin")).unwrap()
    ));
    assert_eq!(
        contract::evidence(&tx, 0).unwrap().unwrap().phase,
        Phase::Incomplete
    );
    assert!(!contract::paths(&f.root, &tx, 0).unwrap().1.exists());
}
#[test]
fn copy_and_sync_failures_retain_partial_evidence_and_never_complete() {
    for fault in [
        Fault::CopyAfter(2, libc::ENOSPC),
        Fault::CopyAfter(1, libc::EIO),
        Fault::Sync,
    ] {
        let control = control("delete-dest");
        assert!(!control.target().exists());
        let f = fixture("copy-fault-");
        let tx = run(&f, "delete-dest", Edge::BeforeUnlink, fault);
        let v = contract::evidence(&tx, 0).unwrap().unwrap();
        assert_eq!(v.phase, Phase::PossiblyUnrecoverable);
        assert!(v.detail.contains("salvage failed"));
        assert!(v.salvage_identity.is_none());
        assert!(contract::paths(&f.root, &tx, 0).unwrap().1.exists());
        assert_eq!(tx.state, TransactionState::RollingBack);
    }
}
#[test]
fn changed_bytes_are_not_completed_undo() {
    let control = control("write-dest");
    assert_eq!(fs::read(control.target()).unwrap(), b"XXXX");
    let f = fixture("changed-");
    let tx = run(&f, "write-dest", Edge::BeforeUnlink, Fault::None);
    assert_eq!(fs::read(f.target()).unwrap(), b"XXXX");
    assert_eq!(
        contract::evidence(&tx, 0).unwrap().unwrap().phase,
        Phase::Incomplete
    );
}
#[test]
fn destination_conflict_is_refused_in_both_controls() {
    let control = Fixture::new("conflict-control-");
    fs::rename(control.source(), control.root.join("old.bin")).unwrap();
    fs::write(control.target(), b"CCCC").unwrap();
    assert_eq!(ordinary(&control).state, TransactionState::RollbackFailed);
    let f = fixture("conflict-r2-");
    fs::write(f.target(), b"CCCC").unwrap();
    let mut tx = enrolled_transaction(&f);
    assert!(
        contract::undo_entry(
            &f.root,
            &mut tx,
            0,
            &mut |tx| save(&f, tx),
            &mut |_| Ok(()),
            Fault::None
        )
        .is_err()
    );
    for c in [&control, &f] {
        assert_eq!(fs::read(c.target()).unwrap(), b"CCCC");
        assert!(identity_matches(
            &c.a,
            &capture_identity(&c.root.join("old.bin")).unwrap()
        ));
    }
}
#[test]
fn every_engine_boundary_has_a_deterministic_noop_pause() {
    for edge in [
        Edge::Intent,
        Edge::BeforeCapture,
        Edge::AfterCapture,
        Edge::CapturedRecorded,
        Edge::BeforePublish,
        Edge::AfterPublish,
        Edge::CleanupRecorded,
        Edge::BeforeUnlink,
        Edge::AfterUnlink,
        Edge::BeforeComplete,
        Edge::CompletionRecorded,
    ] {
        let f = fixture("boundary-");
        let tx = run(&f, "noop", edge, Fault::None);
        assert_eq!(tx.state, TransactionState::RolledBack);
        f.assert_a(&f.target());
        assert!(!f.root.join("old.bin").exists());
    }
}
#[test]
fn crash_worker() {
    let Some(root) = std::env::var_os("EMUWIZ_CONTRACT_CRASH") else {
        return;
    };
    let root = PathBuf::from(root);
    let path = journal_path(&root.join("journals"), "contract1i").unwrap();
    let mut tx = read_journal(&path).unwrap();
    if std::env::var_os("EMUWIZ_CONTRACT_INSPECT").is_some() {
        assert_ne!(tx.state, TransactionState::RolledBack);
        assert_eq!(
            contract::evidence(&tx, 0).unwrap().unwrap().phase,
            Phase::CleanupPending
        );
        assert!(!contract::paths(&root, &tx, 0).unwrap().0.exists());
        assert!(!tx.entries[0].source_path.exists());
        fs::write(
            root.join("fresh-inspection.txt"),
            "Original file may be unrecoverable / Undo incomplete; no resume authority",
        )
        .unwrap();
        return;
    }
    contract::undo_entry(
        &root,
        &mut tx,
        0,
        &mut |tx| write_journal(&root.join("journals"), tx).map_err(io::Error::other),
        &mut |edge| {
            if edge == Edge::BeforeUnlink {
                external_at(&root, "delete-dest");
            }
            if edge == Edge::AfterUnlink {
                println!("KILL-HERE");
                std::io::stdout().flush()?;
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                panic!("parent must SIGKILL this worker");
            }
            Ok(())
        },
        Fault::None,
    )
    .unwrap();
}
#[test]
fn sigkill_between_unlink_and_salvage_has_truthful_fresh_process_refusal() {
    let control = control("delete-dest");
    assert!(!control.target().exists());
    let f = fixture("sigkill-");
    let mut tx = enrolled_transaction(&f);
    save(&f, &mut tx).unwrap();
    let test = format!("{TEST_PREFIX}cleanup::contract_tests::crash_worker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test, "--nocapture"])
        .env("EMUWIZ_CONTRACT_CRASH", &f.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line.trim() == "KILL-HERE" {
            break;
        }
    }
    child.kill().unwrap();
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    assert!(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test])
            .env("EMUWIZ_CONTRACT_CRASH", &f.root)
            .env("EMUWIZ_CONTRACT_INSPECT", "1")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        fs::read_to_string(f.root.join("fresh-inspection.txt"))
            .unwrap()
            .contains("may be unrecoverable")
    );
}

#[path = "fd_contract_root.rs"]
mod root_probes;

#[path = "fd_contract_stress.rs"]
mod stress;

#[test]
fn no_operation_control_is_diagnostic_only_not_g3_criterion() {
    let f = fixture("no-operation-control-");
    external_at(&f.root, "overwrite-b");
    assert!(identity_matches(
        &f.a,
        &capture_identity(&f.root.join("e.bin")).unwrap()
    ));
    assert!(!f.root.join("old.bin").exists());
    println!(
        "DIAGNOSTIC ONLY: no-operation control loses B; ordinary Undo control preserves B. G3 uses ordinary Undo."
    );
}

#[test]
fn completion_record_cannot_hide_current_conflicting_files_from_history() {
    let control = control("delete-dest");
    assert!(!control.target().exists());
    let f = fixture("completion-conflict-");
    let mut tx = enrolled_transaction(&f);
    let result = contract::undo_entry(
        &f.root,
        &mut tx,
        0,
        &mut |tx| save(&f, tx),
        &mut |edge| {
            if edge == Edge::CompletionRecorded {
                external_at(&f.root, "delete-dest");
            }
            Ok(())
        },
        Fault::None,
    );
    assert!(
        result.is_err(),
        "a completion checkpoint followed by observed interference cannot be a successful core outcome"
    );
    let path = journal_path(&f.root.join("journals"), "contract1i").unwrap();
    assert!(
        read_journal(&path).is_err(),
        "History must refuse a contradicted completion record"
    );
    let (visible, problems) = crate::dat::rename_apply::list_journals(&f.root.join("journals"));
    assert!(visible.is_empty());
    assert_eq!(problems.len(), 1);
    assert!(path.exists());
}

#[test]
fn legacy_reconciliation_cannot_discard_holding_evidence() {
    let f = fixture("reconcile-held-");
    let mut tx = enrolled_transaction(&f);
    let error = contract::undo_entry(
        &f.root,
        &mut tx,
        0,
        &mut |tx| save(&f, tx),
        &mut |edge| {
            if edge == Edge::CleanupRecorded {
                return Err(io::Error::other("deterministic interruption"));
            }
            Ok(())
        },
        Fault::None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("interruption"));
    let before = tx.clone();
    assert!(
        crate::dat::rename_apply::reconcile_recovery(&mut tx, &f.root.join("journals"))
            .unwrap_err()
            .contains("FD holding evidence")
    );
    assert_eq!(tx, before);
    assert_eq!(crate::platform_evidence_fusion::plan_transaction::assess_recovery(&tx,&[]),crate::platform_evidence_fusion::plan_transaction::RecoveryAssessment::ManualRecoveryRequired);
}

#[test]
fn known_missing_destination_keeps_original_holding_link() {
    let baseline = control("delete-dest");
    assert!(!baseline.target().exists());
    let f = fixture("retain-known-last-link-");
    let tx = run(&f, "delete-dest", Edge::CleanupRecorded, Fault::None);
    assert_eq!(
        contract::evidence(&tx, 0).unwrap().unwrap().phase,
        Phase::Incomplete
    );
    let (held, salvage) = contract::paths(&f.root, &tx, 0).unwrap();
    assert!(identity_matches(&f.a, &capture_identity(&held).unwrap()));
    assert!(!salvage.exists());
}

#[test]
fn bounded_large_file_actual_salvage_cost() {
    let mut f = fixture("large-salvage-");
    let bytes = vec![0x5a; 8 * 1024 * 1024];
    fs::write(f.root.join("old.bin"), &bytes).unwrap();
    f.a = capture_identity(&f.root.join("old.bin")).unwrap();
    let started = std::time::Instant::now();
    let tx = run(&f, "delete-dest", Edge::BeforeUnlink, Fault::None);
    assert_eq!(
        contract::evidence(&tx, 0).unwrap().unwrap().phase,
        Phase::Salvaged
    );
    let copy = contract::paths(&f.root, &tx, 0).unwrap().1;
    assert_eq!(fs::read(&copy).unwrap(), bytes);
    println!(
        "ACTUAL-SALVAGE bytes={} allocated={} elapsed_ms={}",
        bytes.len(),
        fs::metadata(copy).unwrap().blocks() * 512,
        started.elapsed().as_millis()
    );
}

#[test]
fn foreign_return_blocked_by_reoccupied_name_retains_identity_and_receipt() {
    let baseline = Fixture::new("foreign-blocked-control-");
    fs::rename(baseline.source(), baseline.root.join("old.bin")).unwrap();
    let root = baseline.root.clone();
    rollback::BEFORE_REVERSE_RENAME.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            external_at(&root, "substitute");
            rollback::AFTER_REVERSE_RENAME.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || external_at(&root, "occupy-old")));
            });
        }));
    });
    assert_eq!(ordinary(&baseline).state, TransactionState::RollbackFailed);
    crate::dat::rename_apply::dirsync::set_test_hook(None);
    assert_eq!(fs::read(baseline.root.join("old.bin")).unwrap(), b"CCCC");
    assert!(identity_matches(
        &baseline.b,
        &capture_identity(&baseline.target()).unwrap()
    ));
    assert!(identity_matches(
        &baseline.a,
        &capture_identity(&baseline.root.join("t.bin")).unwrap()
    ));
    let f = fixture("foreign-blocked-return-");
    let mut tx = enrolled_transaction(&f);
    contract::undo_entry(
        &f.root,
        &mut tx,
        0,
        &mut |tx| save(&f, tx),
        &mut |edge| {
            if edge == Edge::BeforeCapture {
                external_at(&f.root, "substitute");
            }
            if edge == Edge::AfterCapture {
                external_at(&f.root, "occupy-old");
            }
            Ok(())
        },
        Fault::None,
    )
    .unwrap();
    let evidence = contract::evidence(&tx, 0).unwrap().unwrap();
    assert_eq!(evidence.phase, Phase::ForeignHeld);
    assert_eq!(evidence.held_identity, Some(f.b.clone()));
    assert_eq!(fs::read(f.root.join("old.bin")).unwrap(), b"CCCC");
    let held = contract::paths(&f.root, &tx, 0).unwrap().0;
    assert!(identity_matches(&f.b, &capture_identity(&held).unwrap()));
    let before = fs::read(journal_path(&f.root.join("journals"), "contract1i").unwrap()).unwrap();
    assert!(
        crate::dat::rename_apply::remove_journal(&f.root.join("journals"), "contract1i").is_err()
    );
    assert_eq!(
        fs::read(journal_path(&f.root.join("journals"), "contract1i").unwrap()).unwrap(),
        before
    );
    let (listed, problems) = crate::dat::rename_apply::list_journals(&f.root.join("journals"));
    assert!(problems.is_empty());
    assert_eq!(listed.len(), 1);
    assert_ne!(tx.state, TransactionState::RolledBack);
    assert!(
        contract::undo_entry(
            &f.root,
            &mut tx,
            0,
            &mut |tx| save(&f, tx),
            &mut |_| Ok(()),
            Fault::None
        )
        .unwrap_err()
        .to_string()
        .contains("operator review required")
    );
    assert_eq!(fs::read(held).unwrap(), b"BBBB");
}

#[test]
fn missing_fd_state_never_becomes_completed_history() {
    let f = fixture("missing-fd-state-");
    let mut tx = enrolled_transaction(&f);
    tx.state = TransactionState::RolledBack;
    tx.entries[0].state = crate::dat::rename_apply::EntryState::RolledBack;
    save(&f, &mut tx).unwrap();
    let path = journal_path(&f.root.join("journals"), "contract1i").unwrap();
    assert!(
        read_journal(&path)
            .unwrap_err()
            .to_string()
            .contains("FD runtime missing")
    );
    let (visible, problems) = crate::dat::rename_apply::list_journals(&f.root.join("journals"));
    assert!(visible.is_empty());
    assert_eq!(problems.len(), 1);
    assert!(path.exists());
    assert!(!f.target().exists());
    assert!(identity_matches(
        &f.a,
        &capture_identity(&f.root.join("old.bin")).unwrap()
    ));
}
