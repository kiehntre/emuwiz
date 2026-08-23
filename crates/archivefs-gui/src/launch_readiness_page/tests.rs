//! Tests for the Launch Readiness panel renderer and its "Launch RetroArch"
//! state machine.
//!
//! Every [`LaunchCandidate`]/[`LaunchPlan`] fixture is built by hand (all
//! fields are public) - this module is testing the renderer's own text and
//! honesty, not re-deriving [`archivefs_core::launch::build_launch_plan`]'s
//! own logic, which already has its own dedicated test suite in
//! `archivefs-core`. Tests that exercise [`RetroArchLaunchState`] itself
//! reach into its private fields directly (this `tests` module is a
//! descendant of `launch_readiness_page`, so that is ordinary Rust
//! visibility, not a workaround) rather than simulating real egui pointer
//! clicks, which the rest of this app's test suites also never do.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use archivefs_core::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use archivefs_core::launch::{
    LaunchBlocker, LaunchBlockerKind, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    LaunchPlanSummary, LaunchWarning, LaunchWarningKind, RetroArchCommand,
    RetroArchCommandSelection, spawn_retroarch,
};

use super::*;

fn retroarch_profile() -> ProfileRef {
    ProfileRef {
        profile_kind: ProfileKind::Native,
        scope: ProfileScope::User,
    }
}

fn retroarch_target() -> LaunchTarget {
    LaunchTarget::RetroArchCore {
        profile: retroarch_profile(),
        core_stem: "mednafen_psx_hw".to_string(),
        platform_id: "PSX",
    }
}

fn resolved_content(path: &str) -> LaunchContentRef {
    LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(PathBuf::from(path)),
        requires_mount: false,
        provenance: "loose/direct content: the archive record's own path is the runnable file"
            .to_string(),
    }
}

fn unresolved_archive_content() -> LaunchContentRef {
    LaunchContentRef {
        kind: None,
        container: Some(LaunchContainerKind::Archive),
        resolved_path: None,
        requires_mount: true,
        provenance: "archive is mounted, but no specific inner member has been resolved yet"
            .to_string(),
    }
}

fn ready_candidate() -> LaunchCandidate {
    LaunchCandidate {
        target: retroarch_target(),
        content: resolved_content("/library/Game.bin"),
        firmware: FirmwareReadiness::NotRequired,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn plan_with(candidates: Vec<LaunchCandidate>) -> LaunchPlan {
    let ready = candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::Ready)
        .count();
    let ready_with_warnings = candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::ReadyWithWarnings)
        .count();
    let blocked = candidates
        .iter()
        .filter(|candidate| candidate.readiness == LaunchReadiness::Blocked)
        .count();
    LaunchPlan {
        platform_id: Some("PSX".to_string()),
        game_key: Some("SLUS-00001".to_string()),
        summary: LaunchPlanSummary {
            candidates: candidates.len(),
            ready,
            ready_with_warnings,
            blocked,
        },
        candidates,
    }
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

fn render(input: &LaunchReadinessInput) -> egui::FullOutput {
    render_with_state(input, &mut RetroArchLaunchState::default())
}

fn render_with_state(
    input: &LaunchReadinessInput,
    state: &mut RetroArchLaunchState,
) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_launch_readiness_panel(ui, input, state);
        });
    })
}

/// Spawns a real, short-lived process via the same [`spawn_retroarch`] core
/// entry point production code uses - never a shell string, an explicit
/// executable plus argv, exactly like [`RetroArchCommand`] requires.
fn spawn_test_process(executable: &str, arguments: &[&str]) -> LaunchedRetroArchProcess {
    let command = RetroArchCommand {
        executable: PathBuf::from(executable),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).into())
            .collect(),
        working_directory: None,
        selection: RetroArchCommandSelection {
            profile: retroarch_profile(),
            core_stem: "mednafen_psx_hw".to_string(),
            platform_id: "PSX".to_string(),
            core_library: PathBuf::from("/does/not/matter/core.so"),
            content_path: PathBuf::from("/library/Game.bin"),
        },
    };
    spawn_retroarch(command).expect("spawning the fixture test process must succeed")
}

