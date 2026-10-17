//! Deterministic restart-continuity fault injection. Real child processes die
//! (`_exit`) at named boundaries of grant consumption, admission, checkpoint
//! publication, recovery and settlement; afterwards only the read-only status
//! and grant inspection run, and each call is checked to leave every byte of
//! evidence, authority and library state unchanged.
use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::fs::MetadataExt;
use std::process::{Command, Output};

const WORKER: &str = "dat::rename_apply::compact_recovery::storage::integration::deployment::tests::deployment_worker";

fn child(f: &Fixture, mut req: serde_json::Value, env: &[(&str, &str)]) -> Output {
    req["state"] = serde_json::json!(f.state);
    let path = f._home.path().join(format!(
        "req-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&path, serde_json::to_vec(&req).unwrap()).unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", WORKER, "--nocapture"])
        .env("EMUWIZ_DEPLOY_REQUEST", &path)
        .env_remove("EMUWIZ_R2_EXIT")
        .env_remove("EMUWIZ_CONTRACT_EXIT")
        .env_remove("EMUWIZ_CONTRACT_EXIT_ENTRY");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}
/// Executes a grant in a child process that dies at `env`'s boundary.
fn crash(f: &Fixture, action: &str, g: &approval::Approval, env: &[(&str, &str)], code: i32) {
    let out = child(
        f,
        serde_json::json!({"grant": g, "action": action, "output": f._home.path().join("unused.json")}),
        env,
    );
    assert_eq!(
        out.status.code(),
        Some(code),
        "{env:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn run(f: &Fixture, action: RequestedAction, g: &approval::Approval) -> io::Result<Execution> {
    execute(&f.state, g, action, true)
}
/// Hash of every byte the restart path could touch: library + holding +
/// compact store, deployment configuration, registration, admission database
/// and every grant and tombstone. Includes inode numbers and link counts.
/// Like [`evidence`] but without the capability-probe artifacts that
/// `execute` writes into private holding before it can refuse (pre-existing,
/// documented behaviour; never library or authority state).
fn durable(f: &Fixture) -> String {
    evidence(f)
        .lines()
        .filter(|l| !l.contains("/.emuwiz-r2-holding/probe-"))
        .collect::<Vec<_>>()
        .join("\n")
}
fn evidence(f: &Fixture) -> String {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = match fs::read_dir(dir) {
            Ok(e) => e.map(|e| e.unwrap().path()).collect(),
            Err(_) => return,
        };
        entries.sort();
        for p in entries {
            let m = fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                out.push(format!("D {} {:o}", p.display(), m.mode()));
                walk(&p, out);
            } else {
                let bytes = fs::read(&p).unwrap_or_default();
                out.push(format!(
                    "F {} ino={} nlink={} {}",
                    p.display(),
                    m.ino(),
                    m.nlink(),
                    hex(&Sha256::digest(&bytes))
                ));
            }
        }
    }
    let mut out = vec![];
    walk(&f.root, &mut out);
    walk(&f.state, &mut out);
    out.join("\n")
}
/// Status, asserted read-only and stable across three restarts.
fn st(f: &Fixture) -> RestartStatus {
    let before = evidence(f);
    let first = status(&f.state);
    for _ in 0..2 {
        let again = status(&f.state);
        assert_eq!(
            serde_json::to_value(&again).unwrap(),
            serde_json::to_value(&first).unwrap(),
            "repeated restart changed the interpretation"
        );
    }
    assert_eq!(evidence(f), before, "status mutated evidence");
    first
}
fn inspect(f: &Fixture, action: RequestedAction, g: &approval::Approval) -> GrantStatus {
    let before = evidence(f);
    let result = inspect_grant(&f.state, g, action);
    assert_eq!(evidence(f), before, "grant inspection mutated evidence");
    result
}
fn reasons(s: &RestartStatus) -> String {
    s.reasons.join(" | ")
}
fn expect(s: &RestartStatus, state: RestartState, next: NextStep) {
    assert_eq!((s.state, s.next), (state, next), "{}", reasons(s));
}
fn applied(f: &Fixture) {
    let g = grant(f, RequestedAction::Apply);
    assert!(run(f, RequestedAction::Apply, &g).unwrap().completed);
}
fn original(f: &Fixture) {
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
    assert!(!f.root.join("b.bin").exists());
}

