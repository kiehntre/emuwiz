//! Behavioural coverage for the "why can't I play this?" journey.

use super::diagnosis::{Attempt, AttemptTracker, Cause, StartFailure, classify_start_failure};
use super::*;
use archivefs_core::launch::{LaunchBlocker, LaunchContentRef, LaunchPlanSummary};
use std::path::PathBuf;

fn blocked(
    kinds: &[LaunchBlockerKind],
    path: Option<&str>,
    firmware: FirmwareReadiness,
) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "duckstation",
            profile_id: "default".into(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: None,
            container: None,
            resolved_path: path.map(PathBuf::from),
            requires_mount: false,
            provenance: "synthetic".into(),
        },
        firmware,
        blockers: kinds
            .iter()
            .map(|kind| LaunchBlocker::new(*kind, format!("technical detail for {kind:?}")))
            .collect(),
        warnings: vec![],
        readiness: if kinds.is_empty() {
            LaunchReadiness::Ready
        } else {
            LaunchReadiness::Blocked
        },
        preference: CandidatePreference::Undetermined,
    }
}

fn plan(candidates: Vec<LaunchCandidate>) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan: LaunchPlan {
            platform_id: Some("PSX".into()),
            game_key: Some("SLUS-00000".into()),
            summary: LaunchPlanSummary {
                candidates: candidates.len(),
                ready: 0,
                ready_with_warnings: 0,
                blocked: candidates.len(),
            },
            candidates,
            media_topology: None,
        },
        retroarch: None,
        retroarch_scanned: true,
        standalone_scans_complete: true,
        dolphin: None,
        pcsx2: None,
        duckstation: None,
        ppsspp: None,
        rpcs3: None,
        xemu: None,
        xenia: None,
        melonds: None,
        cemu: None,
        flycast: None,
        xroar: None,
        azahar: None,
        vita3k: None,
        vice: None,
        amiga_whdload: None,
        #[cfg(target_os = "linux")]
        shadps4: None,
    }
}

fn summary_for(kinds: &[LaunchBlockerKind]) -> GameReadinessSummary {
    project(
        &plan(vec![blocked(
            kinds,
            Some("/g/game.bin"),
            FirmwareReadiness::NotRequired,
        )]),
        ReadinessFreshness::Current,
    )
}

