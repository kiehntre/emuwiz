use super::*;
use crate::dat::rename_apply::identity::fd_publication_probe::contract::{self, Phase};

#[test]
fn fd_signed_restart_resumes_only_supported_trusted_holding_states() {
    for edge in [
        "Intent",
        "BeforeCapture",
        "AfterCapture",
        "CapturedRecorded",
        "BeforePublish",
        "AfterPublish",
        "CleanupRecorded",
        "BeforeAnchorUnlink",
        "AfterAnchorUnlink",
        "BeforeUnlink",
        "AfterUnlink",
        "BeforeComplete",
        "CompletionRecorded",
    ] {
        let (f, b) = fixture_enrolled(1, true);
        b.apply(&b.load().unwrap()).unwrap();
        drop(b);
        let grant = f.issue(RequestedAction::Undo);
        let mut request = f.request("undo", Some(&grant));
        request["fd_exit"] = json!(edge);
        let (out, _) = f.child(request);
        assert_eq!(
            out.status.code(),
            Some(82),
            "{edge}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(f.claims() > 0);
        let inspected = f.run(f.request("inspect", None));
        assert_eq!(inspected["ok"], true, "{edge}: {inspected}");
        assert_eq!(
            inspected["tx"]["state"] == "rolled_back",
            edge == "CompletionRecorded"
        );
        assert_ne!(inspected["assessment"], "SafeToResume");
        refused_unchanged(&f, f.request("undo", None), "genuine signed");
        let next = f.issue(RequestedAction::Undo);
        let result = f.run(f.request("undo", Some(&next)));
        assert_eq!(result["ok"], true, "{edge}: {result}");
        let complete = !["AfterPublish", "AfterUnlink", "BeforeComplete"].contains(&edge);
        assert_eq!(result["success"], complete, "{edge}: {result}");
        if complete {
            f.restored();
            assert_eq!(f.claims(), 0);
        } else {
            assert!(f.claims() > 0);
            let tx: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
            let phase = contract::evidence(&tx, 0).unwrap().unwrap().phase;
            assert!(matches!(
                phase,
                Phase::DestinationOccupied | Phase::PossiblyUnrecoverable
            ));
            let observed = f.run(f.request("inspect", None));
            assert_eq!(observed["tx"], result["tx"]);
            assert_ne!(observed["tx"]["state"], "rolled_back");
        }
    }
}

#[test]
fn fd_signed_refusals_hit_signature_staleness_and_replay_checks() {
    let (f, b) = fixture_enrolled(1, true);
    let stale = b.load().unwrap();
    b.apply(&stale).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    refused_unchanged(&f, f.request("undo", None), "genuine signed");
    let mut forged = f.request("undo", Some(&grant));
    let sig = forged["approval"]["context"]["signed"]["signature"]
        .as_array_mut()
        .unwrap();
    sig[0] = json!(sig[0].as_u64().unwrap() ^ 1);
    refused_unchanged(&f, forged, "signature invalid");
    let mut wrong = f.request("undo", Some(&grant));
    wrong["approval"]["context"]["verifier"]["public_key"] = json!(vec![0; 32]);
    refused_unchanged(&f, wrong, "cannot select its own issuer");
    let mut old = f.request("undo", Some(&grant));
    old["receipt"] = serde_json::to_value(stale).unwrap();
    refused_unchanged(&f, old, "stale complete receipt");
    let substituted = f.issue(RequestedAction::Apply);
    refused_unchanged(
        &f,
        f.request("undo", Some(&substituted)),
        "differs from requested evidence/action/identity",
    );
    assert_eq!(f.run(f.request("consume", Some(&grant)))["ok"], true);
    refused_unchanged(&f, f.request("undo", Some(&grant)), "already consumed");
    let fresh = f.issue(RequestedAction::Undo);
    let mut req = f.request("undo", Some(&fresh));
    req["release"] = json!(false);
    assert_eq!(f.run(req)["success"], true);
    f.restored();
    // Idempotent completed operation still requires a genuinely new grant.
    let again = f.issue(RequestedAction::Undo);
    assert_eq!(f.run(f.request("undo", Some(&again)))["success"], true);
    assert_eq!(f.claims(), 0);
}

#[test]
fn fd_signed_grant_binds_trusted_holding_observation() {
    let (f, b) = fixture_enrolled(1, true);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let mut req = f.request("undo", Some(&grant));
    req["fd_exit"] = json!("CapturedRecorded");
    assert_eq!(f.child(req).0.status.code(), Some(82));
    let stale = f.issue(RequestedAction::Undo);
    let (held, _) = contract::paths(&f.root, &f.base().transaction, 0).unwrap();
    // Diagnostic trusted-evidence corruption, explicitly outside Contract 1-I:
    // refuse stale approval; never let the signed handoff ignore holding state.
    fs::rename(&held, f.home.join("retained-original")).unwrap();
    fs::write(&held, b"foreign").unwrap();
    refused_unchanged(
        &f,
        f.request("undo", Some(&stale)),
        "differs from requested evidence/action/identity",
    );
    assert_eq!(fs::read(&held).unwrap(), b"foreign");
    assert!(f.claims() > 0);
}

#[test]
fn fd_signed_salvage_and_foreign_holding_require_operator_review() {
    for (action, edge, exit, phase) in [
        (
            "delete-destination",
            "BeforeUnlink",
            "BeforeSalvage",
            Phase::SalvagePending,
        ),
        (
            "delete-destination",
            "BeforeUnlink",
            "AfterSalvage",
            Phase::Salvaged,
        ),
        ("foreign-old", "BeforeCapture", "", Phase::ForeignHeld),
    ] {
        let (f, b) = fixture_enrolled(1, true);
        b.apply(&b.load().unwrap()).unwrap();
        drop(b);
        let grant = f.issue(RequestedAction::Undo);
        let mut request = f.request("undo", Some(&grant));
        request["fd_action"] = json!(action);
        request["fd_action_edge"] = json!(edge);
        request["fd_exit"] = json!(exit);
        let (out, result) = f.child(request);
        if exit.is_empty() {
            assert!(out.status.success());
            assert_eq!(result.unwrap()["success"], false);
        } else {
            assert_eq!(out.status.code(), Some(82));
        }
        let observed = f.run(f.request("inspect", None));
        assert_eq!(observed["ok"], true, "{observed}");
        let tx: RenameTransaction = serde_json::from_value(observed["tx"].clone()).unwrap();
        assert_eq!(contract::evidence(&tx, 0).unwrap().unwrap().phase, phase);
        assert_ne!(tx.state, TransactionState::RolledBack);
        assert!(
            !observed["summary"]
                .as_str()
                .unwrap()
                .contains("Run recovery reconciliation")
        );
        assert!(
            observed["summary"]
                .as_str()
                .unwrap()
                .contains("Undo incomplete")
        );
        assert!(journal::remove_journal(&f.root, "enrolled").is_err());
        assert!(f.claims() > 0);
        refused_unchanged(&f, f.request("undo", None), "genuine signed");
        let review = f.issue(RequestedAction::Undo);
        refused_unchanged(
            &f,
            f.request("undo", Some(&review)),
            "operator review required",
        );
        let (held, salvage) = contract::paths(&f.root, &tx, 0).unwrap();
        match phase {
            Phase::ForeignHeld => {
                assert_eq!(fs::read(held).unwrap(), b"foreign B");
                assert!(f.root.join("external-original.bin").exists());
            }
            Phase::Salvaged => {
                let copy = capture_identity(&salvage).unwrap();
                assert_ne!(copy.ino, tx.entries[0].identity.ino);
                assert_eq!(fs::read(salvage).unwrap(), b"original bytes 0");
            }
            Phase::SalvagePending => assert!(!held.exists() && !salvage.exists()),
            _ => unreachable!(),
        }
        assert!(f.claims() > 0);
    }
}

#[test]
fn fd_signed_restart_never_infers_original_object_from_reused_inode() {
    use std::os::unix::fs::MetadataExt;
    let (f, b) = fixture_mode(1, true, false);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let mut req = f.request("undo", Some(&grant));
    req["fd_exit"] = json!("Intent");
    assert_eq!(f.child(req).0.status.code(), Some(82));
    let tx = f.base().transaction;
    let entry = &tx.entries[0];
    let old = &entry.destination_path;
    let original_mtime = fs::metadata(old).unwrap().modified().unwrap();
    let original_inode = fs::metadata(old).unwrap().ino();
    let bytes = fs::read(old).unwrap();
    fs::remove_file(old).unwrap();
    let mut reused = false;
    for _ in 0..2000 {
        fs::write(old, &bytes).unwrap();
        if fs::metadata(old).unwrap().ino() == original_inode {
            reused = true;
            break;
        }
        fs::remove_file(old).unwrap();
    }
    assert!(
        reused,
        "bounded fixture could not reproduce inode reuse; no safety claim made"
    );
    fs::File::options()
        .write(true)
        .open(old)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(original_mtime))
        .unwrap();
    assert_eq!(
        capture_identity(old).unwrap(),
        entry.identity,
        "reproducer requires indistinguishable legacy identity evidence"
    );
    let fresh = f.issue(RequestedAction::Undo);
    let result = f.run(f.request("undo", Some(&fresh)));
    assert_ne!(
        result["success"], true,
        "G1 VIOLATION: a new inode incarnation was published as the original after restart"
    );
    assert!(
        !entry.source_path.exists(),
        "replacement must not be published by EmuWiz"
    );
    assert_eq!(fs::read(old).unwrap(), bytes);
    assert!(f.claims() > 0);
}

