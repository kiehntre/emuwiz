use super::*;
use crate::daedalus::evidence::{ExitOutcome, LogStream, PathAccess, Subject};
use crate::daedalus::process;
use crate::daedalus::retroarch;
use crate::daedalus::rules::{LaunchOutcome, diagnose};
use crate::daedalus::test_fixtures as fixtures;

fn launch_of(emulator: &str, exit_code: i32, output: Option<&str>) -> EvidenceSet {
    let mut set = EvidenceSet::new(Subject {
        emulator: Some(emulator.into()),
        game: None,
    });
    process::record_exit(
        &mut set,
        ExitOutcome::Exited { code: exit_code },
        None,
        None,
    );
    if let Some(output) = output {
        if emulator == retroarch::EMULATOR {
            retroarch::record_log(&mut set, output.as_bytes(), LogStream::Stderr);
        }
        process::record_stderr(&mut set, output.as_bytes(), emulator, &[]);
    }
    set
}

fn launch(exit_code: i32, output: Option<&str>) -> EvidenceSet {
    launch_of(retroarch::EMULATOR, exit_code, output)
}

fn problem_of(assessment: &SaveWriteAssessment) -> SaveWriteProblem {
    *assessment
        .problem()
        .unwrap_or_else(|| panic!("no problem in {assessment:?}"))
}

fn save_folder(set: &mut EvidenceSet, presence: PathPresence, writable: Option<bool>) {
    set.observe(
        EvidenceArea::Filesystem,
        Trust::Measured,
        "read-only look at the path",
        Fact::PathAccess(PathAccess {
            role: PathRole::SaveDirectory,
            path: "/scratch/saves".into(),
            presence,
            readable: Some(true),
            writable,
            role_supplied: false,
        }),
    );
}

const BATTERY_ONLY: &[SaveMechanism] = &[SaveMechanism::BatterySave];
const NOT_BATTERY: &[SaveMechanism] =
    &[SaveMechanism::SaveState, SaveMechanism::EmulatorOwnedFiles];

#[test]
fn a_game_that_ran_and_could_not_save_is_a_write_failure_and_not_a_failed_launch() {
    // Real output: RetroArch 1.21.0, save folder read-only, exit status 0.
    let mut set = launch(0, Some(fixtures::RA_SAVE_DIR_READ_ONLY));
    assert_eq!(diagnose(&set).launch, LaunchOutcome::NoFailureObserved);
    let assessment = assess(&set);
    assert!(assessment.is_write_failure());
    assert!(matches!(assessment.status, SaveWriteStatus::WriteFailed(_)));
    let problem = problem_of(&assessment);
    assert_eq!(problem.standing, SaveWriteStanding::ReportedByEmulator);
    assert!(problem.standing.is_write_error());
    assert_eq!(problem.mechanism, SaveMechanism::BatterySave);
    assert_eq!(problem.times, 1);
    assert_eq!(problem.folder, SaveFolderFinding::NotKnown);
    assert!(problem.recovery_copy_reported);
    assert_eq!(assessment.capture, SaveCapture::Complete);
    assert_eq!(assessment.saves_reported_written, 0);
    // A problem is shown instead of a coverage line.
    assert_eq!(assessment.coverage_line(), None);
    let explanation = problem.explanation("RetroArch");
    assert!(
        explanation.starts_with("RetroArch reported that it could not write this game's save."),
        "{explanation}"
    );
    assert!(explanation.contains("RetroArch-recovery"), "{explanation}");
    // Nothing from the log is in the wording: no path, no line.
    assert!(!explanation.contains("/scratch"), "{explanation}");
    assert!(
        problem
            .next_step()
            .contains("EmuWiz has not changed anything")
    );

    // EmuWiz's own look at the folder agrees: confirmed by two sources.
    save_folder(&mut set, PathPresence::Directory, Some(false));
    let problem = problem_of(&assess(&set));
    assert_eq!(problem.standing, SaveWriteStanding::ConfirmedByCheck);
    assert_eq!(problem.folder, SaveFolderFinding::NotWritable);
    assert!(
        problem
            .explanation("RetroArch")
            .contains("it cannot be written to")
    );
    assert!(problem.next_step().contains("permissions"));
}