fn wait_until_exited(process: &mut LaunchedRetroArchProcess) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline, "fixture process never exited");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_until_not_starting(state: &mut RetroArchLaunchState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        state.poll();
        if !matches!(
            state.tracked,
            Some((_, RetroArchLaunchStage::Starting { .. }))
        ) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "launch worker never reported a result"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// --- evidence-not-loaded prerequisite ---------------------------------------

#[test]
fn evidence_not_loaded_shows_the_exact_prerequisite_message_and_no_plan() {
    let output = render(&LaunchReadinessInput::EvidenceNotLoaded);
    assert!(rendered_text_contains(
        &output,
        "Load ROM Identity & Evidence first."
    ));
    assert!(!rendered_text_contains(&output, "No launch options found"));
}

// --- RetroArch-not-scanned prerequisite -------------------------------------

#[test]
fn retroarch_not_scanned_shows_the_exact_prerequisite_message() {
    let output = render(&LaunchReadinessInput::RetroArchNotScanned);
    assert!(rendered_text_contains(
        &output,
        "Scan RetroArch profiles to check installed cores."
    ));
}

// --- unknown identity --------------------------------------------------------

#[test]
fn unknown_identity_shows_an_honest_unresolved_message_never_no_emulator_found() {
    let output = render(&LaunchReadinessInput::IdentityUnknown);
    assert!(rendered_text_contains(&output, "Identity unresolved"));
    assert!(rendered_text_contains(&output, "could not be verified"));
    assert!(!rendered_text_contains(&output, "No emulator found"));
}

// --- conflicting identity ----------------------------------------------------

#[test]
fn conflicting_identity_shows_a_conflict_message() {
    let output = render(&LaunchReadinessInput::IdentityConflicting);
    assert!(rendered_text_contains(&output, "Identity conflicts"));
    assert!(rendered_text_contains(&output, "conflicts"));
}

// --- no candidate / NoInstallationCandidate blocker -------------------------

#[test]
fn no_candidates_shows_the_empty_state_not_a_blocked_candidate_card() {
    let plan = plan_with(Vec::new());
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(rendered_text_contains(&output, "No launch options found"));
}

#[test]
fn no_installation_candidate_blocker_kind_exists_and_is_distinct_from_unresolved_identity() {
    // Not itself rendered by a literal string match (the renderer shows
    // `blocker.detail`, not the `LaunchBlockerKind` variant name) - this
    // proves the kind used elsewhere in this suite really is the
    // "no candidate" reason, not accidentally `IdentityUnresolved`/
    // `ContentNotResolved`.
    let blocker = LaunchBlocker::new(
        LaunchBlockerKind::NoInstallationCandidate,
        "no installed RetroArch core supports this platform",
    );
    assert_eq!(blocker.kind, LaunchBlockerKind::NoInstallationCandidate);
    assert_ne!(blocker.kind, LaunchBlockerKind::IdentityUnresolved);
}

// --- ready RetroArch candidate -----------------------------------------------

#[test]
fn ready_retroarch_candidate_shows_ready_badge() {
    let plan = plan_with(vec![ready_candidate()]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(rendered_text_contains(&output, "Ready"));
    assert!(rendered_text_contains(&output, "mednafen_psx_hw"));
}

// --- firmware-blocked candidate ----------------------------------------------

#[test]
fn firmware_blocked_candidate_shows_missing_firmware_and_blocked_status() {
    let mut candidate = ready_candidate();
    candidate.firmware = FirmwareReadiness::Missing;
    candidate.readiness = LaunchReadiness::Blocked;
    candidate.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::RequiredFirmwareMissing,
        "required firmware is missing",
    ));

    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(rendered_text_contains(&output, "Blocked"));
    assert!(rendered_text_contains(&output, "Missing"));
    assert!(rendered_text_contains(
        &output,
        "required firmware is missing"
    ));
}

// --- warnings render ----------------------------------------------------------

#[test]
fn warnings_render_their_detail_text() {
    let mut candidate = ready_candidate();
    candidate.readiness = LaunchReadiness::ReadyWithWarnings;
    candidate.warnings.push(LaunchWarning::new(
        LaunchWarningKind::MultipleEligibleProfiles,
        "more than one eligible core exists",
    ));

    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(rendered_text_contains(&output, "Ready with warnings"));
    assert!(rendered_text_contains(
        &output,
        "more than one eligible core exists"
    ));
}

// --- profile/core name renders ------------------------------------------------

