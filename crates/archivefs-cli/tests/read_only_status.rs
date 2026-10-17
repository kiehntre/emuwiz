//! Disposable, subprocess-isolated CLI checks. No real application roots,
//! installed emulators, databases, journals or ROMs are used.
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    root: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let f = Self {
            root: tempfile::tempdir().unwrap(),
        };
        fs::create_dir(f.path("bin")).unwrap();
        fs::create_dir(f.path("data")).unwrap();
        f
    }
    fn path(&self, p: &str) -> PathBuf {
        self.root.path().join(p)
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"));
        c.env_clear()
            .env("HOME", self.root.path())
            .env("PATH", self.path("bin"))
            .env("EMUWIZ_DATA_HOME", self.path("data"))
            .env("EMUWIZ_CONFIG_HOME", self.path("config"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("TMPDIR", self.path("tmp"))
            .current_dir(self.root.path())
            .args(args);
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn json(&self, args: &[&str]) -> serde_json::Value {
        let o = self.run(args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&o.stdout)))
    }
}

// Compare file bytes, inode, mode and modification time and all directory
// membership. Access times can legitimately change from read-only inspection.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, u32, i64, i64, Vec<u8>)> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, (u64, u32, i64, i64, Vec<u8>)>) {
        let m = fs::symlink_metadata(path).unwrap();
        let bytes = if m.is_file() {
            fs::read(path).unwrap()
        } else if m.file_type().is_symlink() {
            fs::read_link(path)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else {
            vec![]
        };
        out.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            (m.ino(), m.mode(), m.mtime(), m.mtime_nsec(), bytes),
        );
        if m.is_dir() {
            for e in fs::read_dir(path).unwrap() {
                walk(root, &e.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn read_only_commands_empty_json_and_human_output_do_not_create_state() {
    let f = Fixture::new();
    let before = snapshot(f.root.path());
    for command in ["emulators", "activity", "history"] {
        let report = f.json(&[command, "--json"]);
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["command"], command);
        assert_eq!(report["read_only"], true);
        assert_eq!(report["coverage_complete"], false);
        assert!(report["problems"].is_array());
        if command != "emulators" {
            assert!(report["records"].as_array().unwrap().is_empty());
        }
        let human = f.run(&[command]);
        assert!(human.status.success());
        assert!(
            String::from_utf8(human.stdout)
                .unwrap()
                .contains("read-only")
        );
    }
    assert_eq!(snapshot(f.root.path()), before);
}

#[test]
fn read_only_commands_help_and_invalid_arguments_never_access_state() {
    let f = Fixture::new();
    let before = snapshot(f.root.path());
    let o = f.run(&["--help"]);
    let help = String::from_utf8(o.stdout).unwrap();
    for command in ["emulators", "activity", "history"] {
        assert!(help.contains(command));
        let o = f.run(&[command, "--help"]);
        assert!(o.status.success());
        assert!(
            String::from_utf8(o.stdout)
                .unwrap()
                .contains("None repairs")
        );
        for action in [
            "apply",
            "install",
            "download",
            "launch",
            "rollback",
            "--unknown",
        ] {
            let o = f.run(&[command, action]);
            assert!(!o.status.success());
            assert!(String::from_utf8(o.stderr).unwrap().contains("usage:"));
        }
    }
    assert_eq!(snapshot(f.root.path()), before);
}

#[test]
fn read_only_emulator_status_never_runs_candidate_executable() {
    let f = Fixture::new();
    let path = f.path("bin/mame");
    fs::write(
        &path,
        b"#!/bin/sh\necho mutated > \"$HOME/SHOULD_NOT_EXIST\"\necho MAME 0.999\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let before = snapshot(f.root.path());
    let report = f.json(&["emulators", "status", "--json"]);
    let mame = report["emulators"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["emulator_id"] == "MAME")
        .unwrap();
    assert_eq!(mame["installations"].as_array().unwrap().len(), 1);
    assert!(mame["installations"][0]["version"]["version"].is_null());
    assert!(mame["installations"][0]["launch_readiness"].is_null());
    assert_eq!(snapshot(f.root.path()), before);
}

#[test]
fn read_only_history_preserves_records_problems_and_activity_filter() {
    let f = Fixture::new();
    let dir = f
        .path("data")
        .join(archivefs_core::dat::rename_apply::RENAME_TRANSACTIONS_DIRECTORY);
    fs::create_dir(&dir).unwrap();
    for (id, state) in [("done", "applied"), ("waiting", "planned")] {
        let record = serde_json::json!({"transaction_id":id,"plan_generation":1,"created_at_unix":2,"source_scan_root":"/synthetic","state":state,"entries":[]});
        fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }
    fs::write(dir.join("bad.json"), b"malformed synthetic receipt").unwrap();
    let before = snapshot(f.root.path());
    let history = f.json(&["history", "list", "--json"]);
    let activity = f.json(&["activity", "list", "--json"]);
    assert_eq!(history["records"].as_array().unwrap().len(), 2);
    assert_eq!(activity["records"].as_array().unwrap().len(), 1);
    assert_eq!(activity["records"][0]["operation_id"], "waiting");
    assert!(!history["problems"].as_array().unwrap().is_empty());
    let o = f.run(&["history"]);
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(text.contains("done"));
    assert!(text.contains("waiting"));
    assert!(text.contains("Warning:"));
    assert_eq!(snapshot(f.root.path()), before);
}

#[test]
fn read_only_commands_report_unavailable_roots_and_skip_cwd_path() {
    let f = Fixture::new();
    let before = snapshot(f.root.path());
    for command in ["emulators", "activity", "history"] {
        let o = f
            .command(&[command, "--json"])
            .env("EMUWIZ_DATA_HOME", "relative-invalid-root")
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(o.status.success());
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert!(!v["problems"].as_array().unwrap().is_empty());
        assert!(String::from_utf8(o.stdout).unwrap().contains("unavailable"));
    }
    assert_eq!(snapshot(f.root.path()), before);
}

#[test]
fn read_only_emulator_status_retains_invalid_managed_manifest() {
    let f = Fixture::new();
    let install = f.path("data/emulators/PCSX2/installs/synthetic");
    fs::create_dir_all(&install).unwrap();
    fs::write(install.join("manifest.json"), b"synthetic invalid manifest").unwrap();
    let before = snapshot(f.root.path());
    let v = f.json(&["emulators", "--json"]);
    assert_eq!(v["managed_installations"].as_array().unwrap().len(), 1);
    assert_eq!(v["managed_installations"][0]["health"], "INVALID_MANIFEST");
    assert!(!v["problems"].as_array().unwrap().is_empty());
    let human = f.run(&["emulators"]);
    assert!(
        String::from_utf8(human.stdout)
            .unwrap()
            .contains("InvalidManifest")
    );
    assert_eq!(snapshot(f.root.path()), before);
}

#[test]
fn read_only_emulator_scope_refuses_relative_path_and_reports_path_limits() {
    let f = Fixture::new();
    fs::write(f.path("mame"), b"synthetic candidate in cwd").unwrap();
    fs::set_permissions(f.path("mame"), fs::Permissions::from_mode(0o755)).unwrap();
    let before = snapshot(f.root.path());
    for path in [
        String::new(),
        ".".into(),
        std::iter::repeat_n(f.path("bin").display().to_string(), 64)
            .chain([f.root.path().display().to_string()])
            .collect::<Vec<_>>()
            .join(":"),
    ] {
        let o = f
            .command(&["emulators", "--json"])
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(o.status.success());
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert!(v["emulators"].as_array().unwrap().is_empty());
        assert!(!v["problems"].as_array().unwrap().is_empty());
    }
    assert_eq!(snapshot(f.root.path()), before);
}

#[test]
fn read_only_history_directory_failure_is_explicit_in_both_formats() {
    let f = Fixture::new();
    let dir = f
        .path("data")
        .join(archivefs_core::dat::rename_apply::RENAME_TRANSACTIONS_DIRECTORY);
    fs::write(dir, b"synthetic non-directory").unwrap();
    let before = snapshot(f.root.path());
    for command in ["activity", "history"] {
        let v = f.json(&[command, "--json"]);
        assert!(!v["problems"].as_array().unwrap().is_empty());
        assert!(
            v["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["state"] == "unavailable")
        );
        let o = f.run(&[command]);
        assert!(String::from_utf8(o.stdout).unwrap().contains("unavailable"));
    }
    assert_eq!(snapshot(f.root.path()), before);
}
