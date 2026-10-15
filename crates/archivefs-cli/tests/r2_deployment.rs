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
        Command::new(env!("CARGO_BIN_EXE_emuwiz-r2"))
            .args(args)
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path())
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