fn render(
    summary: &GameReadinessSummary,
    size: [f32; 2],
    events: Vec<egui::Event>,
) -> Vec<(String, egui::Rect)> {
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show(ui, summary, None);
            });
        },
    );
    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(t) => out.push((
                t.galley.text().to_string(),
                t.galley.rect.translate(t.pos.to_vec2()),
            )),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn text_of(summary: &GameReadinessSummary) -> String {
    render(summary, [1280.0, 800.0], vec![])
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

const RAW_NAMES: &[&str] = &[
    "NoInstallationCandidate",
    "RequiredFirmwareMissing",
    "ContentNotResolved",
    "MediaTopologyBlocked",
    "DuckStationContentFormatUnsupported",
    "Blocker:",
];

#[test]
fn missing_emulator_is_explained_in_plain_words() {
    let mut empty = plan(vec![]);
    if let LaunchReadinessInput::Plan { plan, .. } = &mut empty {
        plan.candidates.clear();
    }
    let summary = project(&empty, ReadinessFreshness::Current);
    assert_eq!(summary.findings[0].cause, Cause::EmulatorMissing);
    let text = text_of(&summary);
    assert!(text.contains("No emulator is set up for this system"));
    assert!(text.contains("can't tell an uninstalled emulator from one it simply didn't detect"));
}

#[test]
fn emulator_problems_offer_emulator_setup_directly() {
    let summary = summary_for(&[LaunchBlockerKind::NoInstallationCandidate]);
    assert_eq!(summary.primary_action, Some(ReadinessAction::SetUpEmulator));
    assert!(text_of(&summary).contains("Set up emulator"));
    let moved = summary_for(&[LaunchBlockerKind::RetroArchExecutableMissing]);
    assert_eq!(moved.findings[0].cause, Cause::EmulatorSetupIncomplete);
    assert_eq!(moved.primary_action, Some(ReadinessAction::SetUpEmulator));
}

#[test]
fn missing_bios_explains_what_it_is_and_routes_to_bios_setup() {
    let summary = summary_for(&[LaunchBlockerKind::RequiredFirmwareMissing]);
    assert_eq!(summary.findings[0].cause, Cause::FirmwareMissing);
    let text = text_of(&summary);
    assert!(text.contains("startup software"));
    assert!(text.contains("legally obtained"));
    assert_eq!(summary.primary_action, Some(ReadinessAction::CheckFirmware));
}

#[test]
fn adapter_specific_firmware_blockers_are_still_firmware() {
    for kind in [
        LaunchBlockerKind::Rpcs3FirmwareUnavailable,
        LaunchBlockerKind::XRoarFirmwareMissing,
    ] {
        assert_eq!(
            summary_for(&[kind]).findings[0].cause,
            Cause::FirmwareMissing
        );
    }
}

#[test]
fn unsupported_game_format_says_the_file_itself_is_fine() {
    let summary = summary_for(&[LaunchBlockerKind::DuckStationContentFormatUnsupported]);
    assert_eq!(summary.findings[0].cause, Cause::FormatUnsupported);
    let text = text_of(&summary);
    assert!(text.contains("isn't supported for launching yet"));
    assert!(text.contains("nothing is wrong with it"));
}

#[test]
fn missing_or_moved_game_file_points_at_the_games_folder() {
    let summary = summary_for(&[LaunchBlockerKind::ContentNotResolved]);
    assert_eq!(summary.findings[0].cause, Cause::GameFileUnavailable);
    assert_eq!(
        summary.primary_action,
        Some(ReadinessAction::CheckGamesFolder)
    );
    assert!(text_of(&summary).contains("may have moved"));
}

#[test]
fn incomplete_or_contradictory_disc_set_routes_to_multi_disc_review() {
    let summary = summary_for(&[LaunchBlockerKind::MediaTopologyBlocked]);
    assert_eq!(summary.findings[0].cause, Cause::DiscSetProblem);
    assert_eq!(summary.primary_action, Some(ReadinessAction::ReviewDiscSet));
    assert!(text_of(&summary).contains("Review multi-disc set"));
}

#[test]
fn a_disc_set_that_needs_review_is_distinct_from_a_broken_one() {
    let review = summary_for(&[LaunchBlockerKind::MediaTopologyReviewRequired]);
    assert_eq!(review.findings[0].cause, Cause::DiscSetNeedsReview);
    assert_ne!(
        review.findings[0].title,
        summary_for(&[LaunchBlockerKind::MediaTopologyBlocked]).findings[0].title
    );
}

#[test]
fn unsupported_platform_is_not_confused_with_an_unknown_refusal() {
    let unsupported = summary_for(&[LaunchBlockerKind::DuckStationPlatformMismatch]);
    assert_eq!(unsupported.findings[0].cause, Cause::PlatformUnsupported);
    assert_eq!(unsupported.findings[0].fix, None);
    let unknown = summary_for(&[LaunchBlockerKind::CandidateBlocked]);
    assert_eq!(unknown.findings[0].cause, Cause::SafetyRefusal);
    assert!(text_of(&unknown).contains("can't determine more than that"));
}

#[test]
fn a_stale_result_offers_recheck_and_will_not_reuse_the_old_approval() {
    let summary = project(
        &plan(vec![blocked(
            &[],
            Some("/g/game.bin"),
            FirmwareReadiness::NotRequired,
        )]),
        ReadinessFreshness::Stale,
    );
    assert_eq!(
        summary.primary_action,
        Some(ReadinessAction::RecheckReadiness)
    );
    let text = text_of(&summary);
    assert!(text.contains("Recheck readiness"));
    assert!(text.contains("will not reuse an old approval"));
}

#[test]
fn launch_plan_safety_refusal_is_reported_as_a_refusal_not_a_failure() {
    let summary = summary_for(&[LaunchBlockerKind::CandidateBlocked]);
    assert_eq!(summary.findings[0].cause, Cause::SafetyRefusal);
    assert!(text_of(&summary).contains("declined to build a launch"));
    assert_eq!(
        classify_start_failure("Preflight(Refused)"),
        StartFailure::Refused
    );
}

#[test]
fn failed_process_start_is_distinct_from_a_readiness_refusal() {
    let mut summary = project(
        &plan(vec![blocked(
            &[],
            Some("/g/game.bin"),
            FirmwareReadiness::NotRequired,
        )]),
        ReadinessFreshness::Current,
    );
    summary.attempt = Some(Attempt::CouldNotStart {
        cause: classify_start_failure("Spawn(Os { code: 2, kind: NotFound, message: \"x\" })"),
        technical: "Spawn(Os { code: 2, kind: NotFound })".into(),
    });
    let text = text_of(&summary);
    assert!(text.contains("The emulator could not be started."));
    assert!(text.contains("may not be where EmuWiz expected it"));
    assert!(text.contains("not a confirmed diagnosis"));
    for claim in ["crashed", "failed internally", "unsupported version"] {
        assert!(!text.contains(claim), "must not claim: {claim}");
    }
    assert!(text.contains("EmuWiz did not change your game files"));
    assert!(!text.contains("NotFound"));
    assert_eq!(
        classify_start_failure("Os { kind: PermissionDenied }"),
        StartFailure::PermissionDenied
    );
}

#[test]
fn an_emulator_that_closes_immediately_is_described_without_guessing() {
    let mut tracker = AttemptTracker::default();
    tracker.started();
    tracker.finished(Some(7), None);
    let attempt = tracker
        .for_game(7)
        .cloned()
        .expect("quick exit is remembered");
    assert!(matches!(attempt, Attempt::EndedQuickly { .. }));
    assert!(
        tracker.for_game(8).is_none(),
        "never shown for another game"
    );
    let mut summary = summary_for(&[]);
    summary.attempt = Some(attempt);
    let text = text_of(&summary);
    assert!(text.contains("The emulator closed shortly after it was started."));
    assert!(text.contains("EmuWiz cannot yet tell why it closed."));
    for claim in [
        "crashed",
        "failed",
        "exit reason",
        "confirm a normal launch",
    ] {
        assert!(!text.contains(claim), "must not claim: {claim}");
    }
    assert!(attempt_details(&summary).contains("not a backend result"));
}

#[test]
fn primary_text_never_shows_internal_names_but_details_keep_them() {
    let summary = summary_for(&[
        LaunchBlockerKind::NoInstallationCandidate,
        LaunchBlockerKind::RequiredFirmwareMissing,
        LaunchBlockerKind::ContentNotResolved,
    ]);
    let text = text_of(&summary);
    for raw in RAW_NAMES {
        assert!(!text.contains(raw), "primary UI leaked {raw}");
    }
    assert!(!text.contains("technical detail for"));
    let technical: Vec<_> = summary
        .findings
        .iter()
        .flat_map(|finding| finding.technical.iter())
        .collect();
    assert!(
        technical
            .iter()
            .any(|line| line.contains("RequiredFirmwareMissing"))
    );
}

#[test]
fn blocked_and_ready_states_say_checking_changes_nothing() {
    assert!(
        text_of(&summary_for(&[LaunchBlockerKind::NoInstallationCandidate]))
            .contains("Your game was not changed")
    );
    let ready = project(
        &plan(vec![blocked(
            &[],
            Some("/g/game.bin"),
            FirmwareReadiness::NotRequired,
        )]),
        ReadinessFreshness::Current,
    );
    assert!(text_of(&ready).contains("does not change your game files"));
}

#[test]
fn several_blockers_are_all_listed_up_front_most_actionable_first() {
    let summary = summary_for(&[
        LaunchBlockerKind::RequiredFirmwareMissing,
        LaunchBlockerKind::NoInstallationCandidate,
        LaunchBlockerKind::ContentNotResolved,
    ]);
    assert_eq!(summary.findings.len(), 3);
    assert_eq!(summary.findings[0].cause, Cause::GameFileUnavailable);
    let text = text_of(&summary);
    assert!(text.contains("3 things need attention"));
    for finding in &summary.findings {
        assert!(text.contains(finding.title));
    }
}

#[test]
fn once_the_blocker_is_resolved_the_card_is_ready_to_play() {
    let ready = project(
        &plan(vec![blocked(
            &[],
            Some("/g/game.bin"),
            FirmwareReadiness::NotRequired,
        )]),
        ReadinessFreshness::Current,
    );
    assert_eq!(ready.status, ReadinessPresentationState::Ready);
    assert_eq!(ready.primary_action, Some(ReadinessAction::Play));
    assert!(ready.findings.is_empty());
    assert!(text_of(&ready).contains("EmuWiz can now prepare this game for DuckStation"));
}

#[test]
fn escape_does_not_get_swallowed_or_trigger_an_action() {
    let summary = summary_for(&[LaunchBlockerKind::NoInstallationCandidate]);
    let escape = vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }];
    let before = render(&summary, [1280.0, 800.0], vec![]).len();
    let after = render(&summary, [1280.0, 800.0], escape).len();
    assert_eq!(
        before, after,
        "the inline card is non-modal and ignores Escape"
    );
}

