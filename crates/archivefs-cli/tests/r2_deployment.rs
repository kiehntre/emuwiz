#![cfg(target_os = "linux")]
use serde_json::Value;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output};
struct Fixture {
    home: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
    signer: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let home = tempfile::Builder::new()
            .prefix("emuwiz-r2-cli-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = home.path().join("library");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Self {
            root,
            state: home.path().join("state"),
            signer: home.path().join("signer"),
            home,
        };
        f.ok(&["issuer-init", f.signer.to_str().unwrap()]);
        f.ok(&[
            "provision",
            f.root.to_str().unwrap(),
            f.state.to_str().unwrap(),
            f.signer.join("public.json").to_str().unwrap(),
            "test operator",
            "--accept-contract-1-i",
        ]);
        fs::write(f.root.join("a.bin"), b"original A").unwrap();
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        // An empty private process table: the writer gate must not see the
        // CLIs that parallel tests (or other users of this machine) are
        // running under other homes as older, unregistered writers.
        let no_processes = self.home.path().join("no-processes");
        fs::create_dir_all(&no_processes).unwrap();
        Command::new(env!("CARGO_BIN_EXE_emuwiz-r2"))
            .args(args)
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path())
            .env(
                archivefs_core::dat::rename_apply::writer_gate::PROC_ROOT_ENV,
                &no_processes,
            )
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn enroll(&self) {
        let file = self.home.path().join("plan.json");
        let p = self.ok(&[
            "plan",
            self.state.to_str().unwrap(),
            file.to_str().unwrap(),
            "a.bin",
            "b.bin",
        ]);
        let args = [
            "enroll",
            self.state.to_str().unwrap(),
            file.to_str().unwrap(),
            "--confirm",
            p["confirmation"].as_str().unwrap(),
        ];
        let off = self.run(&args);
        assert!(!off.status.success());
        assert!(String::from_utf8_lossy(&off.stderr).contains("OFF"));
        assert!(!self.state.join("registration.json").exists());
        let mut on = args.to_vec();
        on.push("--enable-contract-1-i");
        self.ok(&on);
    }
    fn grant(&self, action: &str, name: &str) -> PathBuf {
        let r = self.ok(&["review", self.state.to_str().unwrap(), action]);
        let path = self.home.path().join(name);
        self.ok(&[
            "approve",
            self.state.to_str().unwrap(),
            self.signer.to_str().unwrap(),
            action,
            "--confirm",
            r["confirmation"].as_str().unwrap(),
            path.to_str().unwrap(),
        ]);
        path
    }
    fn execute(&self, action: &str, path: &std::path::Path) -> Output {
        self.run(&[
            "execute",
            self.state.to_str().unwrap(),
            action,
            path.to_str().unwrap(),
            "--enable-contract-1-i",
        ])
    }
}
#[test]
fn independent_cli_processes_round_trip_default_off_and_replay() {
    let f = Fixture::new();
    let ino = fs::metadata(f.root.join("a.bin")).unwrap().ino();
    f.enroll();
    let apply = f.grant("apply", "apply.json");
    let off = f.run(&[
        "execute",
        f.state.to_str().unwrap(),
        "apply",
        apply.to_str().unwrap(),
    ]);
    assert!(!off.status.success());
    let detached = f.home.path().join("signer-offline");
    fs::rename(&f.signer, &detached).unwrap();
    let result = f.execute("apply", &apply);
    fs::rename(&detached, &f.signer).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::metadata(f.root.join("b.bin")).unwrap().ino(), ino);
    assert!(!f.execute("apply", &apply).status.success());
    let undo = f.grant("undo", "undo.json");
    assert!(f.execute("undo", &undo).status.success());
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
    assert_eq!(fs::metadata(f.root.join("a.bin")).unwrap().ino(), ino);
    assert!(!f.root.join("b.bin").exists());
    assert!(!f.execute("undo", &undo).status.success());
}
#[test]
fn external_replacement_refuses_review_and_preserves_foreign_file() {
    let f = Fixture::new();
    f.enroll();
    let apply = f.grant("apply", "apply.json");
    assert!(f.execute("apply", &apply).status.success());
    fs::rename(f.root.join("b.bin"), f.root.join("outside.bin")).unwrap();
    fs::write(f.root.join("b.bin"), b"foreign B").unwrap();
    let result = f.run(&["review", f.state.to_str().unwrap(), "undo"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("differs from approval"));
    assert!(!f.root.join("a.bin").exists());
    assert_eq!(fs::read(f.root.join("outside.bin")).unwrap(), b"original A");
    assert_eq!(fs::read(f.root.join("b.bin")).unwrap(), b"foreign B");
}
#[test]
fn missing_approval_and_stale_operator_review_change_no_library_names() {
    let f = Fixture::new();
    f.enroll();
    let r = f.ok(&["review", f.state.to_str().unwrap(), "apply"]);
    fs::write(f.root.join("b.bin"), b"foreign B").unwrap();
    let grant = f.home.path().join("stale.json");
    let o = f.run(&[
        "approve",
        f.state.to_str().unwrap(),
        f.signer.to_str().unwrap(),
        "apply",
        "--confirm",
        r["confirmation"].as_str().unwrap(),
        grant.to_str().unwrap(),
    ]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("stale"));
    assert!(!grant.exists());
    assert!(!f.execute("apply", &grant).status.success());
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
    assert_eq!(fs::read(f.root.join("b.bin")).unwrap(), b"foreign B");
}
#[test]
fn read_only_status_and_grant_inspection_after_restart() {
    let f = Fixture::new();
    let s = f.ok(&["status", f.state.to_str().unwrap()]);
    assert_eq!(s["state"], "not_enrolled");
    f.enroll();
    let s = f.ok(&["status", f.state.to_str().unwrap()]);
    assert_eq!(
        (s["state"].as_str(), s["next"].as_str()),
        (Some("awaiting_apply"), Some("review_apply"))
    );
    let apply = f.grant("apply", "apply.json");
    let usable = f.ok(&[
        "inspect-grant",
        f.state.to_str().unwrap(),
        "apply",
        apply.to_str().unwrap(),
    ]);
    assert_eq!(usable["verdict"], "usable");
    assert!(f.execute("apply", &apply).status.success());
    // Each CLI invocation is a fresh process: a restart by construction.
    let spent = f.run(&[
        "inspect-grant",
        f.state.to_str().unwrap(),
        "apply",
        apply.to_str().unwrap(),
    ]);
    assert_eq!(spent.status.code(), Some(2));
    let spent: Value = serde_json::from_slice(&spent.stdout).unwrap();
    assert_eq!(spent["verdict"], "consumed");
    assert_eq!(spent["status"]["state"], "applied");
    // A contradiction is reported as review_required with exit 2, never guessed.
    fs::rename(f.root.join("b.bin"), f.home.path().join("moved-away.bin")).unwrap();
    let review = f.run(&["status", f.state.to_str().unwrap()]);
    assert_eq!(review.status.code(), Some(2));
    let review: Value = serde_json::from_slice(&review.stdout).unwrap();
    assert_eq!(review["state"], "review_required");
    assert_eq!(review["next"], "operator_review");
    assert!(
        !f.root.join("b.bin").exists(),
        "status must not repair anything"
    );
}

#[test]
fn closure_empty_or_interrupted_grant_has_actionable_verdict_without_consumption() {
    fn snapshot(dir: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(snapshot(&p));
            } else {
                out.push((p.clone(), fs::read(&p).unwrap()));
            }
        }
        out.sort();
        out
    }
    let f = Fixture::new();
    f.enroll();
    for (index, bytes) in [b"".as_slice(), b"{", b"not json"].iter().enumerate() {
        let path = f
            .home
            .path()
            .join(format!("interrupted-grant-{index}.json"));
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let before = snapshot(&f.state);
        let inspected = f.run(&[
            "inspect-grant",
            f.state.to_str().unwrap(),
            "apply",
            path.to_str().unwrap(),
        ]);
        assert_eq!(inspected.status.code(), Some(2));
        let verdict: Value = serde_json::from_slice(&inspected.stdout)
            .expect("structured grant refusal rather than raw parse error");
        assert_eq!(verdict["verdict"], "foreign_or_invalid");
        assert!(
            verdict["reason"]
                .as_str()
                .unwrap()
                .contains("new grant filename")
        );
        let executed = f.execute("apply", &path);
        assert_eq!(executed.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&executed.stderr).contains("new grant filename"));
        assert_eq!(
            snapshot(&f.state),
            before,
            "invalid grant must not change authority"
        );
        assert_eq!(fs::read(&path).unwrap(), *bytes);
        assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
        assert!(!f.root.join("b.bin").exists());
    }
}