#[test]
fn lifecycle_states_are_recognised_read_only_and_grants_never_renew() {
    let f = fixture();
    expect(&st(&f), RestartState::NotEnrolled, NextStep::None);
    enrolled(&f);
    expect(&st(&f), RestartState::AwaitingApply, NextStep::ReviewApply);
    let apply = grant(&f, RequestedAction::Apply);
    let r = inspect(&f, RequestedAction::Apply, &apply);
    assert_eq!(r.verdict, GrantVerdict::Usable, "{}", r.reason);
    assert_eq!(
        inspect(&f, RequestedAction::Undo, &apply).verdict,
        GrantVerdict::WrongAction
    );
    assert!(run(&f, RequestedAction::Apply, &apply).unwrap().completed);
    let s = st(&f);
    expect(&s, RestartState::Applied, NextStep::ReviewUndo);
    assert_eq!(s.ownership, "held");
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &apply).verdict,
        GrantVerdict::Consumed
    );
    // Replay of the consumed grant: refused, nothing changes.
    let before = durable(&f);
    let replay = run(&f, RequestedAction::Apply, &apply)
        .unwrap_err()
        .to_string();
    assert!(
        replay.contains("already consumed") || replay.contains("evidence differs"),
        "{replay}"
    );
    assert_eq!(durable(&f), before);
    // Two Undo grants for the same reviewed state; only one can ever act.
    let first = grant(&f, RequestedAction::Undo);
    let spare = grant(&f, RequestedAction::Undo);
    assert_eq!(
        inspect(&f, RequestedAction::Undo, &spare).verdict,
        GrantVerdict::Usable
    );
    assert!(run(&f, RequestedAction::Undo, &first).unwrap().completed);
    original(&f);
    let s = st(&f);
    expect(&s, RestartState::Settled, NextStep::None);
    assert_eq!(s.ownership, "released");
    let late = inspect(&f, RequestedAction::Undo, &spare);
    // Settlement changed the evidence it was bound to: stale, never usable.
    assert_eq!(late.verdict, GrantVerdict::EvidenceStale, "{}", late.reason);
    assert_eq!(late.status.state, RestartState::Settled);
    let before = durable(&f);
    assert!(run(&f, RequestedAction::Undo, &spare).is_err());
    assert_eq!(durable(&f), before, "a settled transaction was touched");
    assert_eq!(
        inspect(&f, RequestedAction::Undo, &first).verdict,
        GrantVerdict::Consumed
    );
}

#[test]
fn enrollment_crashes_are_review_required_and_never_adopted() {
    for edge in [
        "after-admission",
        "before-anchor-publication",
        "after-anchor-publication",
    ] {
        let f = fixture();
        let tx = plan(&f.state, &[("a.bin".into(), "b.bin".into())]).unwrap();
        let plan_path = f._home.path().join("plan.json");
        fs::write(&plan_path, serde_json::to_vec(&tx).unwrap()).unwrap();
        let out = child(
            &f,
            serde_json::json!({"mode":"enroll","plan":plan_path}),
            &[("EMUWIZ_R2_EXIT", edge)],
        );
        assert_eq!(out.status.code(), Some(79), "{edge}");
        let s = st(&f);
        expect(&s, RestartState::ReviewRequired, NextStep::OperatorReview);
        assert!(
            reasons(&s).contains("enrollment did not complete"),
            "{edge}: {}",
            reasons(&s)
        );
        assert!(
            reasons(&s).contains("unreleased ownership"),
            "{edge}: {}",
            reasons(&s)
        );
        original(&f);
        // A second enrollment never adopts or overwrites the partial evidence.
        let bytes = serde_json::to_vec(&tx).unwrap();
        let before = durable(&f);
        assert!(enroll(&f.state, &bytes, &plan_confirmation(&bytes).unwrap(), true).is_err());
        assert_eq!(durable(&f), before, "{edge}");
    }
}