#[test]
fn compact_window_keeps_the_explanation_and_fix_within_the_width() {
    let summary = summary_for(&[
        LaunchBlockerKind::RequiredFirmwareMissing,
        LaunchBlockerKind::ContentNotResolved,
    ]);
    let rendered = render(&summary, [700.0, 520.0], vec![]);
    for needle in ["Check games folder", "2 things need attention"] {
        let (_, rect) = rendered
            .iter()
            .find(|(text, _)| text.contains(needle))
            .unwrap_or_else(|| panic!("{needle} missing at 700x520"));
        assert!(rect.max.x <= 700.0, "{needle} runs past the window edge");
    }
}

fn attempt_details(summary: &GameReadinessSummary) -> String {
    summary
        .attempt
        .as_ref()
        .map(Attempt::technical)
        .unwrap_or_default()
}

#[test]
fn an_unrecognised_start_error_gets_no_guessed_cause() {
    let attempt = Attempt::CouldNotStart {
        cause: classify_start_failure("something unexpected"),
        technical: "something unexpected".into(),
    };
    assert_eq!(attempt.hint(), None);
    assert!(attempt.why().contains("did not start"));
    assert_eq!(attempt.technical(), "something unexpected");
}

// ---- exact words for blockers whose meaning is known precisely ----

