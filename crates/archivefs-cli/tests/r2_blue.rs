//! Disposable Blue operator boundary, using the same packaged CLI entry point.
#![cfg(target_os = "linux")]
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};
struct Blue {
    home: tempfile::TempDir,
}
impl Blue {
    fn new() -> Self {
        Self {
            home: tempfile::Builder::new()
                .prefix("emuwiz-blue-r2-")
                .permissions(fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap(),
        }
    }
    fn path(&self, n: &str) -> String {
        self.home.path().join(n).to_str().unwrap().into()
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
            .arg("--r2-operator")
            .args(args)
            .env("HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path())
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn root(&self) -> String {
        let root = self.path("collection");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(Path::new(&root).join("a.bin"), b"synthetic original").unwrap();
        root
    }
    /// The R2 gate admits only canonical local ext4 with 4096-byte blocks. On any
    /// other host the supported path cannot run, so assert the explicit refusal
    /// and that nothing was written. `EMUWIZ_R2_REQUIRE_QUALIFIED=1` (set by CI on
    /// a deliberately qualified loop filesystem) turns that fallback into a failure
    /// so the supported subset can never be silently skipped.
    fn qualified(&self, root: &str) -> bool {
        let o = self.run(&["blue-eligibility", root]);
        if o.status.success() {
            assert!(
                std::env::var_os("EMUWIZ_R2_REQUIRE_REFUSAL").is_none(),
                "refusal required but the filesystem was admitted"
            );
            return true;
        }
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            std::env::var_os("EMUWIZ_R2_REQUIRE_QUALIFIED").is_none(),
            "qualified filesystem required but R2 refused: {err}"
        );
        assert!(
            err.contains("filesystem has not passed this candidate's eligibility tests"),
            "unexpected refusal: {err}"
        );
        assert!(!Path::new(root).join(".emuwiz-r2-holding").exists());
        assert!(!self
            .run(&["blue-init", root, &self.path("workspaces/blue")])
            .status
            .success());
        assert_eq!(
            fs::read(Path::new(root).join("a.bin")).unwrap(),
            b"synthetic original"
        );
        false
    }
    fn execute(&self, state: &str, action: &str, name: &str) {
        let r = self.ok(&["review", state, action]);
        assert_eq!(r["actionable"], true);
        let file = self.path(name);
        self.ok(&[
            "approve",
            state,
            &self.path("workspaces/blue/operator"),
            action,
            "--confirm",
            r["confirmation"].as_str().unwrap(),
            &file,
        ]);
        assert_eq!(
            self.ok(&["inspect-grant", state, action, &file])["verdict"],
            "usable"
        );
        assert_eq!(
            self.ok(&["execute", state, action, &file, "--enable-contract-1-i"])["completed"],
            true
        );
    }
}
#[test]
fn blue_packaged_operator_preview_explicit_approval_undo_and_root_reuse() {
    let f = Blue::new();
    let root = f.root();
    if !f.qualified(&root) {
        return;
    }
    let ws = f.path("workspaces/blue");
    assert_eq!(f.ok(&["blue-eligibility", &root])["read_only"], true);
    assert!(!Path::new(&root).join(".emuwiz-r2-holding").exists());
    let info = f.ok(&["blue-init", &root, &ws]);
    let state = info["state"].as_str().unwrap();
    assert_eq!(info["status"]["state"], "not_enrolled");
    let file = f.path("plan.json");
    let p = f.ok(&["plan", state, &file, "a.bin", "b.bin"]);
    assert!(!f
        .run(&[
            "enroll",
            state,
            &file,
            "--confirm",
            p["confirmation"].as_str().unwrap()
        ])
        .status
        .success());
    assert!(!f
        .run(&[
            "enroll",
            state,
            &file,
            "--confirm",
            "wrong",
            "--enable-contract-1-i"
        ])
        .status
        .success());
    assert!(Path::new(&root).join("a.bin").exists());
    f.ok(&[
        "enroll",
        state,
        &file,
        "--confirm",
        p["confirmation"].as_str().unwrap(),
        "--enable-contract-1-i",
    ]);
    f.execute(state, "apply", "apply.json");
    assert!(!Path::new(&root).join("a.bin").exists());
    assert!(Path::new(&root).join("b.bin").exists());
    f.execute(state, "undo", "undo.json");
    assert_eq!(
        fs::read(Path::new(&root).join("a.bin")).unwrap(),
        b"synthetic original"
    );
    assert_eq!(
        f.ok(&["blue-info", &root, &ws])["status"]["ownership"],
        "released"
    );
    let next = info["next_state"].as_str().unwrap();
    let r = f.ok(&["preview-reuse", state, next]);
    f.ok(&[
        "reuse",
        state,
        next,
        "--confirm",
        r["confirmation"].as_str().unwrap(),
        "--enable-contract-1-i",
    ]);
    let second = f.ok(&["blue-info", &root, &ws]);
    assert_eq!(second["state"], next);
    assert_eq!(second["status"]["state"], "not_enrolled");
    assert_eq!(f.ok(&["history", next]).as_array().unwrap().len(), 1);
    assert!(Path::new(state).exists());
}
#[test]
fn blue_refuses_aliases_partial_workspace_and_corrupt_authority() {
    let f = Blue::new();
    let root = f.root();
    let alias = f.path("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    assert!(!f.run(&["blue-eligibility", &alias]).status.success());
    if !f.qualified(&root) {
        return;
    }
    let ws = f.path("workspaces/blue");
    let info = f.ok(&["blue-init", &root, &ws]);
    assert!(!f.run(&["blue-init", &root, &ws]).status.success());
    fs::create_dir(Path::new(&ws).join("state-2")).unwrap();
    assert!(!f.run(&["blue-info", &root, &ws]).status.success());
    let dest = f.path("must-not-exist");
    assert!(!f
        .run(&[
            "blue-retrieve",
            info["state"].as_str().unwrap(),
            "0",
            "--confirm",
            "bad",
            &dest
        ])
        .status
        .success());
    assert!(!Path::new(&dest).exists());
    assert_eq!(
        fs::read(Path::new(&root).join("a.bin")).unwrap(),
        b"synthetic original"
    );
}