#[test]
fn profile_and_core_name_render() {
    let plan = plan_with(vec![ready_candidate()]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(rendered_text_contains(&output, "mednafen_psx_hw"));
    assert!(rendered_text_contains(&output, "RetroArch"));
}

// --- preference label renders -------------------------------------------------

#[test]
fn every_preference_variant_has_its_own_label() {
    for (preference, label) in [
        (CandidatePreference::Remembered, "Remembered"),
        (CandidatePreference::SoleEligible, "Sole eligible"),
        (CandidatePreference::Undetermined, "Choice needed"),
    ] {
        let mut candidate = ready_candidate();
        candidate.preference = preference;
        let plan = plan_with(vec![candidate]);
        let output = render(&LaunchReadinessInput::Plan(plan));
        assert!(
            rendered_text_contains(&output, label),
            "expected label {label:?} for {preference:?}"
        );
    }
}

// --- archive outer path is never treated as runnable content ----------------

#[test]
fn unresolved_archive_content_never_shows_the_outer_archive_path_as_resolved() {
    let mut candidate = ready_candidate();
    candidate.content = unresolved_archive_content();
    candidate.readiness = LaunchReadiness::Blocked;
    candidate.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::ContentNotResolved,
        "content is inside a container that has not been mounted, so no runnable path exists \
         yet",
    ));

    assert!(!candidate.content.has_runnable_path());
    assert_eq!(candidate.content.resolved_path, None);

    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(rendered_text_contains(&output, "Blocked"));
    assert!(rendered_text_contains(
        &output,
        "content is inside a container that has not been mounted"
    ));
}

// --- section heading -----------------------------------------------------------

#[test]
fn section_heading_and_subtitle_render() {
    let output = render(&LaunchReadinessInput::EvidenceNotLoaded);
    assert!(rendered_text_contains(&output, "Launch readiness"));
    assert!(rendered_text_contains(
        &output,
        "Ways this game can be played."
    ));
}

// --- Launch RetroArch button eligibility ------------------------------------

#[test]
fn strict_native_ready_candidate_shows_launch_retroarch_button() {
    let plan = plan_with(vec![ready_candidate()]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(rendered_text_contains(&output, "Launch RetroArch"));
}

#[test]
fn ready_with_warnings_candidate_has_no_launch_button() {
    let mut candidate = ready_candidate();
    candidate.readiness = LaunchReadiness::ReadyWithWarnings;
    candidate.warnings.push(LaunchWarning::new(
        LaunchWarningKind::MultipleEligibleProfiles,
        "more than one eligible core exists",
    ));
    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(!rendered_text_contains(&output, "Launch RetroArch"));
}

#[test]
fn blocked_candidate_has_no_launch_button() {
    let mut candidate = ready_candidate();
    candidate.readiness = LaunchReadiness::Blocked;
    candidate.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::RequiredFirmwareMissing,
        "required firmware is missing",
    ));
    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(!rendered_text_contains(&output, "Launch RetroArch"));
}

#[test]
fn flatpak_profile_candidate_has_no_launch_button() {
    let mut candidate = ready_candidate();
    candidate.target = LaunchTarget::RetroArchCore {
        profile: ProfileRef {
            profile_kind: ProfileKind::Flatpak,
            scope: ProfileScope::User,
        },
        core_stem: "mednafen_psx_hw".to_string(),
        platform_id: "PSX",
    };
    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(!rendered_text_contains(&output, "Launch RetroArch"));
}

#[test]
fn appimage_profile_candidate_has_no_launch_button() {
    let mut candidate = ready_candidate();
    candidate.target = LaunchTarget::RetroArchCore {
        profile: ProfileRef {
            profile_kind: ProfileKind::AppImage,
            scope: ProfileScope::User,
        },
        core_stem: "mednafen_psx_hw".to_string(),
        platform_id: "PSX",
    };
    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(!rendered_text_contains(&output, "Launch RetroArch"));
}

#[test]
fn standalone_candidate_has_no_launch_button() {
    let mut candidate = ready_candidate();
    candidate.target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "default".to_string(),
        profile_path: None,
    };
    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(!rendered_text_contains(&output, "Launch RetroArch"));
}