#[test]
fn grant_consumption_boundaries_never_renew_a_spent_grant() {
    // Death BEFORE consumption: the grant is untouched and still usable.
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    crash(
        &f,
        "apply",
        &g,
        &[("EMUWIZ_R2_EXIT", "before-consumption")],
        79,
    );
    expect(&st(&f), RestartState::AwaitingApply, NextStep::ReviewApply);
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &g).verdict,
        GrantVerdict::Usable
    );
    assert!(run(&f, RequestedAction::Apply, &g).unwrap().completed);
    // Death AFTER consumption, before any mutation: spent forever; nothing moved.
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    crash(
        &f,
        "apply",
        &g,
        &[("EMUWIZ_R2_EXIT", "after-consumption")],
        79,
    );
    expect(&st(&f), RestartState::AwaitingApply, NextStep::ReviewApply);
    original(&f);
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &g).verdict,
        GrantVerdict::Consumed
    );
    let before = durable(&f);
    assert!(
        run(&f, RequestedAction::Apply, &g)
            .unwrap_err()
            .to_string()
            .contains("already consumed")
    );
    assert_eq!(durable(&f), before);
    let fresh = grant(&f, RequestedAction::Apply);
    assert!(run(&f, RequestedAction::Apply, &fresh).unwrap().completed);
    expect(&st(&f), RestartState::Applied, NextStep::ReviewUndo);
}

#[test]
fn interrupted_apply_restarts_into_fresh_undo_only_and_keeps_review_states() {
    // Recoverable: the original is in trusted holding, nothing published.
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    crash(
        &f,
        "apply",
        &g,
        &[("EMUWIZ_CONTRACT_EXIT", "AfterCapture")],
        82,
    );
    let s = st(&f);
    expect(&s, RestartState::ApplyIncomplete, NextStep::ReviewUndo);
    assert!(
        s.entries[0].verdict.contains("interrupted Apply"),
        "{:?}",
        s.entries
    );
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &g).verdict,
        GrantVerdict::Consumed
    );
    let again = grant(&f, RequestedAction::Apply);
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &again).verdict,
        GrantVerdict::NotApplicable
    );
    let before = durable(&f);
    assert!(
        run(&f, RequestedAction::Apply, &again)
            .unwrap_err()
            .to_string()
            .contains("spent transaction")
    );
    assert!(
        evidence(&f) != before,
        "the refused grant must still be burned"
    );
    expect(&st(&f), RestartState::ApplyIncomplete, NextStep::ReviewUndo);
    let undo = grant(&f, RequestedAction::Undo);
    assert_eq!(
        inspect(&f, RequestedAction::Undo, &undo).verdict,
        GrantVerdict::Usable
    );
    assert!(run(&f, RequestedAction::Undo, &undo).unwrap().completed);
    original(&f);
    expect(&st(&f), RestartState::Settled, NextStep::None);

    // Link made but never recorded: still REVIEW_REQUIRED after recovery.
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    crash(
        &f,
        "apply",
        &g,
        &[("EMUWIZ_CONTRACT_EXIT", "AfterPublish")],
        82,
    );
    expect(&st(&f), RestartState::ApplyIncomplete, NextStep::ReviewUndo);
    let undo = grant(&f, RequestedAction::Undo);
    assert!(!run(&f, RequestedAction::Undo, &undo).unwrap().completed);
    let s = st(&f);
    expect(&s, RestartState::ReviewRequired, NextStep::OperatorReview);
    assert!(
        reasons(&s).contains("Apply/DestinationOccupied"),
        "{}",
        reasons(&s)
    );
    let undo = grant(&f, RequestedAction::Undo);
    assert_eq!(
        inspect(&f, RequestedAction::Undo, &undo).verdict,
        GrantVerdict::NotApplicable
    );
    assert!(run(&f, RequestedAction::Undo, &undo).is_err());
    expect(
        &st(&f),
        RestartState::ReviewRequired,
        NextStep::OperatorReview,
    );

    // Holding link removed but completion never recorded: REVIEW_REQUIRED too.
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    crash(
        &f,
        "apply",
        &g,
        &[("EMUWIZ_CONTRACT_EXIT", "AfterUnlink")],
        82,
    );
    let undo = grant(&f, RequestedAction::Undo);
    assert!(!run(&f, RequestedAction::Undo, &undo).unwrap().completed);
    let s = st(&f);
    expect(&s, RestartState::ReviewRequired, NextStep::OperatorReview);
    assert!(reasons(&s).contains("Apply/Incomplete"), "{}", reasons(&s));
}

