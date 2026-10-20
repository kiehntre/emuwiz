//! Contract 1-I: compare with the same ordinary no-clobber Undo, not no-op.
use super::super::contract::{self, Edge, Fault, Phase};
use super::*;
use crate::dat::rename_apply::{RenameTransaction, TransactionState, rollback, write_journal};
use serde_json::json;
use std::sync::atomic::AtomicBool;

fn transaction(f: &Fixture) -> RenameTransaction {
    serde_json::from_value(json!({
        "transaction_id":"contract1i", "plan_generation":1,"created_at_unix":1,
        "source_scan_root":f.root,"state":"applied",
        "entries":[{"source_path":f.target(),"destination_path":f.root.join("old.bin"),
            "original_basename":"s.bin","proposed_basename":"old.bin",
            "identity":f.a,"state":"applied"}]
    }))
    .unwrap()
}

fn enrolled_transaction(f: &Fixture) -> RenameTransaction {
    let mut tx = transaction(f);
    for entry in &mut tx.entries {
        entry.unknown.insert("r2_fd_enrolled".into(), json!(1));
    }
    tx
}
fn ordinary(f: &Fixture) -> RenameTransaction {
    let mut tx = transaction(f);
    fs::create_dir(f.root.join("journals")).unwrap();
    // Undo requires the sealed journal to exist before it runs; write it the
    // way Apply would, so the control exercises the real ordinary Undo.
    write_journal(&f.root.join("journals"), &tx).unwrap();
    let outcome =
        rollback::rollback_transaction(&mut tx, &f.root.join("journals"), &AtomicBool::new(false))
            .unwrap();
    outcome.transaction
}

#[test]
fn external_worker() {
    let Some(root) = std::env::var_os("EMUWIZ_CLEANUP_EXTERNAL") else {
        return;
    };
    let root = PathBuf::from(root);
    // The old library slot is deliberately outside the private holding area.
    let action = std::env::var("EMUWIZ_CLEANUP_ACTION").unwrap();
    let result = perform_external(&root, &action);
    fs::write(
        root.join(format!("external-{action}.txt")),
        format!(
            "{:?}",
            result.as_ref().err().map(std::io::Error::raw_os_error)
        ),
    )
    .unwrap();
}

fn perform_external(root: &Path, action: &str) -> io::Result<()> {
    match action {
        "substitute" => fs::rename(root.join("old.bin"), root.join("t.bin"))
            .and_then(|()| fs::rename(root.join("e.bin"), root.join("old.bin"))),
        "overwrite-c" => fs::rename(root.join("old.bin"), root.join("victim.bin")),
        "noop" => Ok(()),
        "occupy-old" => fs::write(root.join("old.bin"), b"CCCC"),
        "delete-dest" => fs::remove_file(root.join("destination/s.bin")),
        "replace-dest" => fs::rename(root.join("e.bin"), root.join("destination/s.bin")),
        "rename-dest" => fs::rename(root.join("destination/s.bin"), root.join("t.bin")),
        "write-dest" => fs::write(root.join("destination/s.bin"), b"XXXX"),
        "overwrite-b" => fs::rename(root.join("old.bin"), root.join("e.bin")),
        _ => panic!("unknown external action"),
    }
}

fn external_at(root: &Path, action: &str) {
    assert!(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{TEST_PREFIX}cleanup::external_worker")])
            .env("EMUWIZ_CLEANUP_EXTERNAL", root)
            .env("EMUWIZ_CLEANUP_ACTION", action)
            .status()
            .unwrap()
            .success()
    );
}

fn interfere_before_postcheck(root: PathBuf) {
    let mut ran = false;
    crate::dat::rename_apply::dirsync::set_test_hook(Some(Box::new(move |_| {
        if !ran {
            ran = true;
            external_at(&root, "overwrite-c");
        }
        Ok(())
    })));
}

#[test]
fn g3_foreign_object_survives_same_external_rename() {
    let baseline = Fixture::new("g3-ordinary-");
    // source() is initially private in the older probe fixture. Move it to a
    // genuinely untrusted library slot before either Undo starts.
    fs::rename(baseline.source(), baseline.root.join("old.bin")).unwrap();
    assert_eq!(ordinary(&baseline).state, TransactionState::RolledBack);
    external_at(&baseline.root, "overwrite-b");
    assert!(identity_matches(
        &baseline.b,
        &capture_identity(&baseline.root.join("e.bin")).unwrap()
    ));

    let candidate = Fixture::new("g3-r2-");
    fs::rename(candidate.source(), candidate.root.join("old.bin")).unwrap();
    // Focused correction 1: retire the library slot BEFORE publication. The
    // old publish-first algorithm remains reproducible with the same assertion.
    let publish_first = std::env::var_os("EMUWIZ_CLEANUP_PUBLISH_FIRST").is_some();
    if publish_first {
        let p = Publisher::open(
            &candidate.root,
            &candidate.root.join("old.bin"),
            &candidate.target(),
            &candidate.a,
        )
        .unwrap();
        assert!(matches!(
            p.publish(|| {}).unwrap(),
            Publication::LinkedVerified
        ));
        external_at(&candidate.root, "overwrite-b");
    } else {
        let mut tx = enrolled_transaction(&candidate);
        fs::create_dir(candidate.root.join("journals")).unwrap();
        contract::undo_entry(
            &candidate.root,
            &mut tx,
            0,
            &mut |tx| write_journal(&candidate.root.join("journals"), tx).map_err(io::Error::other),
            &mut |edge| {
                if edge == Edge::BeforeUnlink {
                    external_at(&candidate.root, "overwrite-b");
                }
                Ok(())
            },
            Fault::None,
        )
        .unwrap();
        assert_eq!(tx.state, TransactionState::RolledBack);
    }
    candidate.assert_a(&candidate.target());
    let remaining = capture_identity(&candidate.root.join("e.bin")).unwrap();
    println!("G3 ordinary B retained; candidate e.bin identity={remaining:?}");
    assert!(
        identity_matches(&candidate.b, &remaining),
        "G3 VIOLATION: R2's extra old-name alias let the same external rename destroy B; ordinary Undo retained B"
    );
}