fn blocked_on(adapter_id: &'static str, kinds: &[LaunchBlockerKind]) -> GameReadinessSummary {
    let mut candidate = blocked(kinds, Some("/g/game"), FirmwareReadiness::NotRequired);
    candidate.target = LaunchTarget::Standalone {
        adapter_id,
        profile_id: "default".into(),
        profile_path: None,
    };
    project(&plan(vec![candidate]), ReadinessFreshness::Current)
}

#[test]
fn an_xbox_game_that_is_not_a_disc_image_is_told_what_xemu_needs() {
    let summary = blocked_on("xemu", &[LaunchBlockerKind::XemuContentFormatUnsupported]);
    let finding = &summary.findings[0];
    assert_eq!(finding.cause, Cause::FormatUnsupported);
    assert_eq!(
        finding.title,
        "xemu can only start an Xbox disc image (.iso)"
    );
    let text = text_of(&summary);
    assert!(text.contains("ZIP archive"));
    assert!(text.contains("Your file is fine and nothing was changed"));
    assert!(text.contains("Extract or convert it to an .iso disc image"));
    assert!(!text.contains("isn't supported for launching yet"));
    assert_eq!(
        summary.primary_action,
        Some(ReadinessAction::CheckGamesFolder)
    );
}

#[test]
fn a_ps3_install_package_is_told_to_install_it_in_rpcs3() {
    let summary = blocked_on("rpcs3", &[LaunchBlockerKind::Rpcs3ContentFormatUnsupported]);
    assert_eq!(summary.findings[0].cause, Cause::FormatUnsupported);
    assert_eq!(
        summary.findings[0].title,
        "RPCS3 can't start this PS3 file directly"
    );
    let text = text_of(&summary);
    assert!(text.contains("A .pkg file is an installer, not the game itself"));
    assert!(text.contains("File > Install Packages/Raps/Edats"));
    assert!(!text.contains("isn't supported for launching yet"));
}

#[test]
fn rpcs3_without_confirmed_system_software_names_the_file_and_the_menu() {
    let summary = blocked_on("rpcs3", &[LaunchBlockerKind::Rpcs3FirmwareUnavailable]);
    assert_eq!(summary.findings[0].cause, Cause::FirmwareMissing);
    let text = text_of(&summary);
    assert!(text.contains("RPCS3 needs the PlayStation 3 system software"));
    assert!(text.contains("PS3UPDAT.PUP"));
    assert!(text.contains("File > Install Firmware"));
    assert_eq!(summary.primary_action, Some(ReadinessAction::CheckFirmware));
}