#[test]
fn successive_signed_transactions_share_root_without_reusing_history_or_grants() {
    let mut f = Fixture::new();
    let ino = fs::metadata(f.root.join("a.bin")).unwrap().ino();
    let original = f.state.clone();
    f.enroll();
    let first = f.grant("apply", "first-apply.json");
    assert!(f.execute("apply", &first).status.success());
    let next = f.home.path().join("state-2");
    assert!(
        !f.run(&[
            "preview-reuse",
            original.to_str().unwrap(),
            next.to_str().unwrap()
        ])
        .status
        .success()
    );
    let undo = f.grant("undo", "first-undo.json");
    assert!(f.execute("undo", &undo).status.success());
    let preview = f.ok(&[
        "preview-reuse",
        original.to_str().unwrap(),
        next.to_str().unwrap(),
    ]);
    let args = [
        "reuse",
        original.to_str().unwrap(),
        next.to_str().unwrap(),
        "--confirm",
        preview["confirmation"].as_str().unwrap(),
    ];
    assert!(!f.run(&args).status.success());
    assert!(!next.exists());
    let mut enabled = args.to_vec();
    enabled.push("--enable-contract-1-i");
    f.ok(&enabled);
    assert_eq!(
        f.ok(&["status", next.to_str().unwrap()])["state"],
        "not_enrolled"
    );
    let history = f.ok(&["history", next.to_str().unwrap()]);
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert!(history[0]["historical_settlement"].as_bool().unwrap());
    f.state = next;
    let file = f.home.path().join("second-plan.json");
    let plan = f.ok(&[
        "plan",
        f.state.to_str().unwrap(),
        file.to_str().unwrap(),
        "a.bin",
        "b.bin",
    ]);
    f.ok(&[
        "enroll",
        f.state.to_str().unwrap(),
        file.to_str().unwrap(),
        "--confirm",
        plan["confirmation"].as_str().unwrap(),
        "--enable-contract-1-i",
    ]);
    assert!(!f.execute("apply", &first).status.success());
    assert_eq!(fs::metadata(f.root.join("a.bin")).unwrap().ino(), ino);
    let apply = f.grant("apply", "second-apply.json");
    assert!(f.execute("apply", &apply).status.success());
    assert_eq!(
        f.ok(&["status", original.to_str().unwrap()])["state"],
        "settled"
    );
    assert_eq!(
        f.ok(&["history", f.state.to_str().unwrap()])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let undo = f.grant("undo", "second-undo.json");
    assert!(f.execute("undo", &undo).status.success());
    assert_eq!(fs::metadata(f.root.join("a.bin")).unwrap().ino(), ino);
    assert_eq!(fs::read(f.root.join("a.bin")).unwrap(), b"original A");
}
