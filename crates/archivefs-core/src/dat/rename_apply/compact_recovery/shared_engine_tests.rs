use super::*;
use crate::dat::rename_apply::identity::fd_publication_probe::contract::{self, Action, Phase};
fn fixture_engine(n: usize) -> (Fixture, Rc<Bridge>) {
    let (f, b) = fixture_engine_mode(n, true, true, true);
    b.engine_enabled.set(true);
    (f, b)
}
fn request(f: &Fixture, action: RequestedAction) -> Value {
    let grant = f.issue(action);
    let mut req = f.request(
        if action == RequestedAction::Apply {
            "apply"
        } else {
            "undo"
        },
        Some(&grant),
    );
    req["fd_engine"] = json!(true);
    req
}
#[test]
fn shared_apply_and_legacy_undo_entry_point_round_trip() {
    let (f, b) = fixture_engine(2);
    let result = b.apply(&b.load().unwrap()).unwrap();
    assert_eq!(result.transaction.state, TransactionState::Applied);
    for i in 0..2 {
        let evidence = contract::evidence(&result.transaction, i).unwrap().unwrap();
        assert_eq!(
            (evidence.action, evidence.phase),
            (Action::Apply, Phase::Complete)
        );
        assert!(
            contract::anchor::path(&f.root, &result.transaction, i)
                .unwrap()
                .exists()
        );
    }
    drop(b);
    let result = f.run(request(&f, RequestedAction::Undo));
    assert_eq!(result["success"], true, "{result}");
    f.restored();
    assert_eq!(f.claims(), 0);
}
#[test]
fn shared_runtime_defaults_off_and_never_falls_back_for_enrollment() {
    let (f, b) = fixture_engine_mode(1, true, true, true);
    assert!(!b.engine_enabled.get());
    let original = f.image();
    let result = b.apply(&b.load().unwrap());
    assert!(result.is_err() || result.unwrap().transaction.state != TransactionState::Applied);
    let current = f.image();
    assert_eq!(original, current);
    assert!(f.base().transaction.entries[0].source_path.exists());
    assert!(!f.base().transaction.entries[0].destination_path.exists());
    drop(b);
    assert!(f.claims() > 0);
}
#[test]
fn shared_apply_substitution_never_publishes_foreign_object() {
    let (f, b) = fixture_engine(1);
    drop(b);
    let mut req = request(&f, RequestedAction::Apply);
    req["fd_action"] = json!("foreign-source");
    req["fd_action_edge"] = json!("BeforeCapture");
    let result = f.run(req);
    assert_eq!(result["success"], false, "{result}");
    let tx: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
    let evidence = contract::evidence(&tx, 0).unwrap().unwrap();
    assert_eq!(
        (evidence.action, evidence.phase),
        (Action::Apply, Phase::ForeignHeld)
    );
    assert!(!tx.entries[0].destination_path.exists());
    assert_eq!(
        fs::read(contract::paths(&f.root, &tx, 0).unwrap().0).unwrap(),
        b"foreign B"
    );
    assert_eq!(
        fs::read(f.root.join("external-original.bin")).unwrap(),
        b"original bytes 0"
    );
    assert!(f.claims() > 0);
}

#[test]
fn shared_interrupted_apply_preserves_authority_and_refuses_guessed_resume() {
    for edge in [
        "Intent",
        "BeforeCapture",
        "AfterCapture",
        "CapturedRecorded",
        "BeforePublish",
        "AfterPublish",
        "CleanupRecorded",
        "BeforeUnlink",
        "AfterUnlink",
        "BeforeComplete",
        "CompletionRecorded",
    ] {
        let (f, b) = fixture_engine(2);
        drop(b);
        let mut req = request(&f, RequestedAction::Apply);
        req["fd_exit"] = json!(edge);
        let (out, _) = f.child(req);
        assert_eq!(
            out.status.code(),
            Some(82),
            "{edge}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let observed = f.run(f.request("inspect", None));
        assert_eq!(observed["ok"], true, "{edge}: {observed}");
        assert_ne!(observed["tx"]["state"], "applied", "{edge}");
        assert_ne!(observed["assessment"], "SafeToResume", "{edge}");
        let tx: RenameTransaction = serde_json::from_value(observed["tx"].clone()).unwrap();
        for (i, e) in tx.entries.iter().enumerate() {
            let anchor = contract::anchor::path(&f.root, &tx, i).unwrap();
            assert_eq!(capture_identity(&anchor).unwrap(), e.identity, "{edge}");
            assert_eq!(
                fs::read(anchor).unwrap(),
                format!("original bytes {i}").as_bytes()
            );
        }
        let image = f.image();
        let result = f.run(request(&f, RequestedAction::Apply));
        assert_eq!(result["ok"], false, "{edge}: {result}");
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("spent transaction cannot resume Apply"),
            "{result}"
        );
        assert_eq!(image, f.image());
        assert!(journal::remove_journal(&f.root, "enrolled").is_err());
        assert!(f.claims() > 0);
    }
}