#[test]
fn the_folder_check_changes_the_next_step_and_never_overrides_what_was_reported() {
    let failed = "[INFO] [SRAM]: Saving RAM type #0 to \"/scratch/saves/game.srm\".\n\
                  [ERROR] [SRAM]: Failed to save SRAM.\n";
    // Writable folder: still a reported write failure; the reason is elsewhere.
    let mut set = launch(0, Some(failed));
    save_folder(&mut set, PathPresence::Directory, Some(true));
    let problem = problem_of(&assess(&set));
    assert_eq!(problem.standing, SaveWriteStanding::ReportedByEmulator);
    assert_eq!(problem.folder, SaveFolderFinding::Writable);
    assert!(problem.next_step().contains("free space"));

    let mut set = launch(0, Some(failed));
    save_folder(&mut set, PathPresence::Missing, None);
    let problem = problem_of(&assess(&set));
    assert_eq!(problem.standing, SaveWriteStanding::ConfirmedByCheck);
    assert_eq!(problem.folder, SaveFolderFinding::Missing);
    assert!(problem.next_step().contains("does not exist"));

    // A link or something unexpected where the folder should be: not known.
    let mut set = launch(0, Some(failed));
    save_folder(&mut set, PathPresence::SymbolicLink, None);
    let problem = problem_of(&assess(&set));
    assert_eq!(problem.standing, SaveWriteStanding::ReportedByEmulator);
    assert_eq!(problem.folder, SaveFolderFinding::NotKnown);
}

#[test]
fn an_unwritable_save_folder_with_no_report_is_a_risk_not_a_write_failure() {
    let mut set = launch(0, Some(fixtures::RA_GL_OK));
    save_folder(&mut set, PathPresence::Directory, Some(false));
    let assessment = assess(&set);
    assert!(matches!(assessment.status, SaveWriteStatus::AtRisk(_)));
    assert!(!assessment.is_write_failure());
    let problem = problem_of(&assessment);
    assert_eq!(problem.standing, SaveWriteStanding::Suspected);
    assert!(!problem.standing.is_write_error());
    assert_eq!(problem.times, 0);
    assert_eq!(
        problem.headline(),
        "Progress may not be saved for this game"
    );
    let explanation = problem.explanation("RetroArch");
    assert!(!explanation.contains("reported"), "{explanation}");
    assert!(
        explanation.contains("cannot be written to"),
        "{explanation}"
    );

    // The folder check does not depend on a provider: it holds for an
    // emulator whose output EmuWiz cannot read.
    let mut set = launch_of("pcsx2", 0, None);
    save_folder(&mut set, PathPresence::Directory, Some(false));
    assert!(matches!(assess(&set).status, SaveWriteStatus::AtRisk(_)));
}

#[test]
fn repeated_messages_are_one_problem_with_a_count() {
    let mut output = String::from("[INFO] [Core]: Loading dynamic libretro core\n");
    for _ in 0..7 {
        output.push_str("[ERROR] [SRAM]: Failed to save SRAM.\n");
        output.push_str("[WARN] [SRAM]: Attempting to recover ...\n");
    }
    let problem = problem_of(&assess(&launch(0, Some(&output))));
    assert_eq!(problem.times, 7);
    assert!(
        problem
            .explanation("RetroArch")
            .starts_with("RetroArch reported 7 times that"),
    );
    assert!(!problem.recovery_copy_reported);
}

