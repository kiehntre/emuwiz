//! `launch-history` end to end, in a subprocess with disposable roots. No
//! real application folder is used.
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
        };
        for folder in ["bin", "data", "config", "work"] {
            fs::create_dir(fixture.root.path().join(folder)).unwrap();
        }
        fixture
    }

    fn run(&self, args: &[&str]) -> Output {
        let path = |name: &str| self.root.path().join(name);
        Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
            .env_clear()
            .env("HOME", self.root.path())
            .env("PATH", path("bin"))
            .env("EMUWIZ_DATA_HOME", path("data"))
            .env("EMUWIZ_CONFIG_HOME", path("config"))
            .env("XDG_CACHE_HOME", path("cache"))
            .env("TMPDIR", path("tmp"))
            .current_dir(self.root.path())
            .args(args)
            .output()
            .unwrap()
    }

    /// Everything the command made: every path under the fixture apart
    /// from the four empty folders it starts with.
    fn made(&self) -> Vec<PathBuf> {
        fn walk(root: &Path, path: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                found.push(path.strip_prefix(root).unwrap().to_path_buf());
                if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
                    walk(root, &path, found);
                }
            }
        }
        let mut found = Vec::new();
        walk(self.root.path(), self.root.path(), &mut found);
        found.retain(|path| {
            !["bin", "data", "config", "work"]
                .iter()
                .any(|start| path == Path::new(start))
        });
        found.sort();
        found
    }

    /// Every path under the fixture whose last component is `name`.
    fn find(&self, name: &str) -> Vec<PathBuf> {
        fn walk(path: &Path, name: &str, found: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if entry.file_name() == name {
                    found.push(path.clone());
                }
                if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
                    walk(&path, name, found);
                }
            }
        }
        let mut found = Vec::new();
        walk(self.root.path(), name, &mut found);
        found
    }
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn looking_at_an_empty_history_creates_nothing() {
    let fixture = Fixture::new();
    let help = stdout(&fixture.run(&["launch-history", "--help"]));
    assert!(help.contains("launch-history off | on"), "{help}");
    assert!(help.contains("no emulator output is stored"), "{help}");

    for args in [
        &["launch-history"][..],
        &["launch-history", "list"],
        &["launch-history", "status"],
    ] {
        let text = stdout(&fixture.run(args));
        assert!(text.contains("Recording is on"), "{text}");
    }
    let listing = stdout(&fixture.run(&["launch-history"]));
    assert!(listing.contains("No launch is on record."), "{listing}");
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fixture.run(&["launch-history", "list", "--json"]))).unwrap();
    assert_eq!(json["recording"], true);
    assert_eq!(json["records"], serde_json::json!([]));

    // Nothing that only reads made the history folder, or anything else.
    assert!(fixture.find("launch-diagnostics").is_empty());
    assert_eq!(fixture.made(), Vec::<PathBuf>::new());
}

#[test]
fn recording_can_be_switched_off_and_on_and_the_folder_is_private() {
    let fixture = Fixture::new();
    let off = stdout(&fixture.run(&["launch-history", "off"]));
    assert!(off.starts_with("Recording is off."), "{off}");
    let folders = fixture.find("launch-diagnostics");
    assert_eq!(folders.len(), 1, "{folders:?}");
    assert_eq!(fs::metadata(&folders[0]).unwrap().mode() & 0o777, 0o700);
    let marker = folders[0].join("history-disabled");
    assert_eq!(fs::metadata(&marker).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::read(&marker).unwrap(), b"");

    let status = stdout(&fixture.run(&["launch-history", "status"]));
    assert!(status.contains("Recording is off."), "{status}");
    // Clearing keeps the switch.
    let cleared = stdout(&fixture.run(&["launch-history", "clear"]));
    assert_eq!(cleared.trim(), "Removed 0 record(s).");
    assert!(marker.exists());

    let on = stdout(&fixture.run(&["launch-history", "on"]));
    assert_eq!(on.trim(), "Recording is on.");
    assert!(!marker.exists());

    // Switching and clearing made the history folder and nothing else: no
    // library database, no Undo seal store, no writer record, no settings.
    assert_eq!(
        fixture.made(),
        [PathBuf::from("data/launch-diagnostics")],
        "after on"
    );
}

#[test]
fn bad_requests_fail_without_touching_anything() {
    let fixture = Fixture::new();
    for (args, message) in [
        (
            &["launch-history", "purge"][..],
            "not a launch-history command",
        ),
        (&["launch-history", "show"], "a record id is needed"),
        (
            &["launch-history", "clear", "--help"],
            "not a launch-history command",
        ),
        (
            &["launch-history", "clear", "--json"],
            "--json applies to list and show only",
        ),
        (
            &["launch-history", "show", "20270115T080000Z-0123abcd"],
            "no record has that id",
        ),
        (
            &["launch-history", "show", "../../etc/passwd"],
            "not a record id",
        ),
        (
            &["launch-history", "delete", "../../etc/passwd"],
            "not a record id",
        ),
    ] {
        let output = fixture.run(args);
        assert!(!output.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{args:?}: {stderr}");
    }
    // A refused request made nothing: not even the history folder.
    assert!(fixture.find("launch-diagnostics").is_empty());
    assert!(fixture.find("passwd").is_empty());
    assert_eq!(fixture.made(), Vec::<PathBuf>::new());
}