#[test]
fn shared_signed_undo_restart_boundaries_use_real_rollback_dispatch() {
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
        let (f, b) = fixture_engine(1);
        b.apply(&b.load().unwrap()).unwrap();
        drop(b);
        let mut req = request(&f, RequestedAction::Undo);
        req["fd_exit"] = json!(edge);
        assert_eq!(f.child(req).0.status.code(), Some(82), "{edge}");
        let observed = f.run(f.request("inspect", None));
        assert_eq!(observed["ok"], true, "{edge}: {observed}");
        assert_eq!(
            observed["tx"]["state"] == "rolled_back",
            edge == "CompletionRecorded"
        );
        let result = f.run(request(&f, RequestedAction::Undo));
        assert_eq!(result["ok"], true, "{edge}: {result}");
        let complete = !["AfterPublish", "AfterUnlink", "BeforeComplete"].contains(&edge);
        assert_eq!(result["success"], complete, "{edge}: {result}");
        if complete {
            f.restored();
            assert_eq!(f.claims(), 0);
        } else {
            assert!(f.claims() > 0);
            assert_ne!(result["tx"]["state"], "rolled_back");
        }
    }
}

#[test]
fn shared_signed_refusals_reach_exact_approval_checks() {
    let (f, b) = fixture_engine(1);
    let stale = b.load().unwrap();
    b.apply(&stale).unwrap();
    drop(b);
    let mut missing = f.request("undo", None);
    missing["fd_engine"] = json!(true);
    refused_unchanged(&f, missing, "genuine signed");
    let grant = f.issue(RequestedAction::Undo);
    let mut valid = f.request("undo", Some(&grant));
    valid["fd_engine"] = json!(true);
    let mut forged = valid.clone();
    let sig = forged["approval"]["context"]["signed"]["signature"]
        .as_array_mut()
        .unwrap();
    sig[0] = json!(sig[0].as_u64().unwrap() ^ 1);
    refused_unchanged(&f, forged, "signature invalid");
    let mut wrong = valid.clone();
    wrong["approval"]["context"]["verifier"]["public_key"] = json!(vec![0; 32]);
    refused_unchanged(&f, wrong, "cannot select its own issuer");
    let mut old = valid.clone();
    old["receipt"] = serde_json::to_value(stale).unwrap();
    refused_unchanged(&f, old, "stale complete receipt");
    let mut substituted = request(&f, RequestedAction::Apply);
    substituted["mode"] = json!("undo");
    refused_unchanged(
        &f,
        substituted,
        "differs from requested evidence/action/identity",
    );
    let mut consume = valid.clone();
    consume["mode"] = json!("consume");
    assert_eq!(f.run(consume)["ok"], true);
    refused_unchanged(&f, valid, "already consumed");
    assert_eq!(f.run(request(&f, RequestedAction::Undo))["success"], true);
    f.restored();
}

#[test]
fn public_controller_defaults_off_and_performs_shared_round_trip() {
    use super::super::controller::{Controller, RuntimeSwitch};
    let home = tempfile::Builder::new()
        .prefix("emuwiz-r2-controller-")
        .tempdir()
        .unwrap()
        .keep();
    let root = home.join("files");
    private_dir(&root);
    let auth = home.join("authority");
    private_dir(&auth);
    let config = admission::Configuration::create(&auth).unwrap();
    let tx = transaction(&root, 8);
    assert!(Controller::enroll(&root, &tx, &config, RuntimeSwitch::default()).is_err());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 8);
    let c =
        Controller::enroll(&root, &tx, &config, RuntimeSwitch::enabled_for_evaluation()).unwrap();
    let descriptor: Value =
        serde_json::from_slice(&fs::read(root.join("enrolled.json")).unwrap()).unwrap();
    assert_eq!(descriptor["format"], "emuwiz-enrolled-fd-v4");
    let before = c.transaction().unwrap();
    assert!(
        c.apply(
            Default::default(),
            tx.plan_generation,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(c.transaction().unwrap(), before);
    for e in &tx.entries {
        assert_eq!(capture_identity(&e.source_path).unwrap(), e.identity);
        assert!(!e.destination_path.exists());
    }
    let approved = tx
        .entries
        .iter()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        c.apply(approved, tx.plan_generation, &AtomicBool::new(false))
            .unwrap()
            .transaction
            .state,
        TransactionState::Applied
    );
    assert!(c.holding_inventory().is_ok());
    assert!(matches!(
        c.undo(&AtomicBool::new(false)).unwrap().result,
        RollbackResult::FullyRolledBack
    ));
    for e in tx.entries {
        assert_eq!(capture_identity(&e.source_path).unwrap(), e.identity);
        assert!(!e.destination_path.exists());
    }
}