#[test]
fn fd_enrollment_anchor_preserves_identity_before_any_undo_checkpoint() {
    use std::os::unix::fs::MetadataExt;
    let (f, b) = fixture_enrolled(1, true);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let tx = f.base().transaction;
    let e = &tx.entries[0];
    let anchor = contract::anchor::path(&f.root, &tx, 0).unwrap();
    assert_eq!(capture_identity(&anchor).unwrap(), e.identity);
    fs::remove_file(&e.destination_path).unwrap();
    for _ in 0..100 {
        fs::write(&e.destination_path, b"original bytes 0").unwrap();
        assert_ne!(
            fs::metadata(&e.destination_path).unwrap().ino(),
            e.identity.ino,
            "a trusted surviving hardlink must prevent original inode reuse"
        );
        fs::remove_file(&e.destination_path).unwrap();
    }
    fs::write(&e.destination_path, b"foreign B").unwrap();
    let grant = f.issue(RequestedAction::Undo);
    let result = f.run(f.request("undo", Some(&grant)));
    assert_eq!(result["ok"], false);
    assert!(!e.source_path.exists());
    assert_eq!(fs::read(&e.destination_path).unwrap(), b"foreign B");
    assert_eq!(capture_identity(&anchor).unwrap(), e.identity);
    assert!(f.claims() > 0);
}