#[test]
fn ordinary_messages_about_saving_are_never_read_as_a_failure() {
    // Real output of a successful save, and messages that merely mention
    // saving, failing or SRAM.
    let ordinary = "[INFO] [Overrides]: Redirecting save file to \"/scratch/saves/2048.srm\".\n\
        [INFO] [Overrides]: Redirecting save state to \"/scratch/states/2048.state\".\n\
        [INFO] [SRAM]: Skipping SRAM load.\n\
        [INFO] [SRAM]: Saving RAM type #0 to \"/scratch/saves/2048.srm\".\n\
        [INFO] [SRAM]: Saved successfully to \"/scratch/saves/2048.srm\".\n\
        [INFO] [State]: Core does not support save states.\n\
        [INFO] [Core]: Saved core options file to \"/scratch/config/2048/2048.opt\".\n\
        [ERROR] [DBus]: Failed to suspend screensaver via DBus.\n\
        [libretro INFO] [ERROR] [SRAM]: Failed to save SRAM.\n\
        [INFO] the game said: [ERROR] [SRAM]: Failed to save SRAM.\n\
        [ERROR] [SRAM]: Failed to save SRAM. (not really)\n\
        [WARN] [SRAM]: Failed to save SRAM maybe\n";
    let assessment = assess(&launch(0, Some(ordinary)));
    assert_eq!(assessment.status, SaveWriteStatus::NoFailureDetected);
    assert_eq!(assessment.problem(), None);
    assert_eq!(assessment.saves_reported_written, 1);
    // The whole healthy capture, as it really came out.
    let healthy = assess(&launch(0, Some(fixtures::RA_SAVE_OK)));
    assert_eq!(healthy.status, SaveWriteStatus::NoFailureDetected);
    assert_eq!(healthy.saves_reported_written, 1);
}

#[test]
fn a_launch_with_no_detected_error_says_what_was_checked_and_never_that_saves_were_written() {
    let assessment = assess(&launch(0, Some(fixtures::RA_SAVE_OK)));
    assert_eq!(assessment.status, SaveWriteStatus::NoFailureDetected);
    // Being RetroArch covers battery saves and nothing else.
    assert_eq!(assessment.monitored, BATTERY_ONLY);
    assert_eq!(assessment.not_monitored, NOT_BATTERY);
    let line = assessment.coverage_line().unwrap();
    assert_eq!(
        line,
        "Save check: no error reported for in-game (battery) saves. Not checked: save \
         states and save files the emulator writes by itself."
    );
    let detail = assessment.coverage_detail().join("\n");
    assert!(
        detail.contains("it is not proof that a save was written"),
        "{detail}"
    );
    assert!(
        detail.contains("Not checked: save states and save files"),
        "{detail}"
    );
    assert!(
        detail.contains("A launch that went well does not show that saves were written."),
        "{detail}"
    );
    assert!(detail.contains("That is its own report."), "{detail}");
    for text in [line.as_str(), detail.as_str()] {
        for claim in [
            "saved successfully",
            "were saved",
            "was saved",
            "saves are safe",
        ] {
            assert!(!text.to_lowercase().contains(claim), "{claim:?} in {text}");
        }
    }
}

#[test]
fn save_states_are_not_monitored_even_when_retroarch_talks_about_them() {
    // Real lines about save states. None is a failed write, and none makes
    // save states a monitored mechanism.
    let states = "[INFO] [Overrides]: Redirecting save state to \"/scratch/states/game.state\".\n\
        [INFO] [State]: Core does not support save states.\n\
        [INFO] [Core]: Content ran for a total of: 00 hours, 00 minutes, 04 seconds.\n";
    // Talk about save states is not the start of a battery save either:
    // on its own it shows nothing was being watched.
    let assessment = assess(&launch(0, Some(states)));
    assert_eq!(
        assessment.status,
        SaveWriteStatus::MonitoringUnavailable(MonitoringGap::SavePhaseNotSeen)
    );
    assert!(assessment.monitored.is_empty());
    // With RetroArch's own line that it is starting a battery save, battery
    // saves are watched. Save states still are not.
    let with_save = format!(
        "{states}[INFO] [SRAM]: Saving RAM type #0 to \"/scratch/saves/game.srm\".\n\
         [INFO] [SRAM]: Saved successfully to \"/scratch/saves/game.srm\".\n"
    );
    let assessment = assess(&launch(0, Some(&with_save)));
    assert_eq!(assessment.status, SaveWriteStatus::NoFailureDetected);
    assert_eq!(assessment.monitored, BATTERY_ONLY);
    assert!(assessment.not_monitored.contains(&SaveMechanism::SaveState));
    // Text that looks like a failed save state is not a signature: no real
    // one has been captured, so none is recognised.
    let unrecognised = "[INFO] [Core]: Loading\n[ERROR] [State]: Failed to save state to \"/scratch/states/game.state\".\n";
    let assessment = assess(&launch(0, Some(unrecognised)));
    assert!(assessment.problem().is_none(), "{assessment:?}");
    assert!(assessment.not_monitored.contains(&SaveMechanism::SaveState));
    assert_eq!(observable_mechanisms("retroarch"), Some(BATTERY_ONLY));
}