#[test]
fn shared_bare_legacy_undo_refuses_without_admitted_controller() {
    let (f, b) = fixture_engine(1);
    let mut tx = b.apply(&b.load().unwrap()).unwrap().transaction;
    drop(b);
    let image = f.image();
    let err = rollback::rollback_transaction_confined(
        &mut tx,
        &f.root,
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([&f.root]),
    )
    .unwrap_err();
    assert!(err.contains("admitted operation controller"), "{err}");
    assert_eq!(image, f.image());
    assert!(!tx.entries[0].source_path.exists());
    assert_eq!(
        capture_identity(&tx.entries[0].destination_path).unwrap(),
        tx.entries[0].identity
    );
}

#[test]
fn shared_nonprivate_root_refused_before_ownership_or_artifacts() {
    use super::super::controller::{Controller, RuntimeSwitch};
    let home = tempfile::Builder::new()
        .prefix("emuwiz-r2-ineligible-")
        .tempdir()
        .unwrap()
        .keep();
    let root = home.join("files");
    private_dir(&root);
    let auth = home.join("authority");
    private_dir(&auth);
    let config = admission::Configuration::create(&auth).unwrap();
    let tx = transaction(&root, 1);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    let err = Controller::enroll(&root, &tx, &config, RuntimeSwitch::enabled_for_evaluation())
        .err()
        .unwrap();
    assert!(err.to_string().contains("private owned collection root"));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    let db = rusqlite::Connection::open(auth.join("owners.sqlite")).unwrap();
    let claims: i64 = db
        .query_row("SELECT count(*) FROM claims", [], |r| r.get(0))
        .unwrap();
    assert_eq!(claims, 0);
    assert_eq!(
        capture_identity(&tx.entries[0].source_path).unwrap(),
        tx.entries[0].identity
    );
}

#[test]
fn shared_public_controller_authenticates_fresh_process_restart() {
    let (f, b) = fixture_engine(1);
    drop(b);
    let mut apply = request(&f, RequestedAction::Apply);
    apply["public_controller"] = json!(true);
    assert_eq!(f.run(apply)["success"], true);
    let mut undo = request(&f, RequestedAction::Undo);
    undo["public_controller"] = json!(true);
    assert_eq!(f.run(undo)["success"], true);
    f.restored();
    assert_eq!(f.claims(), 0);
}
#[test]
fn shared_engine_two_participants_refuse_objects_aliases_and_both_slots() {
    shared_admission_engine_case(true, true);
}
fn legacy_race(executable: Option<&str>) {
    let (f, b) = fixture_engine(1);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let mut req = request(&f, RequestedAction::Undo);
    req["fd_action"] = json!("legacy-writer");
    req["fd_action_edge"] = json!("BeforeCapture");
    if let Some(executable) = executable {
        req["legacy_executable"] = json!(executable);
    }
    let result = f.run(req);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["success"], false, "{result}");
    let tx: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
    let entry = &tx.entries[0];
    let held = contract::paths(&f.root, &tx, 0).unwrap().0;
    assert!(
        !entry.source_path.exists(),
        "B must never be published as A"
    );
    assert_eq!(fs::read(held).unwrap(), b"current writer replacement");
    assert_eq!(
        capture_identity(&f.root.join("current-writer-saved-original.bin")).unwrap(),
        entry.identity
    );
    assert_eq!(
        contract::evidence(&tx, 0).unwrap().unwrap().phase,
        Phase::ForeignHeld
    );
    assert!(journal::remove_journal(&f.root, "enrolled").is_err());
    let observed = f.run(f.request("inspect", None));
    assert_eq!(observed["tx"], result["tx"]);
    assert!(
        observed["summary"]
            .as_str()
            .unwrap()
            .contains("Held for review")
    );
    assert_ne!(observed["assessment"], "SafeToResume");
    assert!(f.claims() > 0);
    if let Some(executable) = executable {
        let input = f.home.join("old-inspect.json");
        let output = f.home.join("old-inspect-result.json");
        fs::write(
            &input,
            serde_json::to_vec(&f.request("inspect", None)).unwrap(),
        )
        .unwrap();
        let status = Command::new(executable)
            .args([
                "--exact",
                "dat::rename_apply::compact_recovery::storage::integration::tests::worker",
            ])
            .env("EMUWIZ_R2_REQUEST", input)
            .env("EMUWIZ_R2_RESPONSE", &output)
            .status()
            .unwrap();
        assert!(status.success());
        let old: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(
            old["ok"], false,
            "old reader must refuse new descriptor: {old}"
        );
        assert!(
            old["error"]
                .as_str()
                .unwrap()
                .contains("foreign compact descriptor"),
            "{old}"
        );
    }
}
#[test]
fn shared_current_unenrolled_legacy_writer_cannot_cause_wrong_publication() {
    legacy_race(None);
}
#[test]
#[ignore = "requires separately source-built exact current-main binary; run explicitly with EMUWIZ_R2_OLD_BINARY"]
fn shared_old_main_writer_and_old_reader() {
    let executable = std::env::var("EMUWIZ_R2_OLD_BINARY").expect("exact old-main binary required");
    legacy_race(Some(&executable));
}