#[test]
fn interrupted_undo_and_partial_recovery_survive_repeated_restart() {
    // Plain Undo interruption.
    let f = fixture();
    enrolled(&f);
    applied(&f);
    let g = grant(&f, RequestedAction::Undo);
    crash(
        &f,
        "undo",
        &g,
        &[("EMUWIZ_CONTRACT_EXIT", "CapturedRecorded")],
        82,
    );
    expect(&st(&f), RestartState::UndoIncomplete, NextStep::ReviewUndo);
    assert_eq!(
        inspect(&f, RequestedAction::Undo, &g).verdict,
        GrantVerdict::Consumed
    );
    let fresh = grant(&f, RequestedAction::Undo);
    assert!(run(&f, RequestedAction::Undo, &fresh).unwrap().completed);
    original(&f);
    expect(&st(&f), RestartState::Settled, NextStep::None);
    // Interrupted Apply, then the recovering Undo is interrupted too.
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    crash(
        &f,
        "apply",
        &g,
        &[("EMUWIZ_CONTRACT_EXIT", "AfterCapture")],
        82,
    );
    let g = grant(&f, RequestedAction::Undo);
    crash(
        &f,
        "undo",
        &g,
        &[("EMUWIZ_CONTRACT_EXIT", "AdoptedRecorded")],
        82,
    );
    expect(&st(&f), RestartState::UndoIncomplete, NextStep::ReviewUndo);
    let fresh = grant(&f, RequestedAction::Undo);
    assert!(run(&f, RequestedAction::Undo, &fresh).unwrap().completed);
    original(&f);
    expect(&st(&f), RestartState::Settled, NextStep::None);
    // A grant issued before the interruption is superseded by the new evidence.
    let f = fixture();
    enrolled(&f);
    applied(&f);
    let early = grant(&f, RequestedAction::Undo);
    let g = grant(&f, RequestedAction::Undo);
    crash(
        &f,
        "undo",
        &g,
        &[("EMUWIZ_CONTRACT_EXIT", "CapturedRecorded")],
        82,
    );
    let r = inspect(&f, RequestedAction::Undo, &early);
    assert_eq!(r.verdict, GrantVerdict::EvidenceStale, "{}", r.reason);
    assert!(run(&f, RequestedAction::Undo, &early).is_err());
    expect(&st(&f), RestartState::UndoIncomplete, NextStep::ReviewUndo);
}

#[test]
fn settlement_and_release_boundaries() {
    let f = fixture();
    enrolled(&f);
    applied(&f);
    let g = grant(&f, RequestedAction::Undo);
    crash(&f, "undo", &g, &[("EMUWIZ_R2_EXIT", "before-release")], 79);
    original(&f);
    let s = st(&f);
    expect(
        &s,
        RestartState::SettledAwaitingRelease,
        NextStep::ReviewUndo,
    );
    assert_eq!(s.ownership, "held");
    let fresh = grant(&f, RequestedAction::Undo);
    assert_eq!(
        inspect(&f, RequestedAction::Undo, &fresh).verdict,
        GrantVerdict::Usable
    );
    assert!(run(&f, RequestedAction::Undo, &fresh).unwrap().completed);
    expect(&st(&f), RestartState::Settled, NextStep::None);

    let f = fixture();
    enrolled(&f);
    applied(&f);
    let g = grant(&f, RequestedAction::Undo);
    crash(&f, "undo", &g, &[("EMUWIZ_R2_EXIT", "after-release")], 79);
    original(&f);
    expect(&st(&f), RestartState::Settled, NextStep::None);
}

