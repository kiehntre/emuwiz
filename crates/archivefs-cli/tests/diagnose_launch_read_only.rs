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
        text.contains("Vulkan graphics could not start [possible]"),
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
    assert_eq!(report["causes"][0]["confidence"], "possible");
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
    assert_eq!(json["engine_version"], 2);
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

#[test]
fn what_was_typed_is_reported_as_supplied_and_every_unread_input_is_listed() {
    let fixture = Fixture::new();
    let main = fixture.write("work/retroarch.cfg", "video_driver = \"gl\"\n");
    let hidden = fixture.write("work/real-override.cfg", "video_driver = \"vulkan\"\n");
    let link = fixture.path("work/game.cfg");
    std::os::unix::fs::symlink(&hidden, &link).unwrap();
    let stderr = fixture.write(
        "work/stderr.txt",
        "Fatal error received in: \"video_driver_init_internal()\"\n",
    );
    let args = [
        "diagnose-launch",
        "--emulator",
        " RetroArch ",
        "--exit-code",
        "1",
        "--settings",
        &main,
        "--game-settings",
        link.to_str().unwrap(),
        "--stderr-file",
        &stderr,
    ];
    let text = stdout(&fixture.run(&args));
    // The unread override is named, even though the area has other evidence.
    assert!(
        text.contains("Emulator settings (partly observed):"),
        "{text}"
    );
    assert!(text.contains("could not be read"), "{text}");
    assert!(text.contains("[possible]"), "{text}");
    assert!(text.contains("EmuWiz did not observe it"), "{text}");
    assert!(text.contains("[as reported]"), "{text}");
    assert!(
        text.contains("Launch: the emulator process failed"),
        "{text}"
    );

    let mut json_args = args.to_vec();
    json_args.push("--json");
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&fixture.run(&json_args))).unwrap();
    let exit = report["evidence"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["area"] == "process_exit")
        .unwrap();
    assert_eq!(exit["trust"], "supplied");
    assert!(exit["source"].as_str().unwrap().contains("supplied"));
    assert_ne!(report["verdict"], "explained");
}