#[test]
fn shared_failed_apply_cannot_lose_required_runtime_evidence() {
    let (f, b) = fixture_engine(1);
    let mut tx = b.load().unwrap().transaction;
    tx.state = TransactionState::Applying;
    tx.entries[0].state = EntryState::Applying;
    tx.entries[0].unknown.insert(
        contract::KEY.into(),
        serde_json::to_value(contract::Evidence {
            action: Action::Apply,
            phase: Phase::CapturePending,
            held_identity: None,
            salvage_identity: None,
            retention: None,
            detail: "Apply capture pending / Apply incomplete".into(),
        })
        .unwrap(),
    );
    b.checkpoint(&f.root, &mut tx).unwrap();
    tx.state = TransactionState::ApplyFailed;
    tx.entries[0].state = EntryState::ApplyFailed;
    tx.entries[0].failure_reason = Some("injected mutation uncertainty".into());
    b.checkpoint(&f.root, &mut tx).unwrap();
    let snapshot = b.store.scan(&b.base).unwrap();
    let image = f.image();
    let mut missing = tx.clone();
    missing.entries[0].unknown.remove(contract::KEY);
    let runtime = checkpoint::Runtime::capture(&missing);
    assert!(
        runtime.restore(&b.base, &snapshot.stream).is_err(),
        "missing ApplyFailed runtime must never become ordinary legacy failure evidence"
    );
    assert!(b.checkpoint(&f.root, &mut missing).is_err());
    assert_eq!(f.image(), image);
    assert_eq!(b.load().unwrap().transaction, tx);
    assert_eq!(
        capture_identity(&tx.entries[0].source_path).unwrap(),
        tx.entries[0].identity
    );

    // A genuine preflight failure has an explicit NoMutation fact and does
    // not invent a capture receipt. Preserve that distinct legacy outcome.
    let (f2, b2) = fixture_engine(1);
    let mut no_mutation = b2.load().unwrap().transaction;
    no_mutation.state = TransactionState::ApplyFailed;
    no_mutation.entries[0].state = EntryState::ApplyFailed;
    no_mutation.entries[0].failure_reason = Some("preflight refused before mutation".into());
    b2.checkpoint(&f2.root, &mut no_mutation).unwrap();
    assert_eq!(b2.load().unwrap().transaction, no_mutation);
    assert_eq!(
        capture_identity(&no_mutation.entries[0].source_path).unwrap(),
        no_mutation.entries[0].identity
    );
}

#[test]
fn shared_duplicate_hardlink_entries_refuse_before_admission() {
    use super::super::controller::{Controller, RuntimeSwitch};
    let home = tempfile::Builder::new()
        .prefix("emuwiz-r2-duplicate-object-")
        .tempdir()
        .unwrap()
        .keep();
    let root = home.join("files");
    private_dir(&root);
    let auth = home.join("authority");
    private_dir(&auth);
    let config = admission::Configuration::create(&auth).unwrap();
    let mut tx = transaction(&root, 2);
    let alias = root.join("alias.bin");
    fs::hard_link(&tx.entries[0].source_path, &alias).unwrap();
    tx.entries[1].source_path = alias;
    tx.entries[1].identity = tx.entries[0].identity.clone();
    let err = Controller::enroll(&root, &tx, &config, RuntimeSwitch::enabled_for_evaluation())
        .err()
        .unwrap();
    assert!(err.to_string().contains("distinct objects"));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
    let db = rusqlite::Connection::open(auth.join("owners.sqlite")).unwrap();
    let claims: i64 = db
        .query_row("SELECT count(*) FROM claims", [], |r| r.get(0))
        .unwrap();
    assert_eq!(claims, 0);
}
