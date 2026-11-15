use super::*;
use crate::daedalus::evidence::{
    ConfigLayer, EvidenceArea, EvidenceSet, ExitOutcome, Fact, LogStream, Subject, Trust,
};
use crate::daedalus::rules::diagnose;
use crate::daedalus::test_fixtures as fixtures;
use crate::daedalus::{process, retroarch, saves};

fn launch_of(emulator: &str, code: i32, output: Option<&str>) -> EvidenceSet {
    let mut set = EvidenceSet::new(Subject {
        emulator: Some(emulator.into()),
        game: None,
    });
    process::record_exit(&mut set, ExitOutcome::Exited { code }, None, None);
    if let Some(output) = output {
        if emulator == retroarch::EMULATOR {
            retroarch::record_log(&mut set, output.as_bytes(), LogStream::Stderr);
        }
        process::record_stderr(&mut set, output.as_bytes(), emulator, &[]);
    }
    set
}

fn notice_for(set: &EvidenceSet) -> LaunchNotice {
    launch_notice(&diagnose(set), &saves::assess(set), "RetroArch")
}

fn all_text(notice: &LaunchNotice) -> String {
    let mut text = notice.headline.clone();
    for line in &notice.lines {
        text.push('\n');
        text.push_str(line);
    }
    for extra in [&notice.next_step, &notice.save_coverage]
        .into_iter()
        .flatten()
    {
        text.push('\n');
        text.push_str(extra);
    }
    if let Some(warning) = &notice.save_warning {
        for part in [&warning.headline, &warning.explanation, &warning.next_step] {
            text.push('\n');
            text.push_str(part);
        }
    }
    text
}

#[test]
fn a_failed_launch_is_prominent_and_says_what_is_known_and_how_firmly() {
    let mut set = launch_of("retroarch", 1, Some(fixtures::RA_VULKAN_NO_DRIVER));
    set.observe(
        EvidenceArea::EmulatorConfiguration,
        Trust::Measured,
        "RetroArch settings file",
        Fact::ConfigValue {
            emulator: "retroarch".into(),
            layer: ConfigLayer::Main,
            key: "video_driver".into(),
            value: Some("vulkan".into()),
            line: Some(1),
        },
    );
    let notice = notice_for(&set);
    assert_eq!(notice.level, NoticeLevel::LaunchFailed);
    assert!(notice.level.is_prominent());
    // Emulator output alone: a possible cause, said to be possible.
    assert!(
        notice
            .headline
            .starts_with("The launch failed. A possible cause: "),
        "{}",
        notice.headline
    );
    assert!(!notice.lines.is_empty() && notice.lines.len() <= MAX_LINES);
    assert!(
        notice
            .lines
            .iter()
            .any(|line| line.ends_with("(as reported).")),
        "{:?}",
        notice.lines
    );
    assert!(notice.next_step.is_some());
    // The launch failed before RetroArch got as far as saving, so nothing
    // shows that a failed save could have been seen: no clean bill.
    assert_eq!(
        notice.save_coverage.as_deref(),
        Some("Save check: not available for this launch.")
    );
}

#[test]
fn an_unexplained_failure_says_that_it_is_unexplained() {
    let notice = notice_for(&launch_of("retroarch", 1, None));
    assert_eq!(notice.level, NoticeLevel::LaunchFailed);
    assert_eq!(
        notice.headline,
        "The launch failed, and the evidence does not show why"
    );
    assert_eq!(notice.lines, ["The emulator exited with status 1"]);
    assert_eq!(
        notice.next_step.as_deref(),
        Some("Open Game Doctor for what was and was not examined.")
    );
    assert_eq!(
        notice.save_coverage.as_deref(),
        Some("Save check: not available for this launch.")
    );
}

#[test]
fn a_clean_exit_with_a_stated_error_is_shown_as_a_report_not_as_a_failed_process() {
    // RetroArch ignores a bad core path, draws a blank frame and exits 0.
    let notice = notice_for(&launch_of("retroarch", 0, Some(fixtures::RA_CORE_IGNORED)));
    assert_eq!(notice.level, NoticeLevel::LaunchFailed);
    assert!(
        notice
            .headline
            .starts_with("RetroArch reported an error that stops a game from starting"),
        "{}",
        notice.headline
    );
    assert!(!notice.headline.contains("The launch failed"));
}

#[test]
fn a_save_that_could_not_be_written_is_prominent_although_the_launch_went_well() {
    let set = launch_of("retroarch", 0, Some(fixtures::RA_SAVE_DIR_READ_ONLY));
    assert_eq!(
        diagnose(&set).launch,
        crate::daedalus::rules::LaunchOutcome::NoFailureObserved
    );
    let notice = notice_for(&set);
    assert_eq!(notice.level, NoticeLevel::SaveWriteFailed);
    assert!(notice.level.is_prominent());
    assert_eq!(notice.headline, "Your progress may not have been saved");
    assert_eq!(notice.lines.len(), 1);
    assert!(
        notice.lines[0].starts_with("RetroArch reported that it could not write this game's save."),
        "{}",
        notice.lines[0]
    );
    assert!(
        notice
            .next_step
            .as_deref()
            .unwrap()
            .contains("EmuWiz has not changed anything")
    );
    assert_eq!(notice.save_coverage, None);
    // Nothing the emulator printed is in the notice.
    assert!(!all_text(&notice).contains("/scratch"));
}