#[test]
fn a_crash_next_to_an_unwritable_save_folder_is_not_called_explained() {
    let fixture = Fixture::new();
    let saves = fixture.path("work/saves");
    fs::create_dir(&saves).unwrap();
    fs::set_permissions(&saves, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
    let output = fixture.run(&[
        "diagnose-launch",
        "--emulator",
        "retroarch",
        "--signal",
        "11",
        "--save-dir",
        saves.to_str().unwrap(),
        "--json",
    ]);
    fs::set_permissions(&saves, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    // Running as root makes the folder writable; then there is no cause at all.
    assert_eq!(report["verdict"], "unknown");
}

#[test]
fn stop_requests_relayed_by_a_launcher_are_not_failures() {
    let fixture = Fixture::new();
    for code in ["143", "130"] {
        let report: serde_json::Value = serde_json::from_str(&stdout(&fixture.run(&[
            "diagnose-launch",
            "--emulator",
            "retroarch",
            "--exit-code",
            code,
            "--launcher",
            "flatpak run",
            "--json",
        ])))
        .unwrap();
        assert_eq!(report["verdict"], "no_problem_observed", "{code}");
        assert!(report["symptoms"].as_array().unwrap().is_empty(), "{code}");
        let notes = report["notes"].to_string();
        assert!(notes.contains("stop request"), "{notes}");
    }
}

#[test]
fn hostile_paths_reach_neither_the_error_message_nor_the_json_raw() {
    let fixture = Fixture::new();
    let missing = fixture
        .path("work/gone\x1b]0;owned\x07.txt")
        .to_str()
        .unwrap()
        .to_string();
    let output = fixture.run(&[
        "diagnose-launch",
        "--emulator",
        "retroarch",
        "--stderr-file",
        &missing,
    ]);
    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("cannot read"), "{message}");
    assert!(
        !message.contains('\x1b') && !message.contains('\x07'),
        "{message:?}"
    );

    let odd = fixture.write("work/odd\u{202E}\u{9b}name.cfg", "video_driver = \"gl\"\n");
    for extra in [&[][..], &["--json"][..]] {
        let mut args = vec![
            "diagnose-launch",
            "--emulator",
            "retroarch",
            "--settings",
            &odd,
            "--game-file",
            &odd,
        ];
        args.extend_from_slice(extra);
        let text = stdout(&fixture.run(&args));
        for raw in ['\u{202E}', '\u{9b}', '\x1b'] {
            assert!(
                !text.contains(raw),
                "U+{:04X} in {extra:?} output",
                raw as u32
            );
        }
    }
}

#[test]
fn values_that_are_not_what_they_claim_to_be_are_refused() {
    let fixture = Fixture::new();
    for args in [
        &[
            "diagnose-launch",
            "--emulator",
            "retroarch",
            "--exit-code",
            "256",
        ][..],
        &[
            "diagnose-launch",
            "--emulator",
            "retroarch",
            "--exit-code",
            "-1",
        ],
        &[
            "diagnose-launch",
            "--emulator",
            "retroarch",
            "--game-file",
            "",
        ],
        &[
            "diagnose-launch",
            "--emulator",
            "retroarch",
            "--env",
            "DISPLAY=:0",
            "--env-unset",
            "DISPLAY",
        ],
    ] {
        let output = fixture.run(args);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn a_failure_that_was_only_typed_is_never_called_established_or_explained() {
    let fixture = Fixture::new();
    for reason in ["not-found", "permission-denied"] {
        let args = [
            "diagnose-launch",
            "--emulator",
            "retroarch",
            "--spawn-error",
            reason,
        ];
        let text = stdout(&fixture.run(&args));
        assert!(text.contains("[possible]"), "{text}");
        assert!(!text.contains("[established]"), "{text}");
        assert!(text.contains("Verdict: partly explained"), "{text}");
        assert!(text.contains("According to what was supplied"), "{text}");
        assert!(text.contains("EmuWiz did not observe it"), "{text}");

        let mut json_args = args.to_vec();
        json_args.push("--json");
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&fixture.run(&json_args))).unwrap();
        assert_eq!(report["verdict"], "partly_explained", "{reason}");
        assert_eq!(report["launch"], "failed", "{reason}");
        for cause in report["causes"].as_array().unwrap() {
            assert_eq!(cause["confidence"], "possible", "{cause}");
        }
    }

    // No display, from a typed exit status and a typed environment.
    let report: serde_json::Value = serde_json::from_str(&stdout(&fixture.run(&[
        "diagnose-launch",
        "--emulator",
        "retroarch",
        "--exit-code",
        "1",
        "--env-unset",
        "DISPLAY",
        "--env-unset",
        "WAYLAND_DISPLAY",
        "--json",
    ])))
    .unwrap();
    assert_ne!(report["verdict"], "explained");
    for finding in report["causes"]
        .as_array()
        .unwrap()
        .iter()
        .chain(report["symptoms"].as_array().unwrap())
    {
        assert_eq!(finding["confidence"], "possible", "{finding}");
    }
}

#[test]
fn a_mistyped_path_is_worded_as_given_and_a_missing_settings_file_is_listed() {
    let fixture = Fixture::new();
    let missing_core = fixture.path("work/typo_libretro.so");
    let missing_settings = fixture.path("work/retroarch.cgf");
    let text = stdout(&fixture.run(&[
        "diagnose-launch",
        "--emulator",
        "retroarch",
        "--core",
        missing_core.to_str().unwrap(),
        "--settings",
        missing_settings.to_str().unwrap(),
    ]));
    assert!(
        text.contains("The file given as the emulator core does not exist."),
        "{text}"
    );
    assert!(!text.contains("EmuWiz asked RetroArch"), "{text}");
    assert!(
        text.contains("did not observe the launch using them"),
        "{text}"
    );
    assert!(
        text.contains("no settings file exists at the place given for: main settings"),
        "{text}"
    );

    // Another emulator's launch is not judged by RetroArch's rules.
    let text = stdout(&fixture.run(&[
        "diagnose-launch",
        "--emulator",
        "dolphin",
        "--core",
        missing_core.to_str().unwrap(),
    ]));
    assert!(!text.contains("RetroArch"), "{text}");
}

#[test]
fn a_status_a_shell_would_relay_is_a_failure_when_no_launcher_is_named() {
    let fixture = Fixture::new();
    let report: serde_json::Value = serde_json::from_str(&stdout(&fixture.run(&[
        "diagnose-launch",
        "--emulator",
        "retroarch",
        "--exit-code",
        "143",
        "--json",
    ])))
    .unwrap();
    assert_eq!(report["launch"], "failed");
    assert_eq!(report["verdict"], "unknown");
    let symptoms = report["symptoms"].to_string();
    assert!(
        symptoms.contains("may have been asked to stop"),
        "{symptoms}"
    );
}

#[test]
fn hostile_log_text_never_reaches_the_json_raw_either() {
    let fixture = Fixture::new();
    let stderr = fixture.write(
        "work/stderr.txt",
        "[ERROR] Failed to open libretro core: \"\u{1b}[2J\u{202e}evil\u{200b}\u{fe0f}\u{34f}\"\n",
    );
    let json = stdout(&fixture.run(&[
        "diagnose-launch",
        "--emulator",
        "retroarch",
        "--exit-code",
        "1",
        "--stderr-file",
        &stderr,
        "--json",
    ]));
    for raw in ['\u{1b}', '\u{202e}', '\u{200b}', '\u{fe0f}', '\u{34f}'] {
        assert!(!json.contains(raw), "{raw:?} in {json}");
    }
    for escaped in ["\\u001b", "\\u202e", "\\u200b", "\\ufe0f", "\\u034f"] {
        assert!(
            !json.to_lowercase().contains(escaped),
            "{escaped} in {json}"
        );
    }
    assert!(json.contains("evil"), "{json}");
}
