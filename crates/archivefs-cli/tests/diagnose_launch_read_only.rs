//! `diagnose-launch` end to end, in a subprocess with disposable roots.
//! No real application folders, emulators, databases or game files are
//! used, and the command must leave everything it was shown untouched.
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const VERBOSE_VULKAN_FAILURE: &str =
    include_str!("../../archivefs-core/tests/fixtures/daedalus/retroarch-1.21.0/vk-noicd.log");

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
        };
        for folder in ["bin", "data", "config", "work"] {
            fs::create_dir(fixture.path(folder)).unwrap();
        }
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn write(&self, relative: &str, content: &str) -> String {
        let path = self.path(relative);
        fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
            .env_clear()
            .env("HOME", self.root.path())
            .env("PATH", self.path("bin"))
            .env("EMUWIZ_DATA_HOME", self.path("data"))
            .env("EMUWIZ_CONFIG_HOME", self.path("config"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("TMPDIR", self.path("tmp"))
            .current_dir(self.root.path())
            .args(args)
            .output()
            .unwrap()
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

/// Inode, mode, modification time (seconds, nanoseconds) and bytes of one path.
type Snapshot = BTreeMap<PathBuf, (u64, u32, i64, i64, Vec<u8>)>;

/// Every file and folder under `root`: bytes, inode, mode and modification
/// time. Access times may change from reading and are not compared.
fn snapshot(root: &Path) -> Snapshot {
    fn walk(path: &Path, out: &mut Snapshot) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_file() {
            fs::read(path).unwrap()
        } else {
            Vec::new()
        };
        out.insert(
            path.to_path_buf(),
            (
                metadata.ino(),
                metadata.mode(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                bytes,
            ),
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                walk(&entry.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

#[test]
fn a_captured_vulkan_failure_is_diagnosed_and_nothing_is_touched() {
    let fixture = Fixture::new();
    let config = fixture.write(
        "work/retroarch.cfg",
        "# my settings\nvideo_driver = \"vulkan\"\naudio_driver = \"alsa\"\n",
    );
    let log = fixture.write("work/retroarch.log", VERBOSE_VULKAN_FAILURE);
    let before = snapshot(fixture.root.path());

    let args = [
        "diagnose-launch",
        "--emulator",
        "retroarch",
        "--exit-code",
        "1",
        "--duration-ms",
        "1488",
        "--settings",
        &config,
        "--log-file",
        &log,
        "--env",
        "DISPLAY=:99",
        "--env-unset",
        "WAYLAND_DISPLAY",
    ];
    let text = stdout(&fixture.run(&args));
    assert!(
        text.contains("Vulkan graphics could not start [probable]"),
        "{text}"
    );
    assert!(text.contains("Verdict:"), "{text}");
    assert!(text.contains("(not applied)"), "{text}");

    let mut json_args = args.to_vec();
    json_args.push("--json");
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&fixture.run(&json_args))).unwrap();
    assert_eq!(report["verdict"], "partly_explained");
    assert_eq!(
        report["causes"][0]["rule"],
        "retroarch.video.vulkan_unavailable"
    );
    assert_eq!(report["causes"][0]["confidence"], "probable");
    assert_eq!(report["symptoms"][0]["explains_cause"], false);
    assert!(report["rule_set"]["digest"].as_str().unwrap().len() == 64);
    // Every supporting id resolves to an evidence item in the same report.
    let ids: Vec<&str> = report["evidence"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    for supporting in report["causes"][0]["supporting"].as_array().unwrap() {
        assert!(ids.contains(&supporting.as_str().unwrap()), "{supporting}");
    }

    assert_eq!(snapshot(fixture.root.path()), before);
}

#[test]
fn help_prints_help_and_does_nothing_else() {
    let fixture = Fixture::new();
    let before = snapshot(fixture.root.path());
    let text = stdout(&fixture.run(&["diagnose-launch", "--help"]));
    assert!(text.contains("diagnose-launch --emulator"), "{text}");
    assert!(!text.contains("Verdict"), "{text}");
    assert_eq!(snapshot(fixture.root.path()), before);
}

#[test]
fn bad_usage_fails_without_running_a_diagnosis() {
    let fixture = Fixture::new();
    for args in [
        &["diagnose-launch"][..],
        &["diagnose-launch", "--emulator", "retroarch", "--help"],
        &[
            "diagnose-launch",
            "--emulator",
            "retroarch",
            "--apply-repair",
        ],
        &["diagnose-launch", "--exit-code", "1"],
    ] {
        let output = fixture.run(args);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(message.contains("diagnose-launch"), "{message}");
    }
}

#[test]
fn the_rules_listing_is_available_as_text_and_json() {
    let fixture = Fixture::new();
    let text = stdout(&fixture.run(&["diagnose-launch", "--rules"]));
    assert!(
        text.contains("retroarch.video.vulkan_unavailable"),
        "{text}"
    );
    let json: serde_json::Value = serde_json::from_str(&stdout(&fixture.run(&[
        "diagnose-launch",
        "--rules",
        "--json",
    ])))
    .unwrap();
    assert!(json["rules"].as_array().unwrap().len() >= 20);
    assert_eq!(json["engine_version"], 1);
}

#[test]
fn hostile_log_text_never_reaches_the_terminal_raw() {
    let fixture = Fixture::new();
    let log = fixture.write(
        "work/hostile.log",
        "[INFO] [Core]: Loading dynamic libretro core from: \"/cores/x\x1b[2J\x1b]0;owned\x07.so\"\n\
         [ERROR] Failed to open libretro core: \"/cores/\u{202E}os.x\"\n",
    );
    let output = fixture.run(&[
        "diagnose-launch",
        "--emulator",
        "retroarch",
        "--exit-code",
        "1",
        "--log-file",
        &log,
    ]);
    let text = stdout(&output);
    assert!(text.contains("core could not be opened"), "{text}");
    assert!(!text.contains('\x1b') && !text.contains('\x07') && !text.contains('\u{202E}'));
}