#[test]
fn competing_live_process_is_refused_and_no_grant_is_burned() {
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    let ready = f._home.path().join("ready");
    let release = f._home.path().join("release");
    let req = serde_json::json!({"mode":"hold","state":f.state,"ready":ready,"release":release});
    let path = f._home.path().join("hold.json");
    fs::write(&path, serde_json::to_vec(&req).unwrap()).unwrap();
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", WORKER, "--nocapture"])
        .env("EMUWIZ_DEPLOY_REQUEST", &path)
        .spawn()
        .unwrap();
    for _ in 0..600 {
        if ready.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(ready.exists(), "holder did not start");
    let s = status(&f.state);
    expect(&s, RestartState::Busy, NextStep::RetryWhenIdle);
    let r = inspect_grant(&f.state, &g, RequestedAction::Apply);
    assert_eq!(r.verdict, GrantVerdict::NotApplicable);
    assert_eq!(r.status.state, RestartState::Busy);
    let e = run(&f, RequestedAction::Apply, &g).unwrap_err().to_string();
    assert!(e.contains("busy") || e.contains("already owned"), "{e}");
    fs::write(&release, b"go").unwrap();
    assert!(holder.wait().unwrap().success());
    // The refused attempt never reached consumption: the grant still acts.
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &g).verdict,
        GrantVerdict::Usable
    );
    assert!(run(&f, RequestedAction::Apply, &g).unwrap().completed);
}

#[test]
fn missing_corrupt_rebound_or_contradicted_evidence_is_review_required() {
    let setup = || {
        let f = fixture();
        enrolled(&f);
        applied(&f);
        f
    };
    let review = |f: &Fixture, needle: &str| {
        let s = st(f);
        expect(&s, RestartState::ReviewRequired, NextStep::OperatorReview);
        assert!(
            reasons(&s).contains(needle),
            "wanted {needle}: {}",
            reasons(&s)
        );
    };
    // Registration removed (retained aside, never recreated).
    let f = setup();
    fs::rename(
        f.state.join("registration.json"),
        f._home.path().join("registration.retained"),
    )
    .unwrap();
    review(&f, "without a registration");
    // Registration corrupt.
    let f = setup();
    let path = f.state.join("registration.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.truncate(bytes.len() / 2);
    fs::write(&path, bytes).unwrap();
    review(&f, "registration record corrupt");
    // Compact evidence corrupt in place.
    let f = setup();
    let container = f.root.join(NAMESPACE).join(LEAF);
    let mut image = fs::read(&container).unwrap();
    let base_end = image.iter().rposition(|b| *b != 0).unwrap();
    image[base_end] ^= 0x01;
    fs::write(&container, image).unwrap();
    review(&f, "compact evidence");
    // Admission ownership row missing.
    let f = setup();
    let c = load(&f.state).unwrap();
    let db = rusqlite::Connection::open(c.state.join("admission/owners.sqlite")).unwrap();
    db.execute("DELETE FROM owners", []).unwrap();
    drop(db);
    review(&f, "no admission ownership record");
    // Deployment configuration corrupt.
    let f = setup();
    fs::write(f.state.join("configuration.json"), b"{").unwrap();
    review(&f, "deployment configuration");
    // Collection root replaced by a different directory at the same path.
    let f = setup();
    fs::rename(&f.root, f._home.path().join("root-retained")).unwrap();
    fs::create_dir(&f.root).unwrap();
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o700)).unwrap();
    review(&f, "deployment configuration");
    // Library file substituted after Apply: the journal claim is refused.
    let f = setup();
    fs::rename(
        f.root.join("b.bin"),
        f._home.path().join("published-retained"),
    )
    .unwrap();
    fs::write(f.root.join("b.bin"), b"foreign B").unwrap();
    review(&f, "contradicts current files");
}