#[test]
fn g3_foreign_return_preserves_same_unrelated_object_as_failed_ordinary_undo() {
    let baseline = Fixture::new("g3-return-ordinary-");
    fs::rename(baseline.source(), baseline.root.join("old.bin")).unwrap();
    fs::write(baseline.root.join("victim.bin"), b"CCCC").unwrap();
    let baseline_c = capture_identity(&baseline.root.join("victim.bin")).unwrap();
    let root = baseline.root.clone();
    // Actual legacy Undo: substitute only AFTER its identity check, immediately
    // before its rename syscall. Its wrong-object move is detected, not forced
    // into a successful result. Preserve its real RollbackFailed receipt.
    rollback::BEFORE_REVERSE_RENAME.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            external_at(&root, "substitute");
            rollback::AFTER_REVERSE_RENAME.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || external_at(&root, "overwrite-c")));
            });
        }));
    });
    assert_eq!(ordinary(&baseline).state, TransactionState::RollbackFailed);
    assert_eq!(
        fs::read_to_string(baseline.root.join("external-overwrite-c.txt")).unwrap(),
        format!("{:?}", Some(Some(libc::ENOENT)))
    );
    crate::dat::rename_apply::dirsync::set_test_hook(None);
    assert!(identity_matches(
        &baseline.b,
        &capture_identity(&baseline.target()).unwrap()
    ));
    assert!(identity_matches(
        &baseline_c,
        &capture_identity(&baseline.root.join("victim.bin")).unwrap()
    ));

    let candidate = Fixture::new("g3-return-r2-");
    fs::rename(candidate.source(), candidate.root.join("old.bin")).unwrap();
    fs::write(candidate.root.join("victim.bin"), b"CCCC").unwrap();
    let candidate_c = capture_identity(&candidate.root.join("victim.bin")).unwrap();
    let _pinned_a = Publisher::open(
        &candidate.root,
        &candidate.root.join("old.bin"),
        &candidate.target(),
        &candidate.a,
    )
    .unwrap();
    let mut tx = enrolled_transaction(&candidate);
    fs::create_dir(candidate.root.join("journals")).unwrap();
    let held = if std::env::var_os("EMUWIZ_CLEANUP_RETURN_FOREIGN").is_some() {
        tx.state = TransactionState::RollingBack;
        tx.entries[0].state = crate::dat::rename_apply::EntryState::RollingBack;
        tx.unknown.insert(
            "contract1i_probe".into(),
            json!({"phase":"capture_pending", "holding":candidate.source()}),
        );
        write_journal(&candidate.root.join("journals"), &tx).unwrap();
        external_at(&candidate.root, "substitute");
        assert_eq!(
            retain_old_name(
                &candidate.root,
                &candidate.root.join("old.bin"),
                &candidate.source(),
                &candidate.a,
                || {},
                || interfere_before_postcheck(candidate.root.clone())
            )
            .unwrap(),
            Retirement::ManualReview
        );
        crate::dat::rename_apply::dirsync::set_test_hook(None);
        tx.entries[0].failure_reason = Some("Foreign return raced; Undo incomplete".into());
        write_journal(&candidate.root.join("journals"), &tx).unwrap();
        candidate.source()
    } else {
        // Correction 2, explicitly approved by owner: foreign return is refused
        // when G3 cannot be established. Preserve the object in trusted holding.
        contract::undo_entry(
            &candidate.root,
            &mut tx,
            0,
            &mut |tx| {
                if contract::evidence(tx, 0).unwrap().unwrap().phase == Phase::ForeignHeld {
                    external_at(&candidate.root, "overwrite-c");
                }
                write_journal(&candidate.root.join("journals"), tx).map_err(io::Error::other)
            },
            &mut |edge| {
                if edge == Edge::BeforeCapture {
                    external_at(&candidate.root, "substitute");
                }
                Ok(())
            },
            Fault::None,
        )
        .unwrap();
        assert_eq!(
            contract::evidence(&tx, 0).unwrap().unwrap().phase,
            Phase::ForeignHeld
        );
        assert_eq!(tx.state, TransactionState::RollingBack);
        contract::paths(&candidate.root, &tx, 0).unwrap().0
    };
    assert!(identity_matches(
        &candidate.a,
        &capture_identity(&candidate.root.join("t.bin")).unwrap()
    ));
    assert!(identity_matches(
        &candidate.b,
        &capture_identity(&held).unwrap()
    ));
    assert!(!candidate.target().exists()); // no wrong-object publication
    let remaining = capture_identity(&candidate.root.join("victim.bin")).unwrap();
    println!("G3 failed ordinary Undo retained C; candidate victim identity: {remaining:?}");
    assert!(
        identity_matches(&candidate_c, &remaining),
        "G3 VIOLATION: returning foreign B recreated a library source absent after failed ordinary Undo; the same external rename destroyed unrelated C only with R2"
    );
}

#[path = "fd_contract_tests.rs"]
mod contract_tests;