#[test]
fn fd_admission_refuses_competing_processes_aliases_and_both_slots() {
    shared_admission_case(true);
}
#[test]
fn fd_multifile_signed_restart_restores_exact_objects() {
    let (f, b) = fixture_enrolled(2, true);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let mut req = f.request("undo", Some(&grant));
    req["fd_exit"] = json!("AfterCapture");
    assert_eq!(f.child(req).0.status.code(), Some(82));
    assert!(f.claims() > 0);
    let grant = f.issue(RequestedAction::Undo);
    let result = f.run(f.request("undo", Some(&grant)));
    assert_eq!(result["success"], true);
    f.restored();
    assert_eq!(f.claims(), 0);
}

#[test]
fn fd_missing_enrollment_anchor_refuses_read_and_signed_mutation() {
    let (f, b) = fixture_enrolled(1, true);
    drop(b);
    let tx = f.base().transaction;
    let anchor = contract::anchor::path(&f.root, &tx, 0).unwrap();
    fs::rename(&anchor, f.home.join("anchor-retained-for-review")).unwrap();
    let inspected = f.run(f.request("inspect", None));
    assert_eq!(inspected["ok"], false);
    assert!(
        inspected["error"]
            .as_str()
            .unwrap()
            .contains("enrollment anchor unavailable")
    );
    let grant = f.issue(RequestedAction::Apply);
    let result = f.run(f.request("apply", Some(&grant)));
    assert_eq!(result["ok"], false);
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("enrollment anchor unavailable")
    );
    assert_eq!(
        capture_identity(&tx.entries[0].source_path).unwrap(),
        tx.entries[0].identity
    );
    assert!(!tx.entries[0].destination_path.exists());
    assert!(f.claims() > 0);
}

