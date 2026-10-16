use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
struct Fixture {
    _home: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
    signer: PathBuf,
}
fn fixture() -> Fixture {
    let home = tempfile::Builder::new()
        .prefix("emuwiz-r2-deploy-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = home.path().join("files");
    create_directory(&root).unwrap();
    let state = home.path().join("state");
    let signer = home.path().join("signer");
    let public = issuer_init(&signer).unwrap();
    provision(&root, &state, &public, "fixture operator", ACCEPT).unwrap();
    fs::write(root.join("a.bin"), b"original A").unwrap();
    Fixture {
        _home: home,
        root,
        state,
        signer,
    }
}
fn enrolled(f: &Fixture) {
    let tx = plan(&f.state, &[("a.bin".into(), "b.bin".into())]).unwrap();
    let bytes = serde_json::to_vec(&tx).unwrap();
    enroll(&f.state, &bytes, &plan_confirmation(&bytes).unwrap(), true).unwrap();
}
fn grant(f: &Fixture, action: RequestedAction) -> approval::Approval {
    let r = review(&f.state, action).unwrap();
    approve(&f.state, &f.signer, action, &r.confirmation).unwrap()
}
#[test]
fn provisioned_signed_round_trip_preserves_inode_and_replay_refuses() {
    let f = fixture();
    let id = capture_identity(&f.root.join("a.bin")).unwrap();
    enrolled(&f);
    let approval = grant(&f, RequestedAction::Apply);
    assert!(
        execute(&f.state, &approval, RequestedAction::Apply, false)
            .unwrap_err()
            .to_string()
            .contains("OFF")
    );
    assert!(
        execute(&f.state, &approval, RequestedAction::Apply, true)
            .unwrap()
            .completed
    );
    assert_eq!(capture_identity(&f.root.join("b.bin")).unwrap(), id);
    assert!(execute(&f.state, &approval, RequestedAction::Apply, true).is_err());
    let undo = grant(&f, RequestedAction::Undo);
    assert!(
        execute(&f.state, &undo, RequestedAction::Undo, true)
            .unwrap()
            .completed
    );
    assert_eq!(capture_identity(&f.root.join("a.bin")).unwrap(), id);
    assert!(!f.root.join("b.bin").exists());
    assert!(review(&f.state, RequestedAction::Undo).is_err()); // verified release, not reusable authority
}
#[test]
fn defaults_and_plan_confirmation_refuse_before_enrollment() {
    let f = fixture();
    let tx = plan(&f.state, &[("a.bin".into(), "b.bin".into())]).unwrap();
    let bytes = serde_json::to_vec(&tx).unwrap();
    assert!(enroll(&f.state, &bytes, &hash(&bytes), false).is_err());
    assert!(enroll(&f.state, &bytes, "wrong", true).is_err());
    assert!(!f.state.join("registration.json").exists());
    assert!(!f.root.join(NAMESPACE).exists());
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
}
#[test]
fn changed_review_wrong_issuer_and_substituted_grant_refuse() {
    let f = fixture();
    enrolled(&f);
    let r = review(&f.state, RequestedAction::Apply).unwrap();
    let other = f._home.path().join("other-signer");
    issuer_init(&other).unwrap();
    assert!(
        approve(&f.state, &other, RequestedAction::Apply, &r.confirmation)
            .err()
            .unwrap()
            .to_string()
            .contains("configured issuer")
    );
    fs::write(f.root.join("b.bin"), b"foreign B").unwrap();
    assert!(
        approve(&f.state, &f.signer, RequestedAction::Apply, &r.confirmation)
            .err()
            .unwrap()
            .to_string()
            .contains("stale")
    );
    fs::remove_file(f.root.join("b.bin")).unwrap();
    let g = grant(&f, RequestedAction::Apply);
    assert!(execute(&f.state, &g, RequestedAction::Undo, true).is_err());
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
}
#[test]
fn missing_or_modified_registration_and_root_binding_refuse() {
    let f = fixture();
    assert!(review(&f.state, RequestedAction::Undo).is_err());
    enrolled(&f);
    let registration = f.state.join("registration.json");
    let original = fs::read(&registration).unwrap();
    fs::write(&registration, b"{broken").unwrap();
    assert!(review(&f.state, RequestedAction::Undo).is_err());
    fs::write(&registration, original).unwrap();
    fs::write(
        f.root.join(".emuwiz-r2-holding").join(BINDING),
        b"foreign authority",
    )
    .unwrap();
    assert!(review(&f.state, RequestedAction::Undo).is_err());
}
#[test]
fn permissions_symlinks_and_second_authority_are_refused() {
    let f = fixture();
    let public =
        serde_json::from_slice::<Vec<u8>>(&fs::read(f.signer.join("public.json")).unwrap())
            .unwrap();
    assert!(
        provision(
            &f.root,
            &f._home.path().join("second"),
            &public,
            "operator",
            ACCEPT
        )
        .is_err()
    );
    let alias = f._home.path().join("alias");
    std::os::unix::fs::symlink(&f.state, &alias).unwrap();
    assert!(load(&alias).is_err());
    fs::set_permissions(
        f.state.join("configuration.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(load(&f.state).is_err());
    fs::set_permissions(
        f.state.join("configuration.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(load(&f.state).is_err());
}
#[test]
fn enrollment_limits_and_occupied_root_never_fallback() {
    let f = fixture();
    assert!(plan(&f.state, &[("../a.bin".into(), "b.bin".into())]).is_err());
    assert!(plan(&f.state, &vec![("a.bin".into(), "b.bin".into()); 9]).is_err());
    enrolled(&f);
    let tx = plan(&f.state, &[("a.bin".into(), "c.bin".into())]).unwrap();
    let bytes = serde_json::to_vec(&tx).unwrap();
    assert!(enroll(&f.state, &bytes, &hash(&bytes), true).is_err());
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
}

#[test]
fn deployment_worker() {
    let Some(path) = std::env::var_os("EMUWIZ_DEPLOY_REQUEST") else {
        return;
    };
    let request: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let grant = serde_json::from_value(request["grant"].clone()).unwrap();
    let action = if request["action"] == "apply" {
        RequestedAction::Apply
    } else {
        RequestedAction::Undo
    };
    let result = execute(
        Path::new(request["state"].as_str().unwrap()),
        &grant,
        action,
        true,
    )
    .unwrap();
    fs::write(
        request["output"].as_str().unwrap(),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
}
#[test]
fn nested_failed_undo_is_reported_incomplete_with_foreign_object_retained() {
    let f = fixture();
    enrolled(&f);
    let apply = grant(&f, RequestedAction::Apply);
    assert!(
        execute(&f.state, &apply, RequestedAction::Apply, true)
            .unwrap()
            .completed
    );
    let undo = grant(&f, RequestedAction::Undo);
    let c = load(&f.state).unwrap();
    let r: Registration =
        serde_json::from_slice(&fs::read(f.state.join("registration.json")).unwrap()).unwrap();
    let external = f._home.path().join("external.json");
    fs::write(&external,serde_json::to_vec(&serde_json::json!({"original":r.original,"authority":c.authority,"fd_action":"foreign-old","fd_action_edge":"BeforeCapture"})).unwrap()).unwrap();
    let output = f._home.path().join("result.json");
    let request = f._home.path().join("request.json");
    fs::write(
        &request,
        serde_json::to_vec(&serde_json::json!({"state":f.state,"grant":undo,"output":output}))
            .unwrap(),
    )
    .unwrap();
    let child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","dat::rename_apply::compact_recovery::storage::integration::deployment::tests::deployment_worker","--nocapture"]).env("EMUWIZ_DEPLOY_REQUEST",request).env("EMUWIZ_CONTRACT_ACTION_REQUEST",external).output().unwrap();
    assert!(
        child.status.success(),
        "{} {}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(result["completed"], false);
    assert!(!f.root.join("a.bin").exists());
    assert_eq!(
        fs::read(f.root.join("external-original.bin")).unwrap(),
        b"original A"
    );
    let report = review(&f.state, RequestedAction::Undo).unwrap();
    assert_ne!(report.transaction.state, TransactionState::RolledBack);
    let held = contract::paths(&f.root, &report.transaction, 0).unwrap().0;
    assert_eq!(fs::read(held).unwrap(), b"foreign B");
}

#[test]
fn interrupted_apply_is_recovered_through_the_operator_workflow() {
    let f = fixture();
    let id = capture_identity(&f.root.join("a.bin")).unwrap();
    enrolled(&f);
    let apply = grant(&f, RequestedAction::Apply);
    let output = f._home.path().join("apply-result.json");
    let request = f._home.path().join("apply-request.json");
    fs::write(
        &request,
        serde_json::to_vec(
            &serde_json::json!({"state":f.state,"grant":apply,"output":output,"action":"apply"}),
        )
        .unwrap(),
    )
    .unwrap();
    // The operator's Apply process dies right after the original was captured
    // into trusted holding and before anything was published.
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "dat::rename_apply::compact_recovery::storage::integration::deployment::tests::deployment_worker",
            "--nocapture",
        ])
        .env("EMUWIZ_DEPLOY_REQUEST", &request)
        .env("EMUWIZ_CONTRACT_EXIT", "CapturedRecorded")
        .output()
        .unwrap();
    assert_eq!(
        child.status.code(),
        Some(82),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(!output.exists());
    assert!(!f.root.join("a.bin").exists());
    assert!(!f.root.join("b.bin").exists());
    let report = review(&f.state, RequestedAction::Undo).unwrap();
    assert_eq!(report.transaction.state, TransactionState::Applying);
    assert!(!report.checkpoint_pending);
    assert_eq!(report.holding["retained_entries"], 2, "{}", report.holding);
    // A spent Apply cannot be resumed, even under a fresh reviewed grant.
    let retry = grant(&f, RequestedAction::Apply);
    assert!(
        execute(&f.state, &retry, RequestedAction::Apply, true)
            .unwrap_err()
            .to_string()
            .contains("spent transaction cannot resume Apply")
    );
    assert!(!f.root.join("a.bin").exists());
    let undo = grant(&f, RequestedAction::Undo);
    let result = execute(&f.state, &undo, RequestedAction::Undo, true).unwrap();
    assert!(result.completed, "{:?}", result.transaction);
    assert_eq!(capture_identity(&f.root.join("a.bin")).unwrap(), id);
    assert!(!f.root.join("b.bin").exists());
    let held = contract::paths(&f.root, &result.transaction, 0).unwrap().0;
    assert!(!held.exists());
    assert!(
        !contract::anchor::path(&f.root, &result.transaction, 0)
            .unwrap()
            .exists()
    );
    assert!(review(&f.state, RequestedAction::Undo).is_err()); // verified release
}

#[test]
fn interrupted_recovery_of_an_unmutated_batch_resumes_through_the_operator_workflow() {
    // Independent-review reproduction (2026-10-16): the LAST entry recovery
    // settles is an unmutated one; its record must not seal the transaction
    // while its anchor is still linked, or every later review refuses.
    let f = fixture();
    fs::write(f.root.join("c.bin"), b"original C").unwrap();
    let ids = [
        capture_identity(&f.root.join("a.bin")).unwrap(),
        capture_identity(&f.root.join("c.bin")).unwrap(),
    ];
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
    let worker = |action: &str, grant: &approval::Approval, exit: &str, entry: &str| {
        let output = f._home.path().join(format!("{action}-{exit}-out.json"));
        let request = f._home.path().join(format!("{action}-{exit}-req.json"));
        fs::write(
            &request,
            serde_json::to_vec(
                &serde_json::json!({"state":f.state,"grant":grant,"output":output,"action":action}),
            )
            .unwrap(),
        )
        .unwrap();
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dat::rename_apply::compact_recovery::storage::integration::deployment::tests::deployment_worker",
                "--nocapture",
            ])
            .env("EMUWIZ_DEPLOY_REQUEST", &request)
            .env("EMUWIZ_CONTRACT_EXIT", exit)
            .env("EMUWIZ_CONTRACT_EXIT_ENTRY", entry)
            .output()
            .unwrap()
    };
    // Apply dies before touching anything: entry 0 CapturePending, entry 1 Planned.
    let apply = grant(&f, RequestedAction::Apply);
    assert_eq!(
        worker("apply", &apply, "Intent", "0").status.code(),
        Some(82)
    );
    // Signed Undo dies after entry 0's unmutated record, before its anchor unlink.
    let undo = grant(&f, RequestedAction::Undo);
    assert_eq!(
        worker("undo", &undo, "BeforeAnchorUnlink", "0")
            .status
            .code(),
        Some(82)
    );
    let anchor = contract::anchor::path(&f.root, &tx, 0).unwrap();
    assert_eq!(capture_identity(&anchor).unwrap(), ids[0]);
    assert!(!contract::anchor::path(&f.root, &tx, 1).unwrap().exists());
    // Review still works: the transaction was not sealed with an anchor linked.
    let report = review(&f.state, RequestedAction::Undo).unwrap();
    assert_eq!(report.transaction.state, TransactionState::RollingBack);
    assert!(
        report
            .transaction
            .entries
            .iter()
            .all(|e| e.state == EntryState::Skipped)
    );
    let again = grant(&f, RequestedAction::Undo);
    let result = execute(&f.state, &again, RequestedAction::Undo, true).unwrap();
    assert!(result.completed, "{:?}", result.transaction);
    assert_eq!(result.transaction.state, TransactionState::RolledBack);
    assert!(!anchor.exists());
    for (name, id) in [("a.bin", &ids[0]), ("c.bin", &ids[1])] {
        assert_eq!(&capture_identity(&f.root.join(name)).unwrap(), id);
    }
    assert!(!f.root.join("b.bin").exists() && !f.root.join("d.bin").exists());
    assert!(review(&f.state, RequestedAction::Undo).is_err()); // verified release
}
