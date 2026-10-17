use super::*;
use serde_json::json;
use std::process::Command;
fn next_state(f: &Fixture, n: usize) -> PathBuf {
    f._home.path().join(format!("state-{n}"))
}
fn enroll_state(state: &Path) -> RenameTransaction {
    let tx = plan(state, &[("a.bin".into(), "b.bin".into())]).unwrap();
    let bytes = serde_json::to_vec(&tx).unwrap();
    enroll(state, &bytes, &plan_confirmation(&bytes).unwrap(), true).unwrap();
    tx
}
fn perform(state: &Path, signer: &Path, action: RequestedAction) -> Execution {
    let r = review(state, action).unwrap();
    let approval = approve(state, signer, action, &r.confirmation).unwrap();
    execute(state, &approval, action, true).unwrap()
}
fn settle(f: &Fixture, state: &Path) {
    assert!(perform(state, &f.signer, RequestedAction::Apply).completed);
    assert!(perform(state, &f.signer, RequestedAction::Undo).completed);
    assert_eq!(status(state).state, RestartState::Settled);
}
fn advance(state: &Path, next: &Path) {
    let review = preview_reuse(state, next).unwrap();
    reuse(state, next, &review.confirmation, true).unwrap();
    assert_eq!(status(next).state, RestartState::NotEnrolled);
}
fn child(f: &Fixture, request: serde_json::Value, kill: Option<&str>) -> std::process::Output {
    let path = f._home.path().join(format!(
        "worker-{}.json",
        crate::dat::rename_apply::journal::new_transaction_id(0)
    ));
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    let name = format!(
        "{}::reuse_worker",
        module_path!().split_once("::").unwrap().1
    );
    cmd.args(["--exact", &name, "--nocapture"])
        .env("EMUWIZ_REUSE_REQUEST", &path);
    if let Some(kill) = kill {
        cmd.env("EMUWIZ_R2_EXIT", kill);
    }
    if let Some(edge) = request["fd_exit"].as_str() {
        cmd.env("EMUWIZ_CONTRACT_EXIT", edge);
    }
    if request.get("external").is_some() {
        cmd.env(
            "EMUWIZ_CONTRACT_ACTION_REQUEST",
            request["external"].as_str().unwrap(),
        );
    }
    let out = cmd.output().unwrap();
    if out.status.success() {
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("1 passed; 0 failed"),
            "worker did not execute: {out:?}"
        );
    }
    out
}
#[test]
fn reuse_worker() {
    let Some(path) = std::env::var_os("EMUWIZ_REUSE_REQUEST") else {
        return;
    };
    let r: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let state = Path::new(r["state"].as_str().unwrap());
    let result = match r["mode"].as_str().unwrap() {
        "reuse" => reuse(
            state,
            Path::new(r["next"].as_str().unwrap()),
            r["confirmation"].as_str().unwrap(),
            true,
        ),
        "enroll" => {
            let bytes = serde_json::to_vec(&r["plan"]).unwrap();
            enroll(state, &bytes, &plan_confirmation(&bytes).unwrap(), true)
        }
        "undo" => {
            let grant: approval::Approval = serde_json::from_value(r["grant"].clone()).unwrap();
            execute(state, &grant, RequestedAction::Undo, true).map(|v| {
                assert!(!v.completed);
            })
        }
        _ => panic!("unknown worker mode"),
    };
    if let Some(error) = r["expect_error"].as_str() {
        assert!(result.unwrap_err().to_string().contains(error));
    } else {
        result.unwrap();
    }
}
#[test]
fn reuse_three_round_trips_preserve_history_and_reject_old_grants() {
    let f = fixture();
    let original = capture_identity(&f.root.join("a.bin")).unwrap();
    let mut state = f.state.clone();
    let mut ids = std::collections::BTreeSet::new();
    for n in 0..3 {
        let tx = enroll_state(&state);
        assert!(ids.insert(tx.transaction_id.clone()));
        let r = review(&state, RequestedAction::Apply).unwrap();
        let old = approve(&state, &f.signer, RequestedAction::Apply, &r.confirmation).unwrap();
        let unused = approve(&state, &f.signer, RequestedAction::Apply, &r.confirmation).unwrap();
        assert!(
            execute(&state, &old, RequestedAction::Apply, true)
                .unwrap()
                .completed
        );
        assert!(preview_reuse(&state, &next_state(&f, n + 1)).is_err());
        assert!(perform(&state, &f.signer, RequestedAction::Undo).completed);
        let next = next_state(&f, n + 1);
        advance(&state, &next);
        assert!(execute(&state, &old, RequestedAction::Apply, true).is_err());
        assert!(execute(&state, &unused, RequestedAction::Apply, true).is_err());
        let new = enroll_state(&next);
        assert_ne!(new.transaction_id, tx.transaction_id);
        let before = capture_identity(&f.root.join("a.bin")).unwrap();
        assert!(execute(&next, &unused, RequestedAction::Apply, true).is_err());
        assert_eq!(capture_identity(&f.root.join("a.bin")).unwrap(), before);
        assert!(perform(&next, &f.signer, RequestedAction::Apply).completed);
        // Old History remains a historical settlement while its former name is absent.
        let old_path =
            crate::dat::rename_apply::journal::journal_path(&f.root, &tx.transaction_id).unwrap();
        assert_eq!(
            crate::dat::rename_apply::journal::read_journal(&old_path)
                .unwrap()
                .state,
            TransactionState::RolledBack
        );
        let historical = crate::dat::rename_apply::journal::read_journal(&old_path).unwrap();
        assert!(
            crate::dat::rename_apply::journal::verify_applied_containment(&historical).is_err()
        );
        assert_eq!(
            crate::dat::rename_apply::history::classify_recovery_cleanup(&historical),
            crate::dat::rename_apply::history::RecoveryCleanupClassification::Uncertain,
        );
        if n == 0 {
            let seal = f.root.join(NAMESPACE).join("settled-v1.json");
            let saved = fs::read(&seal).unwrap();
            fs::write(&seal, b"{corrupt").unwrap();
            assert!(crate::dat::rename_apply::journal::read_journal(&old_path).is_err());
            let preserved = f._home.path().join("saved-seal.json");
            fs::rename(&seal, &preserved).unwrap();
            assert!(crate::dat::rename_apply::journal::read_journal(&old_path).is_err());
            fs::write(&seal, saved).unwrap();
        }
        assert_eq!(status(&state).state, RestartState::Settled);
        assert!(perform(&next, &f.signer, RequestedAction::Undo).completed);
        assert_eq!(capture_identity(&f.root.join("a.bin")).unwrap(), original);
        // Close this completed registration; the next loop uses a fresh successor.
        let successor = next_state(&f, n + 10);
        advance(&next, &successor);
        state = successor;
        assert_eq!(history(&state).unwrap().len(), (n + 1) * 2);
    }
}
#[test]
fn reuse_each_new_metadata_boundary_restarts_conservatively() {
    let edges = [
        "reuse-before-settlement-publish-created",
        "reuse-before-settlement-publish-written",
        "reuse-before-settlement-publish-synced",
        "reuse-before-settlement-publish",
        "reuse-before-settlement-publish-renamed",
        "reuse-after-settlement-publish",
        "private-directory-created",
        "reuse-after-state-create",
        "reuse-before-config-publish-created",
        "reuse-before-config-publish-written",
        "reuse-before-config-publish-synced",
        "reuse-before-config-publish",
        "reuse-before-config-publish-renamed",
        "reuse-after-config-publish",
        "reuse-before-successor-commit",
        "reuse-successor-row-staged",
        "reuse-successor-row-committed",
        "reuse-after-successor-commit",
    ];
    for edge in edges {
        let f = fixture();
        enroll_state(&f.state);
        settle(&f, &f.state);
        let next = next_state(&f, 1);
        let r = preview_reuse(&f.state, &next).unwrap();
        let out = child(
            &f,
            json!({"mode":"reuse","state":f.state,"next":next,"confirmation":r.confirmation}),
            Some(edge),
        );
        assert_eq!(out.status.code(), Some(79), "{edge} {out:?}");
        let r = preview_reuse(&f.state, &next).unwrap();
        reuse(&f.state, &next, &r.confirmation, true).unwrap();
        reuse(&f.state, &next, &r.confirmation, true).unwrap();
        enroll_state(&next);
        settle(&f, &next);
        assert_eq!(history(&next).unwrap().len(), 1, "{edge}");
    }
}
#[test]
fn reuse_refuses_pending_undo_changed_root_and_stale_evidence() {
    let f = fixture();
    enroll_state(&f.state);
    assert!(preview_reuse(&f.state, &next_state(&f, 1)).is_err());
    assert!(perform(&f.state, &f.signer, RequestedAction::Apply).completed);
    fs::write(f.root.join("a.bin"), b"foreign occupied original").unwrap();
    let refused = review(&f.state, RequestedAction::Undo).unwrap();
    assert!(!refused.actionable);
    assert!(
        approve(
            &f.state,
            &f.signer,
            RequestedAction::Undo,
            &refused.confirmation
        )
        .is_err()
    );
    assert!(preview_reuse(&f.state, &next_state(&f, 1)).is_err());
    assert_eq!(
        fs::read(f.root.join("a.bin")).unwrap(),
        b"foreign occupied original"
    );
    let f = fixture();
    enroll_state(&f.state);
    settle(&f, &f.state);
    let next = next_state(&f, 1);
    let r = preview_reuse(&f.state, &next).unwrap();
    fs::rename(&f.root, f._home.path().join("saved-root")).unwrap();
    fs::create_dir(&f.root).unwrap();
    assert!(reuse(&f.state, &next, &r.confirmation, true).is_err());
    assert!(!next.exists());
    let f = fixture();
    enroll_state(&f.state);
    settle(&f, &f.state);
    advance(&f.state, &next_state(&f, 1));
    let r: Registration =
        serde_json::from_slice(&read(&directory(&f.state).unwrap(), "registration.json").unwrap())
            .unwrap();
    let base = Authority::decode_legacy(&r.original, 1024 * 1024).unwrap();
    let store = Store::reopen(&r.ticket, &base, CAP).unwrap();
    store
        .file
        .write_all_at(&[1], store.layout.length - 1)
        .unwrap();
    store.file.sync_all().unwrap();
    drop(store);
    let tx = plan(&next_state(&f, 1), &[("a.bin".into(), "b.bin".into())]).unwrap();
    let bytes = serde_json::to_vec(&tx).unwrap();
    assert!(
        enroll(
            &next_state(&f, 1),
            &bytes,
            &plan_confirmation(&bytes).unwrap(),
            true
        )
        .is_err()
    );
    assert!(!next_state(&f, 1).join("registration.json").exists());
}
#[test]
fn reuse_one_successor_and_participating_enrollment_serialisation() {
    let f = fixture();
    enroll_state(&f.state);
    settle(&f, &f.state);
    let next = next_state(&f, 1);
    advance(&f.state, &next);
    let other = next_state(&f, 2);
    let review = preview_reuse(&f.state, &other).unwrap();
    assert!(
        reuse(&f.state, &other, &review.confirmation, true)
            .unwrap_err()
            .to_string()
            .contains("different successor")
    );
    assert!(!other.exists());
    let tx = plan(&next, &[("a.bin".into(), "b.bin".into())]).unwrap();
    let c = load(&next).unwrap();
    let guard = c.authority.acquire().unwrap();
    let out = child(
        &f,
        json!({"mode":"enroll","state":next,"plan":tx,"expect_error":"busy"}),
        None,
    );
    assert!(out.status.success(), "{out:?}");
    assert!(!next.join("registration.json").exists());
    drop(guard);
    enroll_state(&next);
    let out = child(
        &f,
        json!({"mode":"enroll","state":next,"plan":tx,"expect_error":"unresolved"}),
        None,
    );
    assert!(out.status.success(), "{out:?}");
}
#[test]
fn reuse_retained_foreign_is_retrievable_during_later_transaction() {
    let f = fixture();
    let first = enroll_state(&f.state);
    assert!(perform(&f.state, &f.signer, RequestedAction::Apply).completed);
    let c = load(&f.state).unwrap();
    let r: Registration =
        serde_json::from_slice(&read(&directory(&f.state).unwrap(), "registration.json").unwrap())
            .unwrap();
    let external = f._home.path().join("external.json");
    fs::write(&external,serde_json::to_vec(&json!({"original":r.original,"authority":c.authority,"fd_action":"foreign-old","fd_action_edge":"BeforeCapture"})).unwrap()).unwrap();
    let review = review(&f.state, RequestedAction::Undo).unwrap();
    let grant = approve(
        &f.state,
        &f.signer,
        RequestedAction::Undo,
        &review.confirmation,
    )
    .unwrap();
    let out = child(
        &f,
        json!({"mode":"undo","state":f.state,"grant":grant,"external":external}),
        None,
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(status(&f.state).state, RestartState::ReviewRequired);
    assert!(preview_reuse(&f.state, &next_state(&f, 1)).is_err());
    assert!(!perform(&f.state, &f.signer, RequestedAction::RetainForeign).completed);
    assert!(perform(&f.state, &f.signer, RequestedAction::Undo).completed);
    advance(&f.state, &next_state(&f, 1));
    let retained = history(&next_state(&f, 1))
        .unwrap()
        .remove(0)
        .retained
        .remove(0);
    assert_eq!(capture_identity(&retained.path).unwrap(), retained.identity);
    assert_eq!(fs::read(&retained.path).unwrap(), b"foreign B");
    enroll_state(&next_state(&f, 1));
    assert!(perform(&next_state(&f, 1), &f.signer, RequestedAction::Apply).completed);
    let prior = history(&next_state(&f, 1)).unwrap();
    assert_eq!(prior[0].transaction.transaction_id, first.transaction_id);
    assert_eq!(
        capture_identity(&prior[0].retained[0].path).unwrap(),
        retained.identity
    );
    assert_eq!(
        status(&f.state).entries[0]
            .retained_foreign
            .as_ref()
            .unwrap()
            .path,
        retained.path
    );
    assert!(perform(&next_state(&f, 1), &f.signer, RequestedAction::Undo).completed);
    assert_eq!(capture_identity(&retained.path).unwrap(), retained.identity);
}

#[test]
fn reuse_refuses_interrupted_and_partially_completed_undo_until_fresh_recovery() {
    for edge in ["Intent", "CapturedRecorded", "CompletionRecorded"] {
        let f = fixture();
        fs::write(f.root.join("c.bin"), b"original C").unwrap();
        let tx = plan(
            &f.state,
            &[
                ("a.bin".into(), "b.bin".into()),
                ("c.bin".into(), "d.bin".into()),
            ],
        )
        .unwrap();
        let bytes = serde_json::to_vec(&tx).unwrap();
        enroll(&f.state, &bytes, &plan_confirmation(&bytes).unwrap(), true).unwrap();
        assert!(perform(&f.state, &f.signer, RequestedAction::Apply).completed);
        let review = review(&f.state, RequestedAction::Undo).unwrap();
        let grant = approve(
            &f.state,
            &f.signer,
            RequestedAction::Undo,
            &review.confirmation,
        )
        .unwrap();
        let out = child(
            &f,
            json!({"mode":"undo","state":f.state,"grant":grant,"fd_exit":edge}),
            None,
        );
        assert_eq!(out.status.code(), Some(82), "{edge} {out:?}");
        assert!(preview_reuse(&f.state, &next_state(&f, 1)).is_err());
        assert!(execute(&f.state, &grant, RequestedAction::Undo, true).is_err());
        assert!(perform(&f.state, &f.signer, RequestedAction::Undo).completed);
        advance(&f.state, &next_state(&f, 1));
        assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
        assert_eq!(fs::read(f.root.join("c.bin")).unwrap(), b"original C");
    }
}
#[test]
fn reuse_refuses_stale_release_missing_receipt_and_ambiguous_generation() {
    let f = fixture();
    enroll_state(&f.state);
    settle(&f, &f.state);
    let db = rusqlite::Connection::open(f.state.join("admission/owners.sqlite")).unwrap();
    db.execute("UPDATE owners SET released=0", []).unwrap();
    assert!(preview_reuse(&f.state, &next_state(&f, 1)).is_err());
    assert!(!next_state(&f, 1).exists());
    drop(db);
    let f = fixture();
    enroll_state(&f.state);
    settle(&f, &f.state);
    advance(&f.state, &next_state(&f, 1));
    fs::rename(
        f.root.join(NAMESPACE).join("settled-v1.json"),
        f._home.path().join("receipt-preserved.json"),
    )
    .unwrap();
    assert!(plan(&next_state(&f, 1), &[("a.bin".into(), "b.bin".into())]).is_err());
    assert!(!next_state(&f, 1).join("registration.json").exists());
    let f = fixture();
    enroll_state(&f.state);
    settle(&f, &f.state);
    advance(&f.state, &next_state(&f, 1));
    fs::create_dir(f.root.join(NAMESPACE).join("unrecognised-generation")).unwrap();
    assert_eq!(
        status(&next_state(&f, 1)).state,
        RestartState::ReviewRequired
    );
    let tx = plan(&next_state(&f, 1), &[("a.bin".into(), "b.bin".into())]).unwrap();
    let bytes = serde_json::to_vec(&tx).unwrap();
    assert!(
        enroll(
            &next_state(&f, 1),
            &bytes,
            &plan_confirmation(&bytes).unwrap(),
            true
        )
        .is_err()
    );
}

#[test]
fn reuse_refuses_non_sibling_state() {
    let f = fixture();
    enroll_state(&f.state);
    settle(&f, &f.state);
    let nested_parent = f._home.path().join("other-parent");
    fs::create_dir(&nested_parent).unwrap();
    fs::set_permissions(
        &nested_parent,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let error = preview_reuse(&f.state, &nested_parent.join("state")).unwrap_err();
    assert!(error.to_string().contains("sibling"), "{error}");
    assert!(!nested_parent.join("state").exists());
    assert_eq!(status(&f.state).state, RestartState::Settled);
}