#[test]
fn mounted_archive_content_candidate_has_no_launch_button() {
    let mut candidate = ready_candidate();
    candidate.content = unresolved_archive_content();
    let plan = plan_with(vec![candidate]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(!rendered_text_contains(&output, "Launch RetroArch"));
}

// --- request facts sent to core ----------------------------------------------

#[test]
fn eligible_candidate_yields_the_exact_expected_retroarch_launch_request_facts() {
    let plan = plan_with(vec![ready_candidate()]);
    let candidate = &plan.candidates[0];
    let request =
        retroarch_launch_request(&plan, candidate).expect("this candidate is Phase-1 eligible");

    assert_eq!(
        request.selected_content_path,
        PathBuf::from("/library/Game.bin")
    );
    assert_eq!(request.expected_platform_id, "PSX");
    assert_eq!(request.expected_game_key, "SLUS-00001");
    assert_eq!(request.profile, retroarch_profile());
    assert_eq!(request.core_stem, "mednafen_psx_hw");
}

#[test]
fn ineligible_candidates_never_yield_a_launch_request() {
    let plan = plan_with(vec![ready_candidate()]);

    let mut ready_with_warnings = ready_candidate();
    ready_with_warnings.readiness = LaunchReadiness::ReadyWithWarnings;
    assert!(retroarch_launch_request(&plan, &ready_with_warnings).is_none());

    let mut mounted = ready_candidate();
    mounted.content = unresolved_archive_content();
    assert!(retroarch_launch_request(&plan, &mounted).is_none());

    let mut flatpak = ready_candidate();
    flatpak.target = LaunchTarget::RetroArchCore {
        profile: ProfileRef {
            profile_kind: ProfileKind::Flatpak,
            scope: ProfileScope::User,
        },
        core_stem: "mednafen_psx_hw".to_string(),
        platform_id: "PSX",
    };
    assert!(retroarch_launch_request(&plan, &flatpak).is_none());
}

// --- Starting disables a duplicate click -------------------------------------

#[test]
fn starting_stage_shows_a_starting_label_and_no_active_launch_button() {
    let plan = plan_with(vec![ready_candidate()]);
    let candidate = &plan.candidates[0];
    let request = retroarch_launch_request(&plan, candidate).unwrap();
    let key = RetroArchLaunchKey::from_request(&request);
    let (_sender, receiver) = mpsc::channel();
    let mut state = RetroArchLaunchState {
        tracked: Some((key, RetroArchLaunchStage::Starting { receiver })),
    };

    let output = render_with_state(&LaunchReadinessInput::Plan(plan), &mut state);
    assert!(rendered_text_contains(&output, "Starting RetroArch…"));
    assert!(!rendered_text_contains(&output, "Launch RetroArch"));
}

// --- Running shows PID --------------------------------------------------------

#[test]
fn running_stage_shows_running_status_and_pid() {
    let plan = plan_with(vec![ready_candidate()]);
    let candidate = &plan.candidates[0];
    let request = retroarch_launch_request(&plan, candidate).unwrap();
    let key = RetroArchLaunchKey::from_request(&request);
    let process = spawn_test_process("/bin/sleep", &["1"]);
    let pid = process.pid;
    let mut state = RetroArchLaunchState {
        tracked: Some((key, RetroArchLaunchStage::Running { process })),
    };

    let output = render_with_state(&LaunchReadinessInput::Plan(plan), &mut state);
    assert!(rendered_text_contains(&output, "RetroArch running"));
    assert!(rendered_text_contains(&output, &pid.to_string()));
    assert!(!rendered_text_contains(&output, "Stop"));
    assert!(!rendered_text_contains(&output, "Kill"));
}

// --- process exit ---------------------------------------------------------

#[test]
fn clean_process_exit_enters_exited_and_shows_a_clean_exit_message() {
    let plan = plan_with(vec![ready_candidate()]);
    let candidate = &plan.candidates[0];
    let request = retroarch_launch_request(&plan, candidate).unwrap();
    let key = RetroArchLaunchKey::from_request(&request);
    let mut process = spawn_test_process("/bin/true", &[]);
    wait_until_exited(&mut process);
    let mut state = RetroArchLaunchState {
        tracked: Some((key, RetroArchLaunchStage::Exited { process })),
    };

    let output = render_with_state(&LaunchReadinessInput::Plan(plan), &mut state);
    assert!(rendered_text_contains(&output, "RetroArch exited"));
    assert!(rendered_text_contains(&output, "closed normally"));
    assert!(!rendered_text_contains(&output, "Launch failed"));
    // A relaunch is a fresh explicit click, never automatic - the button
    // must still be present (and enabled) after a clean exit.
    assert!(rendered_text_contains(&output, "Launch RetroArch"));
}

#[test]
fn non_zero_exit_shows_bounded_stderr_behind_technical_details() {
    let plan = plan_with(vec![ready_candidate()]);
    let candidate = &plan.candidates[0];
    let request = retroarch_launch_request(&plan, candidate).unwrap();
    let key = RetroArchLaunchKey::from_request(&request);
    let mut process = spawn_test_process(
        "/bin/sh",
        &["-c", "echo launch-failure-marker-9f31 >&2; exit 3"],
    );
    wait_until_exited(&mut process);
    let mut state = RetroArchLaunchState {
        tracked: Some((key, RetroArchLaunchStage::Exited { process })),
    };

    let output = render_with_state(&LaunchReadinessInput::Plan(plan), &mut state);
    assert!(rendered_text_contains(
        &output,
        "RetroArch exited unexpectedly"
    ));
    assert!(rendered_text_contains(&output, "Technical details"));
    // Collapsed by default - the raw stderr must not be visible until the
    // user expands the disclosure (matches
    // `ui::components::technical_details_hides_its_body_until_expanded`).
    assert!(!rendered_text_contains(
        &output,
        "launch-failure-marker-9f31"
    ));
}

// --- preflight/spawn error --------------------------------------------------

#[test]
fn preflight_failure_enters_failed_with_a_gamer_facing_message_never_no_emulator_installed() {
    // A content path that cannot possibly exist forces preflight to reject
    // the request (`ContentNotFound`) - this exercises the real
    // `RetroArchLaunchState::start` worker end-to-end, not a hand-built
    // `Failed` stage. The candidate (not just the request afterwards) must
    // carry this path, so the key the render call re-derives from `plan`
    // matches the key `state.start` tracked it under.
    let mut candidate = ready_candidate();
    candidate.content = resolved_content("/nonexistent/launch-readiness-test-fixture-9f31.bin");
    let plan = plan_with(vec![candidate]);
    let request = retroarch_launch_request(&plan, &plan.candidates[0]).unwrap();
    let key = RetroArchLaunchKey::from_request(&request);

    let mut state = RetroArchLaunchState::default();
    state.start(request);
    wait_until_not_starting(&mut state);
    assert!(matches!(
        state.tracked,
        Some((ref tracked_key, RetroArchLaunchStage::Failed { .. })) if *tracked_key == key
    ));

    let output = render_with_state(&LaunchReadinessInput::Plan(plan), &mut state);
    assert!(rendered_text_contains(&output, "Launch failed"));
    assert!(rendered_text_contains(
        &output,
        "This game's file is no longer available where it was last seen."
    ));
    assert!(!rendered_text_contains(&output, "no emulator installed"));
    assert!(!rendered_text_contains(&output, "No emulator installed"));
    assert!(rendered_text_contains(&output, "Technical details"));
}

// --- stale selection handling -------------------------------------------------

#[test]
fn changing_selected_candidate_never_shows_another_selections_running_process_as_its_own() {
    let mut other_candidate = ready_candidate();
    other_candidate.content = resolved_content("/library/OtherGame.bin");
    let other_plan = plan_with(vec![other_candidate]);
    let other_request = retroarch_launch_request(&other_plan, &other_plan.candidates[0]).unwrap();
    let other_key = RetroArchLaunchKey::from_request(&other_request);
    let process = spawn_test_process("/bin/sleep", &["1"]);
    let mut state = RetroArchLaunchState {
        tracked: Some((other_key, RetroArchLaunchStage::Running { process })),
    };

    // A different candidate (a different game's content path) is rendered
    // while that other tracker is still Running.
    let plan = plan_with(vec![ready_candidate()]);
    let output = render_with_state(&LaunchReadinessInput::Plan(plan), &mut state);

    assert!(
        !rendered_text_contains(&output, "RetroArch running"),
        "a different selection must never display another selection's running process"
    );
    assert!(
        rendered_text_contains(&output, "Launch RetroArch"),
        "the newly displayed, untracked candidate must show its own idle Launch button"
    );
    // The other selection's process is still tracked, not dropped/killed -
    // it keeps getting reaped safely in the background.
    assert!(state.tracked.is_some());
}

// --- no shell command construction, no kill/stop action ----------------------

#[test]
fn show_launch_readiness_panel_takes_no_command_or_process_handle_parameter() {
    // Locks the exact signature: the only inputs are the already-built
    // plan/prerequisite `LaunchReadinessInput` and this module's own launch
    // tracker - never a command string, argv, or `std::process` type. If a
    // future change widened this signature to accept one, this line would
    // fail to compile.
    fn assert_signature(_: fn(&mut egui::Ui, &LaunchReadinessInput, &mut RetroArchLaunchState)) {}
    assert_signature(show_launch_readiness_panel);
}