#[test]
fn fd_enrollment_crashes_preserve_authority_and_never_guess_anchor_setup() {
    for edge in ["before-anchor-publication", "after-anchor-publication"] {
        let (f, b) = fixture(1);
        drop(b);
        let root = f.home.join("new-enrolled-root");
        private_dir(&root);
        let mut tx = transaction(&root, 1);
        tx.transaction_id = "anchor-setup".into();
        tx.entries[0]
            .unknown
            .insert("r2_fd_enrolled".into(), json!(1));
        tx.entries[0]
            .unknown
            .insert(contract::anchor::ENROLLED.into(), json!(true));
        let mut request = f.request("admit", None);
        request["original"] = json!(serde_json::to_vec(&tx).unwrap());
        request["exit"] = json!(edge);
        assert_eq!(f.child(request.clone()).0.status.code(), Some(79));
        assert_eq!(
            capture_identity(&tx.entries[0].source_path).unwrap(),
            tx.entries[0].identity
        );
        assert!(!tx.entries[0].destination_path.exists());
        assert!(f.claims() > 0);
        request["mode"] = json!("inspect");
        request["exit"] = serde_json::Value::Null;
        let observed = f.run(request);
        if edge == "before-anchor-publication" {
            assert_eq!(observed["ok"], false);
            assert!(
                observed["error"]
                    .as_str()
                    .unwrap()
                    .contains("enrollment anchor unavailable")
            );
        } else {
            assert_eq!(observed["ok"], true);
            assert_eq!(observed["tx"]["state"], "planned");
            assert_eq!(
                capture_identity(&contract::anchor::path(&root, &tx, 0).unwrap()).unwrap(),
                tx.entries[0].identity
            );
        }
        assert!(
            journal::journal_path(&root, "anchor-setup")
                .unwrap()
                .exists()
        );
    }
}

#[test]
fn legacy_opaque_fd_field_names_do_not_enroll_or_intercept() {
    let (f, original_bridge) = fixture(1);
    drop(original_bridge);
    let root = f.home.join("opaque-legacy-root");
    private_dir(&root);
    let mut tx = transaction(&root, 1);
    tx.transaction_id = "opaque-legacy".into();
    tx.entries[0]
        .unknown
        .insert(contract::KEY.into(), json!({"future":"opaque"}));
    tx.entries[0]
        .unknown
        .insert(contract::anchor::ENROLLED.into(), json!(true));
    tx.entries[0]
        .unknown
        .insert("r2_fd_enrolled".into(), json!("opaque legacy text"));
    let original_unknown = tx.entries[0].unknown.clone();
    let base = Authority::decode_legacy(&serde_json::to_vec(&tx).unwrap(), 1024 * 1024).unwrap();
    let bridge = Bridge::create(&root, base, &f.config).unwrap();
    assert!(
        !root.join(".emuwiz-r2-holding").exists(),
        "opaque legacy metadata must not create anchors"
    );
    bridge.apply(&bridge.load().unwrap()).unwrap();
    assert_eq!(
        bridge.load().unwrap().transaction.entries[0].unknown,
        original_unknown
    );
    let outcome = bridge
        .undo(&bridge.load().unwrap(), &AtomicBool::new(false))
        .unwrap();
    assert!(matches!(outcome.result, RollbackResult::FullyRolledBack));
    assert_eq!(outcome.transaction.entries[0].unknown, original_unknown);
    assert_eq!(
        capture_identity(&tx.entries[0].source_path).unwrap(),
        tx.entries[0].identity
    );
    bridge.release().unwrap();
    assert!(!root.join(".emuwiz-r2-holding").exists());
}

#[test]
fn fd_checkpoint_cannot_drop_required_runtime_state() {
    let (f, b) = fixture_enrolled(1, true);
    b.apply(&b.load().unwrap()).unwrap();
    let mut pending = b.load().unwrap().transaction;
    pending.state = TransactionState::RollingBack;
    pending.entries[0].state = EntryState::RollingBack;
    pending.entries[0].unknown.insert(
        contract::KEY.into(),
        serde_json::to_value(contract::Evidence {
            action: contract::Action::Undo,
            phase: Phase::CapturePending,
            held_identity: None,
            salvage_identity: None,
            detail: "Capture pending / Undo incomplete".into(),
        })
        .unwrap(),
    );
    b.checkpoint(&f.root, &mut pending).unwrap();
    let snapshot = b.store.scan(&b.base).unwrap();
    let image = f.image();
    let mut missing = pending.clone();
    missing.entries[0].unknown.remove(contract::KEY);
    assert!(
        b.checkpoint(&f.root, &mut missing)
            .unwrap_err()
            .to_string()
            .contains("FD runtime missing")
    );
    assert_eq!(f.image(), image);
    let runtime = checkpoint::Runtime::capture(&missing);
    assert!(
        runtime
            .restore(&b.base, &snapshot.stream)
            .unwrap_err()
            .to_string()
            .contains("FD runtime missing")
    );
    assert_eq!(b.load().unwrap().transaction, pending);
    assert_eq!(
        capture_identity(&pending.entries[0].destination_path).unwrap(),
        pending.entries[0].identity
    );
}