#[test]
fn older_authentic_snapshots_are_caught_where_evidence_allows_and_limits_are_explicit() {
    // (a) Compact store rewound to an authentic earlier image: the claim that
    // nothing moved contradicts the files and is refused.
    let f = fixture();
    enrolled(&f);
    let container = f.root.join(NAMESPACE).join(LEAF);
    let planned = fs::read(&container).unwrap();
    applied(&f);
    fs::write(&container, &planned).unwrap();
    let s = st(&f);
    expect(&s, RestartState::ReviewRequired, NextStep::OperatorReview);
    assert!(reasons(&s).contains("never moved"), "{}", reasons(&s));
    // (b) Admission store restored to its pre-release snapshot after settlement:
    // the files and sealed record still prove settlement; only release repeats.
    let f = fixture();
    enrolled(&f);
    applied(&f);
    let c = load(&f.state).unwrap();
    let db_path = c.state.join("admission/owners.sqlite");
    let held = fs::read(&db_path).unwrap();
    let g = grant(&f, RequestedAction::Undo);
    assert!(run(&f, RequestedAction::Undo, &g).unwrap().completed);
    expect(&st(&f), RestartState::Settled, NextStep::None);
    fs::write(&db_path, &held).unwrap();
    expect(
        &st(&f),
        RestartState::SettledAwaitingRelease,
        NextStep::ReviewUndo,
    );
    let g = grant(&f, RequestedAction::Undo);
    assert!(run(&f, RequestedAction::Undo, &g).unwrap().completed);
    original(&f);
    expect(&st(&f), RestartState::Settled, NextStep::None);
    // (c) Admission store restored to before enrollment: refused.
    let f = fixture();
    let c = load(&f.state).unwrap();
    let db_path = c.state.join("admission/owners.sqlite");
    let empty = fs::read(&db_path).unwrap();
    enrolled(&f);
    applied(&f);
    fs::write(&db_path, &empty).unwrap();
    let s = st(&f);
    expect(&s, RestartState::ReviewRequired, NextStep::OperatorReview);
    assert!(
        reasons(&s).contains("no admission ownership record"),
        "{}",
        reasons(&s)
    );
    // (d) NOT PREVENTED: a grant consumed without any progress, followed by a
    // restore of the whole grant directory, reads as usable again. Local
    // evidence cannot tell an authentic older authority snapshot from the
    // present; this needs an independent monotonic witness (open blocker).
    let f = fixture();
    enrolled(&f);
    let g = grant(&f, RequestedAction::Apply);
    let grants = f.state.join("grants");
    let mut saved = vec![];
    for e in fs::read_dir(&grants).unwrap() {
        let p = e.unwrap().path();
        saved.push((p.clone(), fs::read(&p).unwrap()));
    }
    crash(
        &f,
        "apply",
        &g,
        &[("EMUWIZ_R2_EXIT", "after-consumption")],
        79,
    );
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &g).verdict,
        GrantVerdict::Consumed
    );
    for e in fs::read_dir(&grants).unwrap() {
        let p = e.unwrap().path();
        if !saved.iter().any(|(s, _)| s == &p) {
            fs::rename(&p, f._home.path().join("tombstone-retained")).unwrap();
        }
    }
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &g).verdict,
        GrantVerdict::Usable
    );
    println!(
        "RESTORED GRANT STORE: NOT PREVENTED; a full authority-store rollback reactivates a grant that made no progress"
    );
}

#[test]
fn unused_unchanged_grant_has_no_time_limit_and_restart_cannot_resurrect_consumed() {
    // Present contract, recorded as a production-security limitation: grants
    // are one-shot and evidence-bound, not time-limited. An unused grant over
    // unchanged evidence stays usable across any number of restarts.
    let f = fixture();
    enrolled(&f);
    let unused = grant(&f, RequestedAction::Apply);
    let consumed = grant(&f, RequestedAction::Apply);
    crash(
        &f,
        "apply",
        &consumed,
        &[("EMUWIZ_R2_EXIT", "after-consumption")],
        79,
    );
    for restart in 0..5 {
        // Every restart re-reads the same evidence; nothing is renewed.
        expect(&st(&f), RestartState::AwaitingApply, NextStep::ReviewApply);
        let r = inspect(&f, RequestedAction::Apply, &unused);
        assert_eq!(
            r.verdict,
            GrantVerdict::Usable,
            "restart {restart}: {}",
            r.reason
        );
        assert_eq!(
            inspect(&f, RequestedAction::Apply, &consumed).verdict,
            GrantVerdict::Consumed,
            "restart {restart} resurrected a consumed grant"
        );
    }
    // The consumed grant cannot act after restart; it is burned again only as a refusal.
    let before = durable(&f);
    assert!(
        run(&f, RequestedAction::Apply, &consumed)
            .unwrap_err()
            .to_string()
            .contains("already consumed")
    );
    assert_eq!(durable(&f), before);
    // A forged copy of a consumed grant with another nonce is not an issued grant.
    let mut forged = consumed.clone();
    forged.nonce = "f".repeat(64);
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &forged).verdict,
        GrantVerdict::ForeignOrInvalid
    );
    assert!(run(&f, RequestedAction::Apply, &forged).is_err());
    original(&f);
    // The unused grant still binds evidence: one Apply makes it evidence-stale
    // for Undo use and consumed-or-stale for anything else.
    assert!(run(&f, RequestedAction::Apply, &unused).unwrap().completed);
    assert_eq!(
        inspect(&f, RequestedAction::Apply, &unused).verdict,
        GrantVerdict::Consumed
    );
}