#[test]
fn an_ordinary_launch_is_quiet_and_never_says_that_saves_were_written() {
    let notice = notice_for(&launch_of("retroarch", 0, Some(fixtures::RA_SAVE_OK)));
    assert_eq!(notice.level, NoticeLevel::Quiet);
    assert!(!notice.level.is_prominent());
    assert!(notice.headline.is_empty());
    assert!(notice.lines.is_empty());
    assert_eq!(notice.next_step, None);
    let coverage = notice.save_coverage.as_deref().unwrap();
    assert!(
        coverage.starts_with("Save check: no error reported for in-game (battery) saves."),
        "{coverage}"
    );
    assert!(coverage.contains("Not checked: save states"), "{coverage}");
    let text = all_text(&notice).to_lowercase();
    for claim in [
        "saved successfully",
        "was saved",
        "were saved",
        "saves are safe",
    ] {
        assert!(!text.contains(claim), "{claim:?} in {text}");
    }
}

#[test]
fn an_emulator_with_no_save_monitoring_gets_one_quiet_line_saying_so() {
    for emulator in ["pcsx2", "duckstation", "flycast", "atari800"] {
        let notice = launch_notice(
            &diagnose(&launch_of(emulator, 0, Some("all good\n"))),
            &saves::assess(&launch_of(emulator, 0, Some("all good\n"))),
            "The emulator",
        );
        assert_eq!(notice.level, NoticeLevel::Quiet, "{emulator}");
        assert!(notice.headline.is_empty() && notice.lines.is_empty());
        assert_eq!(
            notice.save_coverage.as_deref(),
            Some("Save check: not available for this emulator."),
            "{emulator}"
        );
    }
}

#[test]
fn a_failed_launch_that_also_lost_a_save_reports_both_and_keeps_them_apart() {
    let output = format!(
        "{}[ERROR] [SRAM]: Failed to save SRAM.\n",
        fixtures::RA_VULKAN_NO_DRIVER
    );
    let notice = notice_for(&launch_of("retroarch", 1, Some(&output)));
    assert_eq!(notice.level, NoticeLevel::LaunchFailed);
    assert!(notice.lines.len() <= MAX_LINES);
    // The lost save has its own place, with its own next step, so the cap on
    // the launch's lines can never cut it and it is said once.
    let warning = notice.save_warning.as_ref().expect("a save warning");
    assert_eq!(warning.headline, "Your progress may not have been saved");
    assert!(
        warning.explanation.starts_with("RetroArch reported"),
        "{}",
        warning.explanation
    );
    assert!(!warning.next_step.is_empty());
    assert!(
        notice.lines.iter().all(|line| !line.contains("save")),
        "{:?}",
        notice.lines
    );
    assert!(!notice.headline.contains("save"), "{}", notice.headline);
    // What is said about the launch is still there, and its next step is
    // about the launch, not the save.
    assert!(
        notice
            .lines
            .iter()
            .any(|line| line.contains("could not set up Vulkan")),
        "{:?}",
        notice.lines
    );
    assert_ne!(
        notice.next_step.as_deref(),
        Some(warning.next_step.as_str())
    );
    assert_eq!(notice.save_coverage, None);

    // A notice that is itself the save warning does not repeat it.
    let only_saves = notice_for(&launch_of(
        "retroarch",
        0,
        Some("[ERROR] [SRAM]: Failed to save SRAM.\n"),
    ));
    assert_eq!(only_saves.level, NoticeLevel::SaveWriteFailed);
    assert_eq!(only_saves.save_warning, None);
}

#[test]
fn a_cause_shown_while_the_evidence_disagrees_carries_its_own_standing() {
    let set = launch_of("retroarch", 1, Some(fixtures::RA_VULKAN_NO_DRIVER));
    let explained = diagnose(&set);
    let cause = explained
        .causes
        .iter()
        .find(|cause| cause.effect != Effect::Degrades)
        .expect("a cause that stops the launch")
        .clone();
    for verdict in [Verdict::Conflicting, Verdict::Unknown] {
        let mut report = explained.clone();
        report.verdict = verdict;
        let notice = launch_notice(&report, &saves::assess(&set), "RetroArch");
        assert!(
            !notice.headline.contains(&cause.title),
            "{}",
            notice.headline
        );
        assert_eq!(
            notice.lines[0],
            format!(
                "One {} cause: {}. {}",
                cause.standing(),
                cause.title,
                cause.explanation
            ),
            "{verdict:?}"
        );
    }
}

#[test]
fn a_problem_that_only_degrades_a_game_that_ran_stays_off_the_play_screen() {
    // A core that makes sound, and RetroArch's audio driver failing: found,
    // shown in Game Doctor, and not a banner on Play.
    let output = "[INFO] [Core]: Geometry: 160x144, Aspect: 1.111, FPS: 59.73, Sample rate: \
                  44100.00 Hz.\n\
                  [ERROR] Failed to initialize audio driver. Will continue without audio.\n";
    let set = launch_of("retroarch", 0, Some(output));
    assert!(!diagnose(&set).causes.is_empty());
    let notice = notice_for(&set);
    assert_eq!(notice.level, NoticeLevel::Quiet);
    assert!(notice.headline.is_empty() && notice.lines.is_empty());
}