#[test]
fn fd_cancellation_persists_incomplete_state_and_retains_ownership() {
    let (f, b) = fixture_enrolled(1, true);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let mut request = f.request("undo", Some(&grant));
    request["cancel"] = json!(true);
    let result = f.run(request);
    assert_eq!(result["ok"], true);
    assert_eq!(result["success"], false);
    assert_eq!(result["tx"]["state"], "rolling_back");
    assert!(
        result["tx"]["entries"][0]["failure_reason"]
            .as_str()
            .unwrap()
            .contains("cancelled")
    );
    assert!(
        result["runtime"]
            .as_str()
            .unwrap()
            .contains("UndoPartiallyCompleted")
    );
    let observed = f.run(f.request("inspect", None));
    assert_eq!(observed["tx"], result["tx"]);
    assert!(
        observed["summary"]
            .as_str()
            .unwrap()
            .contains("unresolved: 1")
    );
    assert!(f.claims() > 0);
    let fresh = f.issue(RequestedAction::Undo);
    assert_eq!(f.run(f.request("undo", Some(&fresh)))["success"], true);
    f.restored();
    assert_eq!(f.claims(), 0);
}

#[test]
fn fd_restart_classifies_unrecorded_foreign_capture_without_return() {
    let (f, b) = fixture_enrolled(1, true);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let mut request = f.request("undo", Some(&grant));
    request["fd_action"] = json!("foreign-old");
    request["fd_action_edge"] = json!("BeforeCapture");
    request["fd_exit"] = json!("AfterCapture");
    assert_eq!(f.child(request).0.status.code(), Some(82));
    let observed = f.run(f.request("inspect", None));
    let tx: RenameTransaction = serde_json::from_value(observed["tx"].clone()).unwrap();
    assert_eq!(
        contract::evidence(&tx, 0).unwrap().unwrap().phase,
        Phase::CapturePending
    );
    let held = contract::paths(&f.root, &tx, 0).unwrap().0;
    let foreign_identity = capture_identity(&held).unwrap();
    assert_eq!(fs::read(&held).unwrap(), b"foreign B");
    let grant = f.issue(RequestedAction::Undo);
    let result = f.run(f.request("undo", Some(&grant)));
    assert_eq!(result["ok"], true);
    assert_eq!(result["success"], false);
    let tx: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
    let evidence = contract::evidence(&tx, 0).unwrap().unwrap();
    assert_eq!(evidence.phase, Phase::ForeignHeld);
    assert_eq!(evidence.held_identity, Some(foreign_identity));
    assert!(!tx.entries[0].source_path.exists());
    assert!(!tx.entries[0].destination_path.exists());
    assert_eq!(
        capture_identity(&f.root.join("external-original.bin")).unwrap(),
        tx.entries[0].identity
    );
    let review = f.issue(RequestedAction::Undo);
    refused_unchanged(
        &f,
        f.request("undo", Some(&review)),
        "operator review required",
    );
    assert_eq!(fs::read(held).unwrap(), b"foreign B");
    assert!(f.claims() > 0);
}

#[test]
fn fd_ineligible_admission_leaves_no_claims_or_private_artifacts() {
    use std::os::unix::fs::MetadataExt;
    let (f, bridge) = fixture_enrolled(1, true);
    drop(bridge);
    let before_claims = f.claims();
    let root = tempfile::Builder::new()
        .prefix("emuwiz-fd-ineligible-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/dev/shm")
        .unwrap()
        .keep();
    let mut tx = transaction(&root, 1);
    tx.transaction_id = "ineligible-fd".into();
    tx.entries[0]
        .unknown
        .insert("r2_fd_enrolled".into(), json!(1));
    tx.entries[0]
        .unknown
        .insert("r2_fd_anchor_v1".into(), json!(true));
    let source = tx.entries[0].source_path.clone();
    let expected = capture_identity(&source).unwrap();
    let bytes = serde_json::to_vec(&tx).unwrap();
    let base = Authority::decode_legacy(&bytes, 1024 * 1024).unwrap();
    let error = match Bridge::create(&root, base, &f.config) {
        Ok(_) => panic!("ineligible root admitted"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("eligibility"), "{error}");
    assert_eq!(f.claims(), before_claims);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    assert_eq!(capture_identity(&source).unwrap(), expected);
    assert_eq!(fs::metadata(&source).unwrap().nlink(), 1);
    println!(
        "INELIGIBLE REFUSAL retained fixture {}: {error}",
        root.display()
    );
}