#[test]
fn uncommitted_checkpoint_is_repaired_only_by_a_fresh_grant_and_torn_ones_refuse() {
    // COMMITTED marker of a record (encoded payload bytes are 1..=16, so the
    // last 0xa5 byte in the container is the newest record's commit marker).
    let newest_marker = |image: &[u8]| image.iter().rposition(|b| *b == 0xa5).unwrap();
    // Fully written, uncommitted: a death between write+sync and commit.
    let f = fixture();
    enrolled(&f);
    applied(&f);
    let container = f.root.join(NAMESPACE).join(LEAF);
    let mut image = fs::read(&container).unwrap();
    let marker = newest_marker(&image);
    image[marker] = 0;
    fs::write(&container, &image).unwrap();
    let s = st(&f);
    // The newest record is either the Apply completion or, when Apply's two
    // timestamp writes straddle a second, a timestamp-only refresh after it;
    // the committed prefix is then apply_incomplete or applied respectively.
    assert!(
        matches!(
            s.state,
            RestartState::ApplyIncomplete | RestartState::Applied
        ) && s.next == NextStep::ReviewUndo,
        "{:?}: {}",
        s.state,
        reasons(&s)
    );
    assert!(s.checkpoint_pending, "{}", reasons(&s));
    assert!(
        reasons(&s).contains("uncommitted checkpoint"),
        "{}",
        reasons(&s)
    );
    assert_eq!(
        fs::read(&container).unwrap(),
        image,
        "status must not commit it"
    );
    let g = grant(&f, RequestedAction::Undo);
    assert_eq!(
        inspect(&f, RequestedAction::Undo, &g).verdict,
        GrantVerdict::Usable
    );
    assert!(run(&f, RequestedAction::Undo, &g).unwrap().completed);
    original(&f);
    expect(&st(&f), RestartState::Settled, NextStep::None);
    // Torn: the newest record lost its tail before commit. Refused, retained.
    let f = fixture();
    enrolled(&f);
    applied(&f);
    let mut image = fs::read(container_of(&f)).unwrap();
    let marker = newest_marker(&image);
    image[marker] = 0;
    for b in &mut image[marker - 40..marker] {
        *b = 0;
    }
    fs::write(container_of(&f), &image).unwrap();
    let s = st(&f);
    expect(&s, RestartState::ReviewRequired, NextStep::OperatorReview);
    assert!(reasons(&s).contains("torn checkpoint"), "{}", reasons(&s));
    assert_eq!(fs::read(container_of(&f)).unwrap(), image);
    // A fresh grant can still be issued from the committed prefix, but
    // execution refuses the torn record after burning the grant: no file
    // moves and the container is retained byte for byte.
    let g = grant(&f, RequestedAction::Undo);
    let e = run(&f, RequestedAction::Undo, &g).unwrap_err().to_string();
    assert!(e.contains("truncated checkpoint"), "{e}");
    assert_eq!(fs::read(container_of(&f)).unwrap(), image);
    assert_eq!(fs::read(f.root.join("b.bin")).unwrap(), b"original A");
    assert!(!f.root.join("a.bin").exists());
    expect(
        &st(&f),
        RestartState::ReviewRequired,
        NextStep::OperatorReview,
    );
}
fn container_of(f: &Fixture) -> std::path::PathBuf {
    f.root.join(NAMESPACE).join(LEAF)
}