#[test]
fn a_standalone_emulator_has_no_save_monitoring_whatever_its_exit_status() {
    for emulator in [
        "pcsx2",
        "duckstation",
        "flycast",
        "atari800",
        "dolphin",
        "RetroArch",
    ] {
        assert_eq!(observable_mechanisms(emulator), None, "{emulator}");
        for output in [
            None,
            Some("[INFO] everything is fine\nMemory card saved successfully\n"),
            Some("[ERROR] [SRAM]: Failed to save SRAM.\n"),
        ] {
            let assessment = assess(&launch_of(emulator, 0, output));
            assert_eq!(
                assessment.status,
                SaveWriteStatus::MonitoringUnavailable(MonitoringGap::NoProvider),
                "{emulator} {output:?}"
            );
            assert!(assessment.monitored.is_empty());
            assert_eq!(assessment.not_monitored, SaveMechanism::ALL);
            assert_eq!(
                assessment.coverage_line().as_deref(),
                Some("Save check: not available for this emulator.")
            );
            let detail = assessment.coverage_detail().join("\n");
            assert!(
                detail.contains("cannot yet read this emulator's output"),
                "{detail}"
            );
            assert!(
                detail.contains("does not show that saves were written"),
                "{detail}"
            );
        }
    }
    // Evidence that does not say which emulator it is about has no provider.
    let mut set = EvidenceSet::default();
    process::record_exit(&mut set, ExitOutcome::Exited { code: 0 }, None, None);
    assert_eq!(
        assess(&set).status,
        SaveWriteStatus::MonitoringUnavailable(MonitoringGap::NoProvider)
    );
}

#[test]
fn missing_quiet_or_cut_output_is_monitoring_unavailable_not_a_clean_result() {
    // No output at all.
    let none = assess(&launch(0, None));
    assert_eq!(
        none.status,
        SaveWriteStatus::MonitoringUnavailable(MonitoringGap::NoOutput)
    );
    assert_eq!(none.capture, SaveCapture::Missing);
    assert!(none.monitored.is_empty());
    assert_eq!(
        none.coverage_line().as_deref(),
        Some("Save check: not available for this launch.")
    );

    // Real quiet output of a launch whose save failed: RetroArch prints
    // nothing about it without detailed logging.
    let quiet = assess(&launch(0, Some(fixtures::RA_SAVE_DIR_READ_ONLY_QUIET)));
    assert_eq!(
        quiet.status,
        SaveWriteStatus::MonitoringUnavailable(MonitoringGap::NotDetailed)
    );
    assert!(quiet.monitored.is_empty());
    assert!(
        quiet
            .coverage_detail()
            .join("\n")
            .contains("could not have been seen")
    );

    // Detailed output that was cut: the end, where saves are written, may
    // be the part that is missing.
    let mut cut = fixtures::RA_GL_OK.to_string();
    cut.push_str(&format!(
        "{}4096{}\n",
        crate::launch::process_spawn::OUTPUT_OMITTED_MARKER_PREFIX,
        crate::launch::process_spawn::OUTPUT_OMITTED_MARKER_SUFFIX
    ));
    let cut = assess(&launch(0, Some(&cut)));
    assert_eq!(
        cut.status,
        SaveWriteStatus::MonitoringUnavailable(MonitoringGap::CutShort)
    );
    assert_eq!(cut.capture, SaveCapture::CutShort);
    assert!(
        cut.coverage_detail()
            .join("\n")
            .contains("could have been missed")
    );

    // A failure that is in a cut capture is still a failure.
    let mut cut_with_failure = fixtures::RA_SAVE_DIR_READ_ONLY.to_string();
    cut_with_failure.push_str(&format!(
        "{}4096{}\n",
        crate::launch::process_spawn::OUTPUT_OMITTED_MARKER_PREFIX,
        crate::launch::process_spawn::OUTPUT_OMITTED_MARKER_SUFFIX
    ));
    assert!(assess(&launch(0, Some(&cut_with_failure))).is_write_failure());

    // Complete detailed output with nothing about a failed save.
    let complete = assess(&launch(0, Some(fixtures::RA_GL_OK)));
    assert_eq!(complete.status, SaveWriteStatus::NoFailureDetected);
    assert_eq!(complete.capture, SaveCapture::Complete);
}

