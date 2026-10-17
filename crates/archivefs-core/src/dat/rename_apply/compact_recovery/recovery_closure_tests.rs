//! The five controlled-campaign findings, exercised only with disposable roots.
use super::*;
use serde_json::{Value, json};
use std::process::Command;
const WORKER: &str = "dat::rename_apply::compact_recovery::storage::integration::deployment::tests::deployment_worker";
fn child(
    f: &Fixture,
    approval: &approval::Approval,
    extra: &[(&str, &str)],
) -> (std::process::Output, PathBuf) {
    let output = f._home.path().join("child-output.json");
    let request = f._home.path().join("child-request.json");
    fs::write(
        &request,
        serde_json::to_vec(
            &json!({"state":f.state,"grant":approval,"output":output,"action":"undo"}),
        )
        .unwrap(),
    )
    .unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", WORKER, "--nocapture"])
        .env("EMUWIZ_DEPLOY_REQUEST", request)
        .env("HOME", f._home.path())
        .env("XDG_CONFIG_HOME", f._home.path())
        .env("XDG_DATA_HOME", f._home.path());
    for &(name, value) in extra {
        cmd.env(name, value);
    }
    (cmd.output().unwrap(), output)
}
fn foreign_undo(f: &Fixture) -> Value {
    foreign_undo_at(f, 2, "external-original.bin")
}
fn foreign_undo_at(f: &Fixture, index: usize, saved: &str) -> Value {
    let c = load(&f.state).unwrap();
    let r: Registration =
        serde_json::from_slice(&fs::read(f.state.join("registration.json")).unwrap()).unwrap();
    let external = f._home.path().join("foreign.json");
    fs::write(&external, serde_json::to_vec(&json!({"original":r.original,"authority":c.authority,"fd_action":"foreign-old","fd_action_index":index,"fd_external_original":saved,"fd_action_edge":"BeforeCapture"})).unwrap()).unwrap();
    let g = grant(f, RequestedAction::Undo);
    let (out, path) = child(
        f,
        &g,
        &[("EMUWIZ_CONTRACT_ACTION_REQUEST", external.to_str().unwrap())],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
#[test]
fn closure_fresh_signed_undo_finishes_other_entries_without_touching_foreign_hold() {
    for interrupted in [false, true] {
        let f = fixture();
        fs::write(f.root.join("c.bin"), b"original C").unwrap();
        fs::write(f.root.join("e.bin"), b"original E").unwrap();
        let tx = plan(
            &f.state,
            &[
                ("a.bin".into(), "b.bin".into()),
                ("c.bin".into(), "d.bin".into()),
                ("e.bin".into(), "f.bin".into()),
            ],
        )
        .unwrap();
        let ids: Vec<_> = tx.entries.iter().map(|e| e.identity.clone()).collect();
        let bytes = serde_json::to_vec(&tx).unwrap();
        enroll(&f.state, &bytes, &plan_confirmation(&bytes).unwrap(), true).unwrap();
        assert!(
            execute(
                &f.state,
                &grant(&f, RequestedAction::Apply),
                RequestedAction::Apply,
                true
            )
            .unwrap()
            .completed
        );
        let first = foreign_undo(&f);
        assert_eq!(first["completed"], false);
        assert_eq!(first["transaction"]["entries"][0]["state"], "applied");
        assert_eq!(first["transaction"]["entries"][1]["state"], "applied");
        let before = review(&f.state, RequestedAction::Undo).unwrap();
        let held = contract::paths(&f.root, &before.transaction, 2).unwrap().0;
        let foreign = capture_identity(&held).unwrap();
        assert_eq!(fs::read(&held).unwrap(), b"foreign B");
        let parked = before.transaction.entries[2].clone();
        let resume = grant(&f, RequestedAction::Undo);
        let env = if interrupted {
            vec![
                ("EMUWIZ_CONTRACT_EXIT", "AfterCapture"),
                ("EMUWIZ_CONTRACT_EXIT_ENTRY", "0"),
            ]
        } else {
            vec![]
        };
        let (out, path) = child(&f, &resume, &env);
        if interrupted {
            assert_eq!(out.status.code(), Some(82));
            assert!(execute(&f.state, &resume, RequestedAction::Undo, true).is_err());
            let fresh = grant(&f, RequestedAction::Undo);
            let (out, _) = child(&f, &fresh, &[]);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        } else {
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let done: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(done["completed"], false);
        for (index, name) in [(0, "a.bin"), (1, "c.bin")] {
            assert_eq!(capture_identity(&f.root.join(name)).unwrap(), ids[index]);
        }
        assert!(!f.root.join("b.bin").exists() && !f.root.join("d.bin").exists());
        let after = review(&f.state, RequestedAction::Undo).unwrap();
        assert_eq!(after.transaction.entries[2], parked);
        assert_eq!(capture_identity(&held).unwrap(), foreign);
        assert_eq!(
            capture_identity(&f.root.join("external-original.bin")).unwrap(),
            ids[2]
        );
        let status = status(&f.state);
        assert_eq!(status.state, RestartState::ReviewRequired);
        assert_eq!(status.ownership, "held");
        assert!(
            approve(
                &f.state,
                &f.signer,
                RequestedAction::Undo,
                &after.confirmation
            )
            .is_err(),
            "no further provable entry remains; do not sign a useless grant"
        );
    }
}
#[test]
fn closure_approve_rejects_spent_apply_without_issuing_a_grant() {
    let f = fixture();
    enrolled(&f);
    assert!(
        execute(
            &f.state,
            &grant(&f, RequestedAction::Apply),
            RequestedAction::Apply,
            true
        )
        .unwrap()
        .completed
    );
    let review = review(&f.state, RequestedAction::Apply).unwrap();
    let before = fs::read_dir(f.state.join("grants")).unwrap().count();
    assert!(
        approve(
            &f.state,
            &f.signer,
            RequestedAction::Apply,
            &review.confirmation
        )
        .is_err()
    );
    assert_eq!(
        fs::read_dir(f.state.join("grants")).unwrap().count(),
        before
    );
}
#[test]
fn closure_refused_executions_leave_no_new_probe_directories() {
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    assert!(
        execute(&f.state, &g, RequestedAction::Apply, true)
            .unwrap()
            .completed
    );
    let count = || {
        fs::read_dir(f.root.join(".emuwiz-r2-holding"))
            .unwrap()
            .filter(|p| {
                p.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("probe-")
            })
            .count()
    };
    let before = count();
    for _ in 0..3 {
        assert!(execute(&f.state, &g, RequestedAction::Apply, true).is_err());
    }
    assert_eq!(count(), before);
    assert_eq!(fs::read(f.root.join("b.bin")).unwrap(), b"original A");
}
#[test]
fn closure_cleanup_uncertainty_reports_original_observed_at_original_path() {
    let f = fixture();
    let id = capture_identity(&f.root.join("a.bin")).unwrap();
    enrolled(&f);
    execute(
        &f.state,
        &grant(&f, RequestedAction::Apply),
        RequestedAction::Apply,
        true,
    )
    .unwrap();
    let g = grant(&f, RequestedAction::Undo);
    let (out, _) = child(&f, &g, &[("EMUWIZ_CONTRACT_EXIT", "AfterUnlink")]);
    assert_eq!(out.status.code(), Some(82));
    assert_eq!(capture_identity(&f.root.join("a.bin")).unwrap(), id);
    let g = grant(&f, RequestedAction::Undo);
    let result = execute(&f.state, &g, RequestedAction::Undo, true).unwrap();
    assert!(!result.completed);
    let detail = result.transaction.entries[0]
        .failure_reason
        .as_ref()
        .unwrap();
    assert!(detail.contains("original path"), "{detail}");
    assert!(!detail.contains("may be unrecoverable"), "{detail}");
    let status = status(&f.state);
    assert_eq!(status.state, RestartState::ReviewRequired);
    assert!(
        status.entries[0].verdict.contains("original path"),
        "{:?}",
        status.entries[0]
    );
    assert_eq!(status.ownership, "held");
}

fn assert_ownership_held(f: &Fixture) {
    let c = load(&f.state).unwrap();
    let r: Registration =
        serde_json::from_slice(&fs::read(f.state.join("registration.json")).unwrap()).unwrap();
    let base = Authority::decode_legacy(&r.original, 1024 * 1024).unwrap();
    let (inspection, _lock) = c.authority.inspect(Some(&base)).unwrap();
    assert!(inspection.claims_complete);
    assert!(
        inspection
            .owner
            .as_ref()
            .is_some_and(|(binding, released)| binding == &base.binding().to_vec() && !released)
    );
}
fn three_applied() -> Fixture {
    let f = fixture();
    fs::write(f.root.join("c.bin"), b"original C").unwrap();
    fs::write(f.root.join("e.bin"), b"original E").unwrap();
    let plan = plan(
        &f.state,
        &[
            ("a.bin".into(), "b.bin".into()),
            ("c.bin".into(), "d.bin".into()),
            ("e.bin".into(), "f.bin".into()),
        ],
    )
    .unwrap();
    let bytes = serde_json::to_vec(&plan).unwrap();
    enroll(&f.state, &bytes, &plan_confirmation(&bytes).unwrap(), true).unwrap();
    assert!(
        execute(
            &f.state,
            &grant(&f, RequestedAction::Apply),
            RequestedAction::Apply,
            true
        )
        .unwrap()
        .completed
    );
    foreign_undo(&f);
    f
}
fn retain_child(
    f: &Fixture,
    g: &approval::Approval,
    edge: Option<&str>,
    fault: Option<&str>,
) -> std::process::Output {
    let output = f._home.path().join("retention-output.json");
    let request = f._home.path().join("retention-request.json");
    fs::write(&request,serde_json::to_vec(&json!({"state":f.state,"grant":g,"action":"retain-foreign","output":output,"retention_fault":fault})).unwrap()).unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", WORKER, "--nocapture"])
        .env("EMUWIZ_DEPLOY_REQUEST", request)
        .env("HOME", f._home.path())
        .env("XDG_CONFIG_HOME", f._home.path());
    if let Some(edge) = edge {
        cmd.env("EMUWIZ_CONTRACT_EXIT", edge)
            .env("EMUWIZ_CONTRACT_EXIT_ENTRY", "2");
    }
    cmd.output().unwrap()
}
fn finish_after_retention(f: &Fixture, foreign: &ObjectIdentity, held: &Path) {
    let reviewed = review(&f.state, RequestedAction::Undo).unwrap();
    assert_eq!(
        contract::evidence(&reviewed.transaction, 2)
            .unwrap()
            .unwrap()
            .phase,
        contract::Phase::RetainedForeign
    );
    assert_eq!(status(&f.state).ownership, "held");
    let ordinary = grant(f, RequestedAction::Undo);
    let result = execute(&f.state, &ordinary, RequestedAction::Undo, true).unwrap();
    assert!(result.completed, "{:?}", result.transaction);
    let final_status = status(&f.state);
    assert_eq!(final_status.state, RestartState::Settled);
    assert_eq!(final_status.ownership, "released");
    for (index, name) in [(0, "a.bin"), (1, "c.bin"), (2, "e.bin")] {
        assert_eq!(
            capture_identity(&f.root.join(name)).unwrap(),
            result.transaction.entries[index].identity
        );
    }
    assert_eq!(capture_identity(held).unwrap(), *foreign);
    let retained = final_status.entries[2]
        .retained_foreign
        .as_ref()
        .expect("retained data must remain discoverable AFTER release");
    assert_eq!(retained.path, held);
    assert_eq!(retained.identity, *foreign);
    let copy = f._home.path().join("retrieved-foreign-copy.bin");
    fs::copy(&retained.path, &copy).unwrap();
    assert_eq!(fs::read(&copy).unwrap(), b"foreign B");
    assert_eq!(
        capture_identity(&copy).unwrap().freshness.unwrap().sha256,
        foreign.freshness.as_ref().unwrap().sha256
    );
    assert_ne!(capture_identity(&copy).unwrap().ino, foreign.ino);
    assert_eq!(capture_identity(held).unwrap(), *foreign);
    assert!(
        review(&f.state, RequestedAction::RetainForeign).is_err(),
        "released roots never reused"
    );
}
#[test]
fn closure_separate_retention_authority_settles_foreign_without_disposal_then_signed_undo_releases()
{
    let f = three_applied();
    let r = review(&f.state, RequestedAction::RetainForeign).unwrap();
    assert!(r.actionable);
    assert_eq!(r.eligible_entries, vec![2]);
    let held = contract::paths(&f.root, &r.transaction, 2).unwrap().0;
    let foreign = capture_identity(&held).unwrap();
    let unrelated_apply =
        super::restart_status_tests::unfiltered_fixture_grant(&f, RequestedAction::Apply);
    assert!(
        execute(
            &f.state,
            &unrelated_apply,
            RequestedAction::RetainForeign,
            true
        )
        .unwrap_err()
        .to_string()
        .contains("differs from requested")
    );
    let ordinary = grant(&f, RequestedAction::Undo);
    assert!(
        execute(&f.state, &ordinary, RequestedAction::RetainForeign, true)
            .unwrap_err()
            .to_string()
            .contains("differs from requested")
    );
    assert_eq!(capture_identity(&held).unwrap(), foreign);
    let wrong = grant(&f, RequestedAction::RetainForeign);
    assert!(
        execute(&f.state, &wrong, RequestedAction::Undo, true)
            .unwrap_err()
            .to_string()
            .contains("differs from requested")
    );
    let g = grant(&f, RequestedAction::RetainForeign);
    let mut forged = serde_json::to_value(&g).unwrap();
    let sig = forged["context"]["signed"]["signature"]
        .as_array_mut()
        .unwrap();
    sig[0] = json!(sig[0].as_u64().unwrap() ^ 1);
    let forged: approval::Approval = serde_json::from_value(forged).unwrap();
    assert!(
        execute(&f.state, &forged, RequestedAction::RetainForeign, true)
            .unwrap_err()
            .to_string()
            .contains("signature invalid")
    );
    assert_eq!(capture_identity(&held).unwrap(), foreign);
    let offline = f._home.path().join("offline-private-key");
    fs::rename(&f.signer, &offline).unwrap();
    let out = retain_child(&f, &g, None, None);
    fs::rename(&offline, &f.signer).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        execute(&f.state, &g, RequestedAction::RetainForeign, true).is_err(),
        "retention grant is still one-shot"
    );
    let retained = review(&f.state, RequestedAction::Undo).unwrap();
    assert_eq!(
        retained.transaction.entries[0].state,
        EntryState::Applied,
        "retention authority cannot perform ordinary Undo"
    );
    assert_eq!(retained.transaction.entries[1].state, EntryState::Applied);
    finish_after_retention(&f, &foreign, &held);
}
#[test]
fn closure_retention_fresh_process_at_every_new_mutation_and_receipt_boundary() {
    for edge in [
        "RetentionIntentRecorded",
        "BeforeRetainedPublish",
        "AfterRetainedPublish",
        "BeforeRetentionReceipt",
        "RetentionReceiptRecorded",
    ] {
        let f = three_applied();
        let r = review(&f.state, RequestedAction::RetainForeign).unwrap();
        let held = contract::paths(&f.root, &r.transaction, 2).unwrap().0;
        let foreign = capture_identity(&held).unwrap();
        let g = grant(&f, RequestedAction::RetainForeign);
        let out = retain_child(&f, &g, Some(edge), None);
        assert_eq!(
            out.status.code(),
            Some(82),
            "{edge}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(capture_identity(&held).unwrap(), foreign);
        assert_eq!(status(&f.state).ownership, "held");
        assert!(execute(&f.state, &g, RequestedAction::RetainForeign, true).is_err());
        let r = review(&f.state, RequestedAction::RetainForeign).unwrap();
        if r.actionable {
            let fresh = approve(
                &f.state,
                &f.signer,
                RequestedAction::RetainForeign,
                &r.confirmation,
            )
            .unwrap();
            let out = retain_child(&f, &fresh, None, None);
            assert!(
                out.status.success(),
                "{edge}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        finish_after_retention(&f, &foreign, &held);
    }
}
#[test]
fn closure_retention_receipt_publication_failures_preserve_and_refuse_or_recover_without_guessing()
{
    for fault in ["write", "sync", "commit", "commit-sync", "short", "eio"] {
        let f = three_applied();
        let r = review(&f.state, RequestedAction::RetainForeign).unwrap();
        let held = contract::paths(&f.root, &r.transaction, 2).unwrap().0;
        let foreign = capture_identity(&held).unwrap();
        let g = grant(&f, RequestedAction::RetainForeign);
        let out = retain_child(&f, &g, None, Some(fault));
        if ["short", "eio"].contains(&fault) {
            assert!(out.status.success(), "{fault}");
        } else {
            assert_eq!(
                out.status.code(),
                Some(77),
                "{fault}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        assert_eq!(capture_identity(&held).unwrap(), foreign);
        assert_ownership_held(&f);
        assert_ne!(status(&f.state).state, RestartState::Settled);
        match review(&f.state, RequestedAction::RetainForeign) {
            Ok(r) if r.actionable => {
                let fresh = approve(
                    &f.state,
                    &f.signer,
                    RequestedAction::RetainForeign,
                    &r.confirmation,
                )
                .unwrap();
                let out = retain_child(&f, &fresh, None, None);
                assert!(
                    out.status.success(),
                    "{fault}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                finish_after_retention(&f, &foreign, &held);
            }
            Ok(_) => finish_after_retention(&f, &foreign, &held),
            Err(_) => {
                assert_eq!(status(&f.state).state, RestartState::ReviewRequired);
                assert!(
                    f.root.join("e.bin").exists(),
                    "{fault}: original publication must survive refused receipt repair"
                );
                assert!(f.root.join("b.bin").exists() && f.root.join("d.bin").exists());
            }
        }
    }
}
#[test]
fn closure_retention_refuses_changed_foreign_anchor_destination_and_signed_receipts() {
    for changed in ["foreign", "anchor", "destination", "source", "receipt"] {
        let f = three_applied();
        let r = review(&f.state, RequestedAction::RetainForeign).unwrap();
        let held = contract::paths(&f.root, &r.transaction, 2).unwrap().0;
        let g = grant(&f, RequestedAction::RetainForeign);
        match changed {
            "foreign" => fs::write(&held, b"changed foreign").unwrap(),
            "anchor" => {
                fs::rename(
                    contract::anchor::path(&f.root, &r.transaction, 2).unwrap(),
                    f._home.path().join("kept-anchor"),
                )
                .unwrap();
            }
            "destination" => fs::write(f.root.join("f.bin"), b"unrelated destination").unwrap(),
            "source" => fs::write(f.root.join("e.bin"), b"unrelated source").unwrap(),
            _ => {
                let out = retain_child(&f, &g, None, None);
                assert!(out.status.success());
                let retained = review(&f.state, RequestedAction::Undo).unwrap();
                let nonce = &contract::evidence(&retained.transaction, 2)
                    .unwrap()
                    .unwrap()
                    .retention
                    .unwrap()
                    .nonce;
                fs::write(
                    f.state.join("grants").join(format!("used-{nonce}")),
                    b"torn receipt authority",
                )
                .unwrap();
                assert!(review(&f.state, RequestedAction::Undo).is_err());
                assert_eq!(status(&f.state).state, RestartState::ReviewRequired);
                assert_eq!(status(&f.state).ownership, "held");
                continue;
            }
        }
        let bytes = fs::read(&held).unwrap();
        let anchor = fs::read(f.root.join("external-original.bin")).unwrap();
        assert!(
            execute(&f.state, &g, RequestedAction::RetainForeign, true).is_err(),
            "{changed}"
        );
        assert_eq!(fs::read(&held).unwrap(), bytes);
        assert_eq!(
            fs::read(f.root.join("external-original.bin")).unwrap(),
            anchor
        );
        assert_eq!(status(&f.state).ownership, "held");
        assert_eq!(status(&f.state).state, RestartState::ReviewRequired);
        if changed == "source" {
            assert_eq!(fs::read(f.root.join("e.bin")).unwrap(), b"unrelated source");
        }
        if changed == "destination" {
            assert_eq!(
                fs::read(f.root.join("f.bin")).unwrap(),
                b"unrelated destination"
            );
        }
        if changed == "anchor" {
            assert_eq!(
                fs::read(f._home.path().join("kept-anchor")).unwrap(),
                b"original E"
            );
        }
    }
}

#[test]
fn closure_eight_entries_two_retained_foreign_objects_resume_between_receipts() {
    let f = fixture();
    let mut pairs = vec![("a.bin".to_owned(), "b.bin".to_owned())];
    for i in 1..8 {
        let source = format!("source-{i}.bin");
        fs::write(f.root.join(&source), format!("original {i}")).unwrap();
        pairs.push((source, format!("destination-{i}.bin")));
    }
    let plan = plan(&f.state, &pairs).unwrap();
    let ids = plan
        .entries
        .iter()
        .map(|e| e.identity.clone())
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&plan).unwrap();
    enroll(&f.state, &bytes, &plan_confirmation(&bytes).unwrap(), true).unwrap();
    assert!(
        execute(
            &f.state,
            &grant(&f, RequestedAction::Apply),
            RequestedAction::Apply,
            true
        )
        .unwrap()
        .completed
    );
    foreign_undo_at(&f, 7, "external-original-7.bin");
    foreign_undo_at(&f, 6, "external-original-6.bin");
    let reviewed = review(&f.state, RequestedAction::RetainForeign).unwrap();
    assert_eq!(reviewed.eligible_entries, vec![6, 7]);
    let held = [6, 7].map(|i| {
        contract::paths(&f.root, &reviewed.transaction, i)
            .unwrap()
            .0
    });
    let foreign = held.each_ref().map(|p| capture_identity(p).unwrap());
    assert_ne!(foreign[0].ino, foreign[1].ino);
    let g = grant(&f, RequestedAction::RetainForeign);
    let output = f._home.path().join("multi-output.json");
    let request = f._home.path().join("multi-request.json");
    fs::write(
        &request,
        serde_json::to_vec(
            &json!({"state":f.state,"grant":g,"action":"retain-foreign","output":output}),
        )
        .unwrap(),
    )
    .unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", WORKER])
        .env("EMUWIZ_DEPLOY_REQUEST", request)
        .env("HOME", f._home.path())
        .env("XDG_CONFIG_HOME", f._home.path())
        .env("XDG_DATA_HOME", f._home.path())
        .env("EMUWIZ_CONTRACT_EXIT", "RetentionReceiptRecorded")
        .env("EMUWIZ_CONTRACT_EXIT_ENTRY", "6")
        .output()
        .unwrap();
    assert_eq!(child.status.code(), Some(82));
    assert_ownership_held(&f);
    let reviewed = review(&f.state, RequestedAction::RetainForeign).unwrap();
    assert_eq!(reviewed.eligible_entries, vec![7]);
    let fresh = approve(
        &f.state,
        &f.signer,
        RequestedAction::RetainForeign,
        &reviewed.confirmation,
    )
    .unwrap();
    let out = retain_child(&f, &fresh, None, None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let undo = grant(&f, RequestedAction::Undo);
    assert!(
        execute(&f.state, &undo, RequestedAction::Undo, true)
            .unwrap()
            .completed
    );
    for (i, (name, _)) in pairs.iter().enumerate() {
        assert_eq!(capture_identity(&f.root.join(name)).unwrap(), ids[i]);
    }
    let status = status(&f.state);
    assert_eq!(status.state, RestartState::Settled);
    assert_eq!(status.ownership, "released");
    for (n, i) in [6, 7].into_iter().enumerate() {
        assert_eq!(capture_identity(&held[n]).unwrap(), foreign[n]);
        assert_eq!(
            status.entries[i]
                .retained_foreign
                .as_ref()
                .unwrap()
                .identity,
            foreign[n]
        );
    }
    assert_eq!(
        status.entries.len(),
        8,
        "hardlink anchors and foreign objects are not independent completed files"
    );
}
