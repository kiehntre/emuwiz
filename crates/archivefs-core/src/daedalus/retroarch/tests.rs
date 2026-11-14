use super::*;
use crate::daedalus::evidence::LogEvent;
use crate::daedalus::test_fixtures as fixtures;

fn events(log: &str) -> Vec<LogEvent> {
    let mut set = EvidenceSet::default();
    record_log(&mut set, log.as_bytes(), LogStream::LogFile);
    set.items()
        .iter()
        .filter_map(|item| match &item.fact {
            Fact::LogEvent(event) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

fn meanings(log: &str) -> Vec<LogMeaning> {
    events(log).iter().map(|event| event.meaning).collect()
}

fn verbose(log: &str) -> bool {
    let mut set = EvidenceSet::default();
    record_log(&mut set, log.as_bytes(), LogStream::Stderr);
    set.items()
        .iter()
        .any(|item| matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.verbose))
}

fn has(log: &str, meaning: LogMeaning) -> bool {
    meanings(log).contains(&meaning)
}

#[test]
fn a_missing_vulkan_driver_is_recognised_in_a_verbose_log() {
    let log = fixtures::RA_VULKAN_NO_DRIVER;
    assert!(verbose(log));
    assert!(has(log, LogMeaning::VulkanUnavailable));
    assert!(has(log, LogMeaning::VideoDriverInitFailed));
    assert!(has(log, LogMeaning::CoreLoadStarted));
    assert!(!has(log, LogMeaning::VideoDriverReady));
    assert!(!has(log, LogMeaning::FatalErrorUnspecific));
    let ran = events(log)
        .into_iter()
        .find(|event| event.meaning == LogMeaning::ContentRanFor)
        .unwrap();
    assert_eq!(ran_for_seconds(ran.value.as_deref().unwrap()), Some(0));
}

#[test]
fn a_quiet_run_leaves_only_the_place_where_retroarch_gave_up() {
    let log = fixtures::RA_VULKAN_NO_DRIVER_QUIET;
    assert!(!verbose(log));
    assert_eq!(meanings(log), [LogMeaning::VideoDriverInitFailed]);
    assert_eq!(
        events(log)[0].signature,
        "retroarch.fatal.video_driver_init"
    );
}

#[test]
fn healthy_runs_report_their_driver_and_no_failure() {
    for (log, driver) in [
        (fixtures::RA_VULKAN_OK, "vulkan"),
        (fixtures::RA_GL_OK, "gl"),
    ] {
        let found = events(log);
        let ready = found
            .iter()
            .find(|event| event.meaning == LogMeaning::VideoDriverReady)
            .unwrap();
        assert_eq!(ready.value.as_deref(), Some(driver));
        for failure in [
            LogMeaning::VulkanUnavailable,
            LogMeaning::VideoDriverInitFailed,
            LogMeaning::CoreOpenFailed,
            LogMeaning::ContentReadFailed,
            LogMeaning::ContentRejected,
            LogMeaning::SaveWriteFailed,
            LogMeaning::FatalErrorUnspecific,
        ] {
            assert!(
                !found.iter().any(|event| event.meaning == failure),
                "{failure:?}"
            );
        }
        let ran = found
            .iter()
            .find(|event| event.meaning == LogMeaning::ContentRanFor)
            .unwrap();
        assert_eq!(ran_for_seconds(ran.value.as_deref().unwrap()), Some(4));
    }
    assert!(has(
        fixtures::RA_VULKAN_OK,
        LogMeaning::VulkanDeviceSelected
    ));
    assert!(has(fixtures::RA_GL_OK, LogMeaning::OpenGlRendererReported));
}

#[test]
fn an_ignored_core_is_recognised_even_though_the_run_looks_healthy() {
    let log = fixtures::RA_CORE_IGNORED;
    assert!(has(log, LogMeaning::CoreRequestIgnored));
    assert!(!has(log, LogMeaning::CoreLoadStarted));
    // The dummy core RetroArch falls back to still initialises video.
    assert!(has(log, LogMeaning::VideoDriverReady));
}

#[test]
fn core_and_content_failures_are_told_apart() {
    assert!(has(
        fixtures::RA_CORE_NOT_A_LIBRARY,
        LogMeaning::CoreOpenFailed
    ));
    assert!(has(
        fixtures::RA_CONTENT_GARBAGE,
        LogMeaning::ContentRejected
    ));
    assert!(!has(
        fixtures::RA_CONTENT_GARBAGE,
        LogMeaning::ContentReadFailed
    ));
    assert!(has(
        fixtures::RA_SAVE_DIR_READ_ONLY,
        LogMeaning::SaveWriteFailed
    ));
}

#[test]
fn a_missing_and_an_unreadable_game_file_print_the_same_message() {
    let missing = meanings(fixtures::RA_CONTENT_MISSING);
    let unreadable = meanings(fixtures::RA_CONTENT_UNREADABLE);
    assert!(missing.contains(&LogMeaning::ContentReadFailed));
    assert_eq!(missing, unreadable);
}

#[test]
fn a_silent_core_reports_a_zero_sample_rate_alongside_the_audio_failure() {
    let found = events(fixtures::RA_SILENT_CORE_AUDIO);
    assert!(
        found
            .iter()
            .any(|event| event.meaning == LogMeaning::AudioDriverInitFailed)
    );
    let output = found
        .iter()
        .find(|event| event.meaning == LogMeaning::CoreReportedOutput)
        .unwrap();
    assert_eq!(output.value.as_deref(), Some("0.00"));
}

#[test]
fn lines_printed_by_a_core_are_not_retroarch_events() {
    let log = "[libretro ERROR] [ERROR] Failed to create Vulkan instance.\n\
               [libretro INFO] Fatal error received in: \"video_driver_init_internal()\"\n";
    assert!(meanings(log).is_empty());
}

#[test]
fn a_core_can_forge_a_line_after_a_line_break_which_is_why_events_are_untrusted() {
    // What a core's log callback produces when its message embeds a newline.
    let log = "[libretro INFO] hello\n[ERROR] Failed to create Vulkan instance.\n";
    let mut set = EvidenceSet::default();
    let ids = record_log(&mut set, log.as_bytes(), LogStream::Stderr);
    assert_eq!(ids.len(), 1);
    assert_eq!(set.get(&ids[0]).unwrap().trust, Trust::UntrustedText);
}

#[test]
fn no_display_output_matches_no_retroarch_signature() {
    assert!(meanings(fixtures::RA_NO_DISPLAY).is_empty());
    assert!(!verbose(fixtures::RA_NO_DISPLAY));
}

#[test]
fn run_time_is_parsed_strictly() {
    assert_eq!(
        ran_for_seconds("00 hours, 00 minutes, 04 seconds."),
        Some(4)
    );
    assert_eq!(
        ran_for_seconds("01 hours, 02 minutes, 03 seconds."),
        Some(3_723)
    );
    assert_eq!(ran_for_seconds("4 seconds"), None);
    assert_eq!(ran_for_seconds("aa hours, 00 minutes, 04 seconds."), None);
    assert_eq!(ran_for_seconds(""), None);
}

fn config_values(set: &EvidenceSet) -> Vec<(String, Option<String>, Option<u32>)> {
    set.items()
        .iter()
        .filter_map(|item| match &item.fact {
            Fact::ConfigValue {
                key, value, line, ..
            } => Some((key.clone(), value.clone(), *line)),
            _ => None,
        })
        .collect()
}

#[test]
fn settings_are_read_with_their_line_and_a_content_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retroarch.cfg");
    std::fs::write(
        &path,
        "# comment\nvideo_driver = \"vulkan\"\naudio_driver = alsa # trailing\n\
         unrelated_key = \"1\"\n",
    )
    .unwrap();
    let mut set = EvidenceSet::default();
    let ids = record_config(&mut set, &path, ConfigLayer::Main);
    assert_eq!(ids.len(), DIAGNOSTIC_KEYS.len() + 1);
    let values = config_values(&set);
    assert_eq!(values.len(), DIAGNOSTIC_KEYS.len());
    assert!(values.contains(&("video_driver".into(), Some("vulkan".into()), Some(2))));
    assert!(values.contains(&("audio_driver".into(), Some("alsa".into()), Some(3))));
    assert!(values.contains(&("savefile_directory".into(), None, None)));
    assert!(!values.iter().any(|(key, ..)| key == "unrelated_key"));
    for item in set.items() {
        assert_eq!(item.trust, Trust::Measured);
        assert_eq!(
            item.fingerprint
                .as_ref()
                .unwrap()
                .sha256
                .as_deref()
                .map(str::len),
            Some(64)
        );
    }
}

#[test]
fn the_first_of_a_repeated_key_is_the_value_and_the_repeat_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retroarch.cfg");
    std::fs::write(
        &path,
        "video_driver = \"vulkan\"\n#include \"other.cfg\"\nvideo_driver = \"gl\"\nbroken line\n",
    )
    .unwrap();
    let mut set = EvidenceSet::default();
    record_config(&mut set, &path, ConfigLayer::Main);
    assert!(config_values(&set).contains(&("video_driver".into(), Some("vulkan".into()), Some(1))));
    let structure = set
        .items()
        .iter()
        .find_map(|item| match &item.fact {
            Fact::ConfigStructure {
                includes,
                malformed_lines,
                repeated_keys,
                ..
            } => Some((*includes, *malformed_lines, repeated_keys.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(structure, (1, 1, vec!["video_driver".to_string()]));
}

#[test]
fn a_settings_file_that_cannot_be_read_safely_yields_no_values() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.cfg");
    std::fs::write(&real, "video_driver = \"vulkan\"\n").unwrap();
    let link = dir.path().join("retroarch.cfg");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let binary = dir.path().join("binary.cfg");
    std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
    for path in [link, binary, dir.path().join("absent.cfg")] {
        let mut set = EvidenceSet::default();
        let ids = record_config(&mut set, &path, ConfigLayer::Main);
        assert_eq!(ids.len(), 1, "{}", path.display());
        assert!(matches!(
            set.items()[0].state,
            EvidenceState::Unavailable { .. }
        ));
        assert!(config_values(&set).is_empty());
    }
}

#[test]
fn hostile_setting_values_are_sanitised() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retroarch.cfg");
    std::fs::write(&path, "video_driver = \"vul\x1b[2Jkan\u{202E}\"\n").unwrap();
    let mut set = EvidenceSet::default();
    record_config(&mut set, &path, ConfigLayer::Main);
    let (_, value, _) = config_values(&set)
        .into_iter()
        .find(|(key, ..)| key == "video_driver")
        .unwrap();
    let value = value.unwrap();
    assert!(!value.contains('\x1b') && !value.contains('\u{202E}'));
    assert_ne!(value, "vulkan");
}