#[test]
fn a_failed_launch_keeps_its_own_outcome_and_its_save_finding_side_by_side() {
    let output = format!(
        "{}[ERROR] [SRAM]: Failed to save SRAM.\n",
        fixtures::RA_VULKAN_NO_DRIVER
    );
    let set = launch(1, Some(&output));
    assert_eq!(diagnose(&set).launch, LaunchOutcome::Failed);
    let assessment = assess(&set);
    assert!(assessment.is_write_failure());
    assert_eq!(
        problem_of(&assessment).standing,
        SaveWriteStanding::ReportedByEmulator
    );
}

#[test]
fn asserted_evidence_never_raises_a_save_warning() {
    let mut set = launch(0, Some(fixtures::RA_GL_OK));
    set.observe(
        EvidenceArea::Filesystem,
        Trust::Asserted,
        "AI suggestion",
        Fact::PathAccess(PathAccess {
            role: PathRole::SaveDirectory,
            path: "/scratch/saves".into(),
            presence: PathPresence::Directory,
            readable: Some(true),
            writable: Some(false),
            role_supplied: false,
        }),
    );
    assert_eq!(assess(&set).status, SaveWriteStatus::NoFailureDetected);
}

#[test]
fn every_provider_row_is_for_an_emulator_with_captured_save_messages() {
    // The table is the record of what can be observed. Growing it is a
    // decision that needs a real capture, so the test pins it.
    assert_eq!(PROVIDERS, &[("retroarch", BATTERY_ONLY)]);
    assert_eq!(SaveMechanism::ALL.len(), 3);
}

/// "No failure reported" is only said when something shows that a failed
/// save would have been reported in the wording that is looked for.
#[test]
fn a_clean_bill_needs_proof_that_a_failed_save_would_have_been_seen() {
    let not_available = |output: &str, exit: i32| {
        let assessment = assess(&launch(exit, Some(output)));
        assert_eq!(
            assessment.status,
            SaveWriteStatus::MonitoringUnavailable(MonitoringGap::SavePhaseNotSeen),
            "{output}"
        );
        assert!(assessment.monitored.is_empty(), "{output}");
        assert_eq!(
            assessment.coverage_line().as_deref(),
            Some("Save check: not available for this launch.")
        );
        assert!(
            assessment
                .coverage_detail()
                .join("\n")
                .contains("could not have been recognised"),
            "{output}"
        );
    };
    // A real launch that died while starting its video driver: detailed,
    // complete output, and RetroArch never got as far as saving.
    not_available(fixtures::RA_VULKAN_NO_DRIVER, 1);
    // RetroArch set to another language keeps its tags and translates its
    // messages. A failed save is then not in the wording that is known.
    // (Made-up lines: no such capture exists.)
    not_available(
        "[INFO] [Core]: Cargando.\n\
         [INFO] [SRAM]: Guardando RAM tipo #0 en \"/scratch/saves/game.srm\".\n\
         [ERROR] [SRAM]: Error al guardar SRAM.\n",
        0,
    );
    // The log sent to a file: only what was printed before that reaches
    // EmuWiz.
    not_available(
        "[INFO] [Config]: Looking for config in: \"/scratch/retroarch.cfg\".\n\
         [INFO] [Core]: Loading dynamic libretro core from: \"/scratch/core.so\"\n",
        0,
    );
    // The same start-up lines followed by RetroArch's own line that it is
    // starting a save: now a failed write would have been seen.
    let saved = assess(&launch(0, Some(fixtures::RA_SAVE_OK)));
    assert_eq!(saved.status, SaveWriteStatus::NoFailureDetected);
    assert_eq!(saved.monitored, BATTERY_ONLY);
}