/// These three were grouped by name as "EmuWiz can't safely prepare this
/// launch": a missing Kickstart was reported as an unexplained refusal.
#[test]
fn a_missing_amiga_kickstart_is_firmware_with_its_own_fix_not_a_safety_refusal() {
    for kind in [
        LaunchBlockerKind::FsUaeKickstartUnavailable,
        LaunchBlockerKind::AmiberryKickstartUnavailable,
        LaunchBlockerKind::WhdloadKickstartUnavailable,
    ] {
        let summary = blocked_on("fsuae", &[kind]);
        assert_eq!(summary.findings.len(), 1, "{kind:?}");
        assert_eq!(
            summary.findings[0].cause,
            Cause::FirmwareMissing,
            "{kind:?}"
        );
        assert_eq!(
            summary.findings[0].title,
            "The Amiga emulator has no Kickstart ROM it can use"
        );
        let text = text_of(&summary);
        assert!(text.contains("the kickstart_file setting or its Kickstarts folder"));
        assert!(text.contains("EmuWiz never supplies one"));
        assert!(
            !text.contains("can't safely prepare this launch"),
            "{kind:?}"
        );
        assert_eq!(summary.primary_action, Some(ReadinessAction::CheckFirmware));
    }
}

#[test]
fn amiga_ipf_and_missing_settings_point_at_the_emulator_setup() {
    for (kind, title, phrase) in [
        (
            LaunchBlockerKind::FsUaeIpfBackendUnavailable,
            "This disk image (.ipf) needs the emulator's IPF plug-in",
            "CAPS, also called capsimg",
        ),
        (
            LaunchBlockerKind::AmiberryIpfBackendUnavailable,
            "This disk image (.ipf) needs the emulator's IPF plug-in",
            "Your file is fine and nothing was changed",
        ),
        (
            LaunchBlockerKind::FsUaeProfileRequired,
            "The Amiga emulator has no settings EmuWiz can use",
            "save a configuration with your Amiga model and Kickstart",
        ),
        (
            LaunchBlockerKind::AmiberryProfileRequired,
            "The Amiga emulator has no settings EmuWiz can use",
            "Open the emulator once",
        ),
        (
            LaunchBlockerKind::WhdloadProfileMissing,
            "The Amiga emulator has no settings EmuWiz can use",
            "then check again",
        ),
    ] {
        let summary = blocked_on("fsuae", &[kind]);
        assert_eq!(
            summary.findings[0].cause,
            Cause::EmulatorSetupIncomplete,
            "{kind:?}"
        );
        assert_eq!(summary.findings[0].title, title, "{kind:?}");
        let text = text_of(&summary);
        assert!(text.contains(phrase), "{kind:?}");
        assert!(
            !text.contains("can't safely prepare this launch"),
            "{kind:?}"
        );
        assert_eq!(summary.primary_action, Some(ReadinessAction::SetUpEmulator));
    }
}

#[test]
fn xemu_system_files_are_named_and_other_emulators_keep_the_general_words() {
    let xemu = blocked_on("xemu", &[LaunchBlockerKind::RequiredFirmwareMissing]);
    assert_eq!(xemu.findings[0].title, "xemu needs its Xbox system files");
    let text = text_of(&xemu);
    assert!(text.contains("the MCPX boot ROM, a flash BIOS, an EEPROM and a hard disk image"));
    assert_eq!(xemu.primary_action, Some(ReadinessAction::CheckFirmware));

    let other = summary_for(&[LaunchBlockerKind::RequiredFirmwareMissing]);
    assert_eq!(
        other.findings[0].title,
        "Console startup software (BIOS / firmware) is needed"
    );

    // The firmware state alone (no named blocker) reads the same way for xemu.
    let mut candidate = blocked(
        &[LaunchBlockerKind::XemuBindingUnavailable],
        Some("/g/game.iso"),
        FirmwareReadiness::Missing,
    );
    candidate.target = LaunchTarget::Standalone {
        adapter_id: "xemu",
        profile_id: "default".into(),
        profile_path: None,
    };
    let state_only = project(&plan(vec![candidate]), ReadinessFreshness::Current);
    assert!(
        state_only
            .findings
            .iter()
            .any(|finding| finding.title == "xemu needs its Xbox system files")
    );
}

#[test]
fn blockers_of_one_cause_share_one_finding_and_the_exact_words_win_in_either_order() {
    for kinds in [
        [
            LaunchBlockerKind::FsUaeKickstartUnavailable,
            LaunchBlockerKind::RequiredFirmwareMissing,
        ],
        [
            LaunchBlockerKind::RequiredFirmwareMissing,
            LaunchBlockerKind::FsUaeKickstartUnavailable,
        ],
    ] {
        let summary = blocked_on("fsuae", &kinds);
        assert_eq!(summary.findings.len(), 1, "{kinds:?}");
        assert_eq!(
            summary.findings[0].title,
            "The Amiga emulator has no Kickstart ROM it can use"
        );
        assert_eq!(summary.findings[0].technical.len(), 2);
    }
}
