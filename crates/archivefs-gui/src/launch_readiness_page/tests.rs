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

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use archivefs_core::dat::firmware_evidence::{FirmwareIdentityRecord, FirmwareSystem};
use archivefs_core::dat::model::DatEcosystem;
use archivefs_core::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use archivefs_core::identity_source::hashing::Crc32;
use archivefs_core::launch::{
    DolphinCommand, DolphinCommandSelection, LaunchBlocker, LaunchBlockerKind, LaunchContainerKind,
    LaunchContentKind, LaunchContentRef, LaunchPlanSummary, LaunchWarning, LaunchWarningKind,
    Pcsx2Command, Pcsx2CommandSelection, RetroArchCommand, RetroArchCommandSelection,
    spawn_dolphin, spawn_pcsx2, spawn_retroarch,
};
use archivefs_core::patch_manager::{
    DolphinLocalProfileDiscovery, DolphinNativeLaunchBinding, DolphinUserDirectoryMode,
    Pcsx2ProfileDiscoveryRoots, Pcsx2UserDirectoryMode, discover_dolphin_local_profiles,
    discover_pcsx2_profiles,
};

use super::*;

static NEXT_DOLPHIN_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_PCSX2_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

fn plan_input(plan: LaunchPlan) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan,
        dolphin: None,
        pcsx2: None,
    }
}

fn dolphin_plan_input(plan: LaunchPlan, context: DolphinLaunchContext) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan,
        dolphin: Some(context),
        pcsx2: None,
    }
}

fn pcsx2_plan_input(plan: LaunchPlan, context: Pcsx2LaunchContext) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan,
        dolphin: None,
        pcsx2: Some(context),
    }
}

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
    render_with_states(
        input,
        &mut RetroArchLaunchState::default(),
        &mut DolphinLaunchState::default(),
        &mut Pcsx2LaunchState::default(),
    )
}

fn render_with_state(
    input: &LaunchReadinessInput,
    state: &mut RetroArchLaunchState,
) -> egui::FullOutput {
    render_with_states(
        input,
        state,
        &mut DolphinLaunchState::default(),
        &mut Pcsx2LaunchState::default(),
    )
}

fn render_with_dolphin_state(
    input: &LaunchReadinessInput,
    state: &mut DolphinLaunchState,
) -> egui::FullOutput {
    render_with_states(
        input,
        &mut RetroArchLaunchState::default(),
        state,
        &mut Pcsx2LaunchState::default(),
    )
}

fn render_with_pcsx2_state(
    input: &LaunchReadinessInput,
    state: &mut Pcsx2LaunchState,
) -> egui::FullOutput {
    render_with_states(
        input,
        &mut RetroArchLaunchState::default(),
        &mut DolphinLaunchState::default(),
        state,
    )
}

fn render_with_states(
    input: &LaunchReadinessInput,
    retroarch_state: &mut RetroArchLaunchState,
    dolphin_state: &mut DolphinLaunchState,
    pcsx2_state: &mut Pcsx2LaunchState,
) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_launch_readiness_panel(ui, input, retroarch_state, dolphin_state, pcsx2_state);
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
        let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
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
    let output = render(&plan_input(plan));
    assert!(!rendered_text_contains(&output, "Launch RetroArch"));
}

#[test]
fn mounted_archive_content_candidate_has_no_launch_button() {
    let mut candidate = ready_candidate();
    candidate.content = unresolved_archive_content();
    let plan = plan_with(vec![candidate]);
    let output = render(&plan_input(plan));
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

    let output = render_with_state(&plan_input(plan), &mut state);
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

    let output = render_with_state(&plan_input(plan), &mut state);
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

    let output = render_with_state(&plan_input(plan), &mut state);
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

    let output = render_with_state(&plan_input(plan), &mut state);
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

    let output = render_with_state(&plan_input(plan), &mut state);
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
    let output = render_with_state(&plan_input(plan), &mut state);

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
    fn assert_signature(
        _: fn(
            &mut egui::Ui,
            &LaunchReadinessInput,
            &mut RetroArchLaunchState,
            &mut DolphinLaunchState,
            &mut Pcsx2LaunchState,
        ),
    ) {
    }
    assert_signature(show_launch_readiness_panel);
}

// ---------------------------------------------------------------------------
// Launch Dolphin fixtures
// ---------------------------------------------------------------------------

struct DolphinFixture {
    root: PathBuf,
}

impl DolphinFixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_DOLPHIN_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-gui-dolphin-launch-{label}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for DolphinFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A structurally valid, minimal Dolphin disc header - the only bytes
/// `inspect_catalogued_game_identity` ever reads for a direct ISO.
fn gamecube_iso_bytes(game_id: &[u8; 6]) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x20];
    bytes[..6].copy_from_slice(game_id);
    bytes[6] = 1;
    bytes[0x1c..0x20].copy_from_slice(&[0xc2, 0x33, 0x9f, 0x3d]);
    bytes
}

fn base_dolphin_roots(fixture: &DolphinFixture) -> DolphinLocalDiscoveryRoots {
    DolphinLocalDiscoveryRoots {
        home: fixture.path("home"),
        xdg_config_home: fixture.path("config"),
        xdg_data_home: fixture.path("data"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
        dolphin_emu_userpath_override: None,
    }
}

fn empty_dolphin_roots() -> DolphinLocalDiscoveryRoots {
    DolphinLocalDiscoveryRoots {
        home: PathBuf::from("/nonexistent-dolphin-launch-readiness-home"),
        xdg_config_home: PathBuf::from("/nonexistent-dolphin-launch-readiness-config"),
        xdg_data_home: PathBuf::from("/nonexistent-dolphin-launch-readiness-data"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
        dolphin_emu_userpath_override: None,
    }
}

/// No discovered profile at all - used by negative eligibility tests that
/// are rejected before `dolphin_launch_request` ever needs to look one up
/// (readiness/target/content checks all run first), so no filesystem
/// fixture is needed for them.
fn empty_dolphin_context() -> DolphinLaunchContext {
    DolphinLaunchContext {
        discovery: DolphinLocalProfileDiscovery {
            profiles: Vec::new(),
            complete: true,
        },
        roots: empty_dolphin_roots(),
    }
}

/// A real, on-disk, genuinely-eligible `Explicit`/`ExplicitRoot` Dolphin
/// profile - `Explicit`, not `Native`, for exactly the reason
/// `dolphin_execution`'s own core test suite documents: only
/// `explicit_configuration_roots`/`explicit_executables` let a test
/// deterministically control installation type without ever touching a
/// real `PATH` scan (which would leak the real host environment into a
/// test).
struct ReadyDolphinFixture {
    fixture: DolphinFixture,
    context: DolphinLaunchContext,
    profile_id: String,
    profile_root: PathBuf,
    executable: PathBuf,
    content_path: PathBuf,
}

fn build_ready_dolphin_fixture(label: &str) -> ReadyDolphinFixture {
    let fixture = DolphinFixture::new(label);
    let profile_root = fixture.path("dolphin-portable");
    std::fs::create_dir_all(profile_root.join("Config")).unwrap();
    std::fs::write(profile_root.join("Dolphin.ini"), b"[Core]\n").unwrap();
    std::fs::write(profile_root.join("Config/Dolphin.ini"), b"[Core]\n").unwrap();
    let executable = fixture.path("bin/dolphin-emu");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let content_path = fixture.path("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, gamecube_iso_bytes(b"GALE01")).unwrap();

    let mut roots = base_dolphin_roots(&fixture);
    roots
        .explicit_configuration_roots
        .push(profile_root.clone());
    roots.explicit_executables.push(executable.clone());
    let discovery = discover_dolphin_local_profiles(&roots);
    let profile_id = format!("dolphin:{}", profile_root.display());
    assert!(
        discovery
            .profiles
            .iter()
            .any(|profile| profile.profile_id == profile_id && profile.eligible),
        "fixture profile must be discovered and eligible"
    );
    ReadyDolphinFixture {
        fixture,
        context: DolphinLaunchContext { discovery, roots },
        profile_id,
        profile_root,
        executable,
        content_path,
    }
}

/// The same shape as [`build_ready_dolphin_fixture`], but a `Portable`
/// (AppImage-shaped) install: genuinely eligible at the plan/readiness
/// level, but never a native launch binding - see
/// `resolve_dolphin_native_launch_binding`'s own `UnsupportedInstallationType`
/// refusal.
fn build_ready_dolphin_portable_fixture(label: &str) -> ReadyDolphinFixture {
    let fixture = DolphinFixture::new(label);
    let profile_root = fixture.path("dolphin-appimage/User");
    std::fs::create_dir_all(&profile_root).unwrap();
    std::fs::write(profile_root.join("Dolphin.ini"), b"[Core]\n").unwrap();
    let content_path = fixture.path("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, gamecube_iso_bytes(b"GALE01")).unwrap();

    let mut roots = base_dolphin_roots(&fixture);
    roots
        .portable_configuration_roots
        .push(profile_root.clone());
    let discovery = discover_dolphin_local_profiles(&roots);
    let profile_id = format!("dolphin:{}", profile_root.display());
    assert!(
        discovery
            .profiles
            .iter()
            .any(|profile| profile.profile_id == profile_id && profile.eligible),
        "fixture portable profile must be discovered and eligible"
    );
    ReadyDolphinFixture {
        fixture,
        context: DolphinLaunchContext { discovery, roots },
        profile_id,
        profile_root,
        executable: PathBuf::new(),
        content_path,
    }
}

fn dolphin_candidate(profile_id: &str, content_path: &Path) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "dolphin",
            profile_id: profile_id.to_string(),
            profile_path: None,
        },
        content: resolved_content(content_path.to_str().unwrap()),
        firmware: FirmwareReadiness::NotRequired,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn dolphin_plan_with(candidates: Vec<LaunchCandidate>, game_id: &str) -> LaunchPlan {
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
        platform_id: Some(DOLPHIN_SUPPORTED_PLATFORM_ID.to_string()),
        game_key: Some(game_id.to_string()),
        summary: LaunchPlanSummary {
            candidates: candidates.len(),
            ready,
            ready_with_warnings,
            blocked,
        },
        candidates,
    }
}

fn spawn_test_dolphin_process(
    executable: &str,
    arguments: &[&str],
    content_path: &Path,
    profile_id: &str,
) -> LaunchedDolphinProcess {
    let command = DolphinCommand {
        executable: PathBuf::from(executable),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).into())
            .collect(),
        working_directory: None,
        selection: DolphinCommandSelection {
            profile_id: profile_id.to_string(),
            user_directory_mode: DolphinUserDirectoryMode::DefaultNative,
            platform_id: DOLPHIN_SUPPORTED_PLATFORM_ID.to_string(),
            game_id: "GALE01".to_string(),
            content_path: content_path.to_path_buf(),
        },
    };
    spawn_dolphin(command).expect("spawning the fixture test process must succeed")
}

fn wait_until_dolphin_exited(process: &mut LaunchedDolphinProcess) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline, "fixture process never exited");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_until_dolphin_not_starting(state: &mut DolphinLaunchState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        state.poll();
        if !matches!(
            state.tracked,
            Some((_, DolphinLaunchStage::Starting { .. }))
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

// --- Launch Dolphin button eligibility --------------------------------------

#[test]
fn eligible_native_gamecube_candidate_shows_launch_dolphin_button() {
    let ready = build_ready_dolphin_fixture("eligible");
    let candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let plan = dolphin_plan_with(vec![candidate], "GALE01");
    let output = render(&dolphin_plan_input(plan, ready.context));
    assert!(rendered_text_contains(&output, "Launch Dolphin"));
}

#[test]
fn dolphin_ready_with_warnings_candidate_has_no_launch_button() {
    let mut candidate = dolphin_candidate("dolphin:/whatever", Path::new("/library/game.iso"));
    candidate.readiness = LaunchReadiness::ReadyWithWarnings;
    candidate.warnings.push(LaunchWarning::new(
        LaunchWarningKind::MultipleEligibleProfiles,
        "more than one eligible profile exists",
    ));
    let plan = dolphin_plan_with(vec![candidate], "GALE01");
    let output = render(&dolphin_plan_input(plan, empty_dolphin_context()));
    assert!(!rendered_text_contains(&output, "Launch Dolphin"));
}

#[test]
fn dolphin_blocked_candidate_has_no_launch_button() {
    let mut candidate = dolphin_candidate("dolphin:/whatever", Path::new("/library/game.iso"));
    candidate.readiness = LaunchReadiness::Blocked;
    candidate.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::ProfileIneligible,
        "the discovered profile is not eligible",
    ));
    let plan = dolphin_plan_with(vec![candidate], "GALE01");
    let output = render(&dolphin_plan_input(plan, empty_dolphin_context()));
    assert!(!rendered_text_contains(&output, "Launch Dolphin"));
}

#[test]
fn portable_appimage_profile_has_no_launch_dolphin_button() {
    let ready = build_ready_dolphin_portable_fixture("portable-appimage");
    let candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let plan = dolphin_plan_with(vec![candidate], "GALE01");
    // Genuinely `Ready` at the plan level (the profile is eligible) - only
    // `resolve_dolphin_native_launch_binding`'s own refusal to treat a
    // portable/AppImage-shaped install as native-equivalent keeps the
    // button from showing.
    let output = render(&dolphin_plan_input(plan, ready.context));
    assert!(!rendered_text_contains(&output, "Launch Dolphin"));
}

#[test]
fn wrong_standalone_emulator_has_no_launch_dolphin_button() {
    let mut candidate = dolphin_candidate("dolphin:/whatever", Path::new("/library/game.iso"));
    candidate.target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "native".to_string(),
        profile_path: None,
    };
    let plan = dolphin_plan_with(vec![candidate], "GALE01");
    let output = render(&dolphin_plan_input(plan, empty_dolphin_context()));
    assert!(!rendered_text_contains(&output, "Launch Dolphin"));
}

#[test]
fn eligible_native_wii_candidate_shows_launch_dolphin_button() {
    let ready = build_ready_dolphin_fixture("wii");
    let candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let mut plan = dolphin_plan_with(vec![candidate], "RALE01");
    plan.platform_id = Some("Wii".to_string());
    let output = render(&dolphin_plan_input(plan, ready.context));
    assert!(rendered_text_contains(&output, "Launch Dolphin"));
}

#[test]
fn direct_dolphin_containers_are_visible_only_with_matching_verified_platform() {
    for extension in ["rvz", "ciso"] {
        let ready = build_ready_dolphin_fixture(extension);
        let candidate = dolphin_candidate(
            &ready.profile_id,
            Path::new(&format!("/library/game.{extension}")),
        );
        let plan = dolphin_plan_with(vec![candidate], "GALE01");
        let output = render(&dolphin_plan_input(plan, ready.context));
        assert!(
            rendered_text_contains(&output, "Launch Dolphin"),
            "{extension}"
        );
    }

    let ready = build_ready_dolphin_fixture("wbfs-gamecube");
    let wbfs = dolphin_candidate(&ready.profile_id, Path::new("/library/game.wbfs"));
    let mut gamecube_plan = dolphin_plan_with(vec![wbfs], "GALE01");
    gamecube_plan.platform_id = Some("GameCube".to_string());
    let output = render(&dolphin_plan_input(gamecube_plan, ready.context));
    assert!(!rendered_text_contains(&output, "Launch Dolphin"));

    let ready = build_ready_dolphin_fixture("wbfs-wii");
    let wbfs = dolphin_candidate(&ready.profile_id, Path::new("/library/game.wbfs"));
    let mut wii_plan = dolphin_plan_with(vec![wbfs], "RALE01");
    wii_plan.platform_id = Some("Wii".to_string());
    let output = render(&dolphin_plan_input(wii_plan, ready.context));
    assert!(rendered_text_contains(&output, "Launch Dolphin"));
}

#[test]
fn archive_or_mounted_content_has_no_launch_dolphin_button() {
    let mut candidate = dolphin_candidate("dolphin:/whatever", Path::new("/library/game.iso"));
    candidate.content = unresolved_archive_content();
    let plan = dolphin_plan_with(vec![candidate], "GALE01");
    let output = render(&dolphin_plan_input(plan, empty_dolphin_context()));
    assert!(!rendered_text_contains(&output, "Launch Dolphin"));
}

// --- request facts sent to core ----------------------------------------------

#[test]
fn clicking_emits_exact_dolphin_launch_request_facts() {
    let ready = build_ready_dolphin_fixture("request-facts");
    let candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let plan = dolphin_plan_with(vec![candidate.clone()], "GALE01");
    let request = dolphin_launch_request(&plan, &candidate, &ready.context)
        .expect("this candidate is Dolphin-eligible");

    assert_eq!(request.selected_content_path, ready.content_path);
    assert_eq!(request.expected_game_id, "GALE01");
    assert_eq!(request.profile_id, ready.profile_id);
    assert_eq!(request.expected_executable, ready.executable);
    assert_eq!(
        request.expected_user_directory_mode,
        DolphinUserDirectoryMode::ExplicitRoot(ready.profile_root.clone())
    );
}

#[test]
fn gui_never_emits_argv_or_command_strings() {
    // Exhaustive destructure: if a future change ever added an
    // `arguments`/`-u`/`-e` field to `DolphinLaunchRequest`, this would
    // fail to compile, forcing a review rather than silently letting the
    // GUI start building argv.
    let request = DolphinLaunchRequest {
        selected_content_path: PathBuf::from("/library/game.iso"),
        expected_game_id: "GALE01".to_string(),
        profile_id: "dolphin:/profile".to_string(),
        expected_executable: PathBuf::from("/usr/bin/dolphin-emu"),
        expected_user_directory_mode: DolphinUserDirectoryMode::DefaultNative,
    };
    let DolphinLaunchRequest {
        selected_content_path: _,
        expected_game_id: _,
        profile_id: _,
        expected_executable: _,
        expected_user_directory_mode: _,
    } = request;
}

#[test]
fn default_native_binding_produces_request_without_gui_inventing_u() {
    let binding = DolphinNativeLaunchBinding {
        executable: PathBuf::from("/usr/bin/dolphin-emu"),
        user_directory_mode: DolphinUserDirectoryMode::DefaultNative,
    };
    let request = dolphin_launch_request_from_binding(
        "dolphin:/config/dolphin-emu".to_string(),
        "GALE01".to_string(),
        PathBuf::from("/library/game.iso"),
        binding,
    );
    assert_eq!(
        request.expected_executable,
        PathBuf::from("/usr/bin/dolphin-emu")
    );
    assert_eq!(
        request.expected_user_directory_mode,
        DolphinUserDirectoryMode::DefaultNative
    );
}

#[test]
fn explicit_root_binding_preserves_the_verified_mode_and_path_as_facts_only() {
    let root = PathBuf::from("/profiles/dolphin-portable");
    let binding = DolphinNativeLaunchBinding {
        executable: PathBuf::from("/opt/dolphin/dolphin-emu"),
        user_directory_mode: DolphinUserDirectoryMode::ExplicitRoot(root.clone()),
    };
    let request = dolphin_launch_request_from_binding(
        "dolphin:/profiles/dolphin-portable".to_string(),
        "GALE01".to_string(),
        PathBuf::from("/library/game.iso"),
        binding,
    );
    assert_eq!(
        request.expected_user_directory_mode,
        DolphinUserDirectoryMode::ExplicitRoot(root)
    );
    assert_eq!(
        request.expected_executable,
        PathBuf::from("/opt/dolphin/dolphin-emu")
    );
}

// --- Starting disables a duplicate click -------------------------------------

#[test]
fn dolphin_starting_stage_shows_a_starting_label_and_no_active_launch_button() {
    let ready = build_ready_dolphin_fixture("starting");
    let candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let plan = dolphin_plan_with(vec![candidate.clone()], "GALE01");
    let request = dolphin_launch_request(&plan, &candidate, &ready.context).unwrap();
    let key = DolphinLaunchKey::from_request(&request);
    let (_sender, receiver) = mpsc::channel();
    let mut state = DolphinLaunchState {
        tracked: Some((key, DolphinLaunchStage::Starting { receiver })),
    };

    let output = render_with_dolphin_state(&dolphin_plan_input(plan, ready.context), &mut state);
    assert!(rendered_text_contains(&output, "Starting Dolphin…"));
    assert!(!rendered_text_contains(&output, "Launch Dolphin"));
}

// --- Running shows PID --------------------------------------------------------

#[test]
fn dolphin_running_stage_shows_running_status_and_pid() {
    let ready = build_ready_dolphin_fixture("running");
    let candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let plan = dolphin_plan_with(vec![candidate.clone()], "GALE01");
    let request = dolphin_launch_request(&plan, &candidate, &ready.context).unwrap();
    let key = DolphinLaunchKey::from_request(&request);
    let process =
        spawn_test_dolphin_process("/bin/sleep", &["1"], &ready.content_path, &ready.profile_id);
    let pid = process.pid;
    let mut state = DolphinLaunchState {
        tracked: Some((key, DolphinLaunchStage::Running { process })),
    };

    let output = render_with_dolphin_state(&dolphin_plan_input(plan, ready.context), &mut state);
    assert!(rendered_text_contains(&output, "Dolphin running"));
    assert!(rendered_text_contains(&output, &pid.to_string()));
    assert!(!rendered_text_contains(&output, "Stop"));
    assert!(!rendered_text_contains(&output, "Kill"));
}

// --- process exit -------------------------------------------------------------

#[test]
fn dolphin_clean_process_exit_enters_exited_and_shows_a_clean_exit_message() {
    let ready = build_ready_dolphin_fixture("clean-exit");
    let candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let plan = dolphin_plan_with(vec![candidate.clone()], "GALE01");
    let request = dolphin_launch_request(&plan, &candidate, &ready.context).unwrap();
    let key = DolphinLaunchKey::from_request(&request);
    let mut process =
        spawn_test_dolphin_process("/bin/true", &[], &ready.content_path, &ready.profile_id);
    wait_until_dolphin_exited(&mut process);
    let mut state = DolphinLaunchState {
        tracked: Some((key, DolphinLaunchStage::Exited { process })),
    };

    let output = render_with_dolphin_state(&dolphin_plan_input(plan, ready.context), &mut state);
    assert!(rendered_text_contains(&output, "Dolphin exited"));
    assert!(rendered_text_contains(&output, "closed normally"));
    assert!(!rendered_text_contains(&output, "Launch failed"));
    // A relaunch is a fresh explicit click, never automatic - the button
    // must still be present (and enabled) after a clean exit.
    assert!(rendered_text_contains(&output, "Launch Dolphin"));
}

#[test]
fn dolphin_non_zero_exit_shows_bounded_stderr_behind_technical_details() {
    let ready = build_ready_dolphin_fixture("non-zero-exit");
    let candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let plan = dolphin_plan_with(vec![candidate.clone()], "GALE01");
    let request = dolphin_launch_request(&plan, &candidate, &ready.context).unwrap();
    let key = DolphinLaunchKey::from_request(&request);
    let mut process = spawn_test_dolphin_process(
        "/bin/sh",
        &["-c", "echo dolphin-failure-marker-9f31 >&2; exit 3"],
        &ready.content_path,
        &ready.profile_id,
    );
    wait_until_dolphin_exited(&mut process);
    let mut state = DolphinLaunchState {
        tracked: Some((key, DolphinLaunchStage::Exited { process })),
    };

    let output = render_with_dolphin_state(&dolphin_plan_input(plan, ready.context), &mut state);
    assert!(rendered_text_contains(
        &output,
        "Dolphin exited unexpectedly"
    ));
    assert!(rendered_text_contains(&output, "Technical details"));
    assert!(!rendered_text_contains(
        &output,
        "dolphin-failure-marker-9f31"
    ));
}

// --- preflight/binding failure -----------------------------------------------

#[test]
fn dolphin_preflight_failure_enters_failed_with_a_gamer_facing_message() {
    let ready = build_ready_dolphin_fixture("preflight-failure");
    // The candidate (not just the request afterwards) must carry this same
    // nonexistent path, so the key the render call re-derives from `plan`
    // matches the key `state.start` tracked it under - the same reasoning
    // `preflight_failure_enters_failed_with_a_gamer_facing_message_never_no_emulator_installed`
    // documents for RetroArch.
    let missing_content = PathBuf::from("/nonexistent/dolphin-launch-readiness-fixture-9f31.iso");
    let candidate = dolphin_candidate(&ready.profile_id, &missing_content);
    let plan = dolphin_plan_with(vec![candidate], "GALE01");
    // Content is checked before profile/binding lookups in
    // `preflight_dolphin_launch`, so this alone is enough to force
    // `Failed` through the real background worker end-to-end.
    let request = DolphinLaunchRequest {
        selected_content_path: missing_content,
        expected_game_id: "GALE01".to_string(),
        profile_id: ready.profile_id.clone(),
        expected_executable: ready.executable.clone(),
        expected_user_directory_mode: DolphinUserDirectoryMode::ExplicitRoot(
            ready.profile_root.clone(),
        ),
    };
    let key = DolphinLaunchKey::from_request(&request);

    let mut state = DolphinLaunchState::default();
    state.start(request);
    wait_until_dolphin_not_starting(&mut state);
    assert!(matches!(
        state.tracked,
        Some((ref tracked_key, DolphinLaunchStage::Failed { .. })) if *tracked_key == key
    ));

    let output = render_with_dolphin_state(&dolphin_plan_input(plan, ready.context), &mut state);
    assert!(rendered_text_contains(&output, "Launch failed"));
    assert!(rendered_text_contains(
        &output,
        "This game's file is no longer available where it was last seen."
    ));
    assert!(rendered_text_contains(&output, "Technical details"));
}

// --- stale selection handling -------------------------------------------------

#[test]
fn dolphin_switching_selection_never_shows_stale_state_and_still_reaps_the_old_watcher() {
    let ready = build_ready_dolphin_fixture("stale-selection");
    let other_candidate = dolphin_candidate(&ready.profile_id, &ready.content_path);
    let other_plan = dolphin_plan_with(vec![other_candidate.clone()], "GALE01");
    let other_request =
        dolphin_launch_request(&other_plan, &other_candidate, &ready.context).unwrap();
    let other_key = DolphinLaunchKey::from_request(&other_request);
    let process =
        spawn_test_dolphin_process("/bin/sleep", &["1"], &ready.content_path, &ready.profile_id);
    let mut state = DolphinLaunchState {
        tracked: Some((other_key, DolphinLaunchStage::Running { process })),
    };

    // A different game's content, from the same profile discovery, is
    // rendered while the other selection's tracker is still Running.
    let different_content = ready.fixture.path("games/other-game.iso");
    std::fs::write(&different_content, gamecube_iso_bytes(b"GALE01")).unwrap();
    let plan = dolphin_plan_with(
        vec![dolphin_candidate(&ready.profile_id, &different_content)],
        "GALE01",
    );
    let output = render_with_dolphin_state(&dolphin_plan_input(plan, ready.context), &mut state);

    assert!(
        !rendered_text_contains(&output, "Dolphin running"),
        "a different selection must never display another selection's running process"
    );
    assert!(
        rendered_text_contains(&output, "Launch Dolphin"),
        "the newly displayed, untracked candidate must show its own idle Launch button"
    );
    // The other selection's process is still tracked (never dropped/killed
    // just because the selection changed) - `poll()` keeps reaping it in
    // the background regardless of what is currently rendered.
    assert!(state.tracked.is_some());
    state.poll();
    assert!(state.tracked.is_some());
}

// ---------------------------------------------------------------------------
// Launch PCSX2 fixtures
// ---------------------------------------------------------------------------

struct Pcsx2Fixture {
    root: PathBuf,
}

impl Pcsx2Fixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_PCSX2_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "archivefs-gui-pcsx2-launch-{label}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for Pcsx2Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn base_pcsx2_roots(fixture: &Pcsx2Fixture) -> Pcsx2ProfileDiscoveryRoots {
    Pcsx2ProfileDiscoveryRoots {
        home: fixture.path("home"),
        xdg_config_home: fixture.path("config"),
        xdg_data_home: fixture.path("data"),
        documents_home: fixture.path("home/Documents"),
        flatpak_system_root: fixture.path("system-flatpak"),
        appimage_directory: None,
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
    }
}

/// The smallest ISO9660 structure the core PS2 identity inspector needs to
/// derive `SLUS-12345` from `SYSTEM.CNF` - synthetic test bytes only.
fn ps2_iso_bytes() -> Vec<u8> {
    const SECTOR: usize = 2_048;
    fn directory_record(name: &[u8], extent: u32, size: u32, directory: bool) -> Vec<u8> {
        let length = 33 + name.len() + usize::from(name.len().is_multiple_of(2));
        let mut record = vec![0_u8; length];
        record[0] = length as u8;
        record[2..6].copy_from_slice(&extent.to_le_bytes());
        record[6..10].copy_from_slice(&extent.to_be_bytes());
        record[10..14].copy_from_slice(&size.to_le_bytes());
        record[14..18].copy_from_slice(&size.to_be_bytes());
        record[25] = if directory { 2 } else { 0 };
        record[28..30].copy_from_slice(&1_u16.to_le_bytes());
        record[30..32].copy_from_slice(&1_u16.to_be_bytes());
        record[32] = name.len() as u8;
        record[33..33 + name.len()].copy_from_slice(name);
        record
    }

    let mut iso = vec![0_u8; 24 * SECTOR];
    let pvd = 16 * SECTOR;
    iso[pvd] = 1;
    iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
    iso[pvd + 6] = 1;
    let root = directory_record(&[0], 20, SECTOR as u32, true);
    iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
    let terminator = 17 * SECTOR;
    iso[terminator] = 255;
    iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
    iso[terminator + 6] = 1;

    let cnf = b"VER = 1.00\r\nBOOT2 = cdrom0:\\SLUS_123.45;1\r\n";
    let root_offset = 20 * SECTOR;
    let cnf_record = directory_record(b"SYSTEM.CNF;1", 21, cnf.len() as u32, false);
    iso[root_offset..root_offset + cnf_record.len()].copy_from_slice(&cnf_record);
    let cnf_offset = 21 * SECTOR;
    iso[cnf_offset..cnf_offset + cnf.len()].copy_from_slice(cnf);
    iso
}

const PS2_SERIAL: &str = "SLUS-12345";
/// The standard `abc` hash vector. It is synthetic fixture data, never a
/// real BIOS dump or a Redump-published BIOS hash.
const SYNTHETIC_BIOS_BYTES: &[u8] = b"abc";

fn matching_ps2_firmware_record() -> FirmwareIdentityRecord {
    FirmwareIdentityRecord {
        system: FirmwareSystem::PlayStation2,
        provider: DatEcosystem::Redump,
        name: "synthetic PS2 BIOS fixture".to_string(),
        description: Some("synthetic test record; not a real Redump hash".to_string()),
        size_bytes: SYNTHETIC_BIOS_BYTES.len() as u64,
        crc32: Crc32::of(SYNTHETIC_BIOS_BYTES),
        md5: "900150983cd24fb0d6963f7d28e17f72".to_string(),
        sha1: "a9993e364706816aba3e25717850c26c9cd0d89d".to_string(),
        dat_version: Some("test-revision".to_string()),
    }
}

struct ReadyPcsx2Fixture {
    fixture: Pcsx2Fixture,
    context: Pcsx2LaunchContext,
    profile_id: String,
    executable: PathBuf,
    content_path: PathBuf,
}

fn build_ready_pcsx2_fixture(label: &str) -> ReadyPcsx2Fixture {
    let fixture = Pcsx2Fixture::new(label);
    let mut roots = base_pcsx2_roots(&fixture);
    let profile_root = roots.xdg_config_home.join("PCSX2");
    std::fs::create_dir_all(profile_root.join("bios")).unwrap();
    std::fs::write(profile_root.join("PCSX2.ini"), b"[Filenames]\n").unwrap();
    std::fs::write(
        profile_root.join("bios/scph-test.bin"),
        SYNTHETIC_BIOS_BYTES,
    )
    .unwrap();
    let executable = fixture.path("bin/pcsx2-qt");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    roots.explicit_executables.push(executable.clone());
    let content_path = fixture.path("games/game.iso");
    std::fs::create_dir_all(content_path.parent().unwrap()).unwrap();
    std::fs::write(&content_path, ps2_iso_bytes()).unwrap();

    let discovery = discover_pcsx2_profiles(&roots).unwrap();
    let profile_id = discovery
        .profiles
        .iter()
        .find(|profile| profile.configuration_path == profile_root)
        .expect("fixture profile must be discovered")
        .profile_id
        .clone();
    let context = Pcsx2LaunchContext {
        discovery,
        roots,
        firmware_evidence: vec![matching_ps2_firmware_record()],
        verified_ps2_serial: Some(PS2_SERIAL.to_string()),
    };
    ReadyPcsx2Fixture {
        fixture,
        context,
        profile_id,
        executable,
        content_path,
    }
}

fn empty_pcsx2_context() -> Pcsx2LaunchContext {
    Pcsx2LaunchContext {
        discovery: Pcsx2ProfileDiscovery {
            profiles: Vec::new(),
            warnings: Vec::new(),
            complete: true,
        },
        roots: Pcsx2ProfileDiscoveryRoots {
            home: PathBuf::from("/nonexistent-pcsx2-launch-home"),
            xdg_config_home: PathBuf::from("/nonexistent-pcsx2-launch-config"),
            xdg_data_home: PathBuf::from("/nonexistent-pcsx2-launch-data"),
            documents_home: PathBuf::from("/nonexistent-pcsx2-launch-home/Documents"),
            flatpak_system_root: PathBuf::from("/nonexistent-pcsx2-launch-flatpak"),
            appimage_directory: None,
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
        },
        firmware_evidence: Vec::new(),
        verified_ps2_serial: Some(PS2_SERIAL.to_string()),
    }
}

fn pcsx2_candidate(profile_id: &str, content_path: &Path) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "pcsx2",
            profile_id: profile_id.to_string(),
            profile_path: None,
        },
        content: resolved_content(content_path.to_str().unwrap()),
        firmware: FirmwareReadiness::Verified,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn pcsx2_plan_with(candidates: Vec<LaunchCandidate>) -> LaunchPlan {
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
        platform_id: Some(PCSX2_SUPPORTED_PLATFORM_ID.to_string()),
        game_key: Some(PS2_SERIAL.to_string()),
        summary: LaunchPlanSummary {
            candidates: candidates.len(),
            ready,
            ready_with_warnings,
            blocked,
        },
        candidates,
    }
}

fn spawn_test_pcsx2_process(
    executable: &str,
    arguments: &[&str],
    content_path: &Path,
    profile_id: &str,
) -> LaunchedPcsx2Process {
    spawn_pcsx2(Pcsx2Command {
        executable: PathBuf::from(executable),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).into())
            .collect(),
        working_directory: None,
        selection: Pcsx2CommandSelection {
            profile_id: profile_id.to_string(),
            user_directory_mode: Pcsx2UserDirectoryMode::DefaultNative,
            platform_id: PCSX2_SUPPORTED_PLATFORM_ID.to_string(),
            verified_ps2_serial: PS2_SERIAL.to_string(),
            content_path: content_path.to_path_buf(),
        },
    })
    .expect("spawning the fixture test process must succeed")
}

fn wait_until_pcsx2_exited(process: &mut LaunchedPcsx2Process) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline, "fixture process never exited");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// --- Launch PCSX2 button eligibility and request facts ---------------------

#[test]
fn strict_ready_ps2_candidate_exposes_launch_pcsx2() {
    let ready = build_ready_pcsx2_fixture("eligible");
    let plan = pcsx2_plan_with(vec![pcsx2_candidate(
        &ready.profile_id,
        &ready.content_path,
    )]);
    let output = render(&pcsx2_plan_input(plan, ready.context));
    assert!(rendered_text_contains(&output, "Launch PCSX2"));
}

#[test]
fn pcsx2_ready_with_warnings_or_blocked_candidates_do_not_offer_launch() {
    for readiness in [LaunchReadiness::ReadyWithWarnings, LaunchReadiness::Blocked] {
        let mut candidate = pcsx2_candidate("pcsx2:unknown", Path::new("/library/game.iso"));
        candidate.readiness = readiness;
        if readiness == LaunchReadiness::ReadyWithWarnings {
            candidate.warnings.push(LaunchWarning::new(
                LaunchWarningKind::MultipleEligibleProfiles,
                "profile choice is not strict",
            ));
        } else {
            candidate.blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::RequiredFirmwareMissing,
                "firmware is missing",
            ));
        }
        let output = render(&pcsx2_plan_input(
            pcsx2_plan_with(vec![candidate]),
            empty_pcsx2_context(),
        ));
        assert!(!rendered_text_contains(&output, "Launch PCSX2"));
    }
}

#[test]
fn pcsx2_wrong_platform_wrong_adapter_mount_or_non_iso_never_yield_request() {
    let ready = build_ready_pcsx2_fixture("ineligible-content");
    let candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    assert!(pcsx2_launch_request(&plan, &candidate, &ready.context).is_some());

    let mut wrong_platform = plan.clone();
    wrong_platform.platform_id = Some("PSX".to_string());
    assert!(pcsx2_launch_request(&wrong_platform, &candidate, &ready.context).is_none());

    let mut wrong_adapter = candidate.clone();
    wrong_adapter.target = LaunchTarget::Standalone {
        adapter_id: "dolphin",
        profile_id: ready.profile_id.clone(),
        profile_path: None,
    };
    assert!(pcsx2_launch_request(&plan, &wrong_adapter, &ready.context).is_none());

    let mut mounted = candidate.clone();
    mounted.content = unresolved_archive_content();
    assert!(pcsx2_launch_request(&plan, &mounted, &ready.context).is_none());

    let mut non_iso = candidate;
    non_iso.content.resolved_path = Some(ready.fixture.path("games/game.chd"));
    assert!(pcsx2_launch_request(&plan, &non_iso, &ready.context).is_none());
}

#[test]
fn missing_firmware_or_verified_serial_prevents_pcsx2_request() {
    let ready = build_ready_pcsx2_fixture("missing-firmware");
    let mut candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    candidate.firmware = FirmwareReadiness::Missing;
    candidate.readiness = LaunchReadiness::Blocked;
    candidate.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::RequiredFirmwareMissing,
        "no verified PS2 BIOS",
    ));
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    assert!(pcsx2_launch_request(&plan, &candidate, &ready.context).is_none());

    // The real planner also makes this inconsistent state non-Ready, but
    // button eligibility independently requires verified firmware so a
    // malformed/stale UI candidate can never expose a launch action first.
    let mut unverified_but_ready = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    unverified_but_ready.firmware = FirmwareReadiness::PresentUnverified;
    let plan = pcsx2_plan_with(vec![unverified_but_ready.clone()]);
    assert!(pcsx2_launch_request(&plan, &unverified_but_ready, &ready.context).is_none());

    let mut context = ready.context;
    context.verified_ps2_serial = None;
    let candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    assert!(pcsx2_launch_request(&plan, &candidate, &context).is_none());
}

#[test]
fn matching_synthetic_redump_evidence_is_preserved_for_core_preflight() {
    let ready = build_ready_pcsx2_fixture("firmware-evidence");
    let candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    let request = pcsx2_launch_request(&plan, &candidate, &ready.context)
        .expect("verified firmware and serial must permit a facts-only request");
    let command = archivefs_core::launch::preflight_pcsx2_launch(
        &request,
        &ready.context.roots,
        &ready.context.firmware_evidence,
    )
    .expect("the exact evidence handed from the context must verify the synthetic BIOS");
    assert_eq!(command.selection.verified_ps2_serial, PS2_SERIAL);
}

#[test]
fn clicking_pcsx2_uses_a_facts_only_request_never_argv_or_shell() {
    let ready = build_ready_pcsx2_fixture("request-facts");
    let candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let request = pcsx2_launch_request(
        &pcsx2_plan_with(vec![candidate.clone()]),
        &candidate,
        &ready.context,
    )
    .unwrap();
    assert_eq!(request.selected_content_path, ready.content_path);
    assert_eq!(request.expected_platform_id, PCSX2_SUPPORTED_PLATFORM_ID);
    assert_eq!(request.expected_game_key, PS2_SERIAL);
    assert_eq!(request.expected_ps2_serial, PS2_SERIAL);
    assert_eq!(request.profile_id, ready.profile_id);
    assert_eq!(request.expected_executable, ready.executable);
    assert_eq!(
        request.expected_user_directory_mode,
        Pcsx2UserDirectoryMode::DefaultNative
    );
    let Pcsx2LaunchRequest {
        selected_content_path: _,
        expected_platform_id: _,
        expected_game_key: _,
        expected_ps2_serial: _,
        profile_id: _,
        expected_executable: _,
        expected_user_directory_mode: _,
    } = request;
}

// --- PCSX2 lifecycle and stale-selection handling --------------------------

#[test]
fn pcsx2_starting_and_running_states_show_status_and_pid_without_stop_controls() {
    let ready = build_ready_pcsx2_fixture("starting-running");
    let candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    let request = pcsx2_launch_request(&plan, &candidate, &ready.context).unwrap();
    let key = Pcsx2LaunchKey::from_request(&request);
    let (_sender, receiver) = mpsc::channel();
    let mut starting = Pcsx2LaunchState {
        tracked: Some((key.clone(), Pcsx2LaunchStage::Starting { receiver })),
    };
    let output = render_with_pcsx2_state(
        &pcsx2_plan_input(plan.clone(), ready.context),
        &mut starting,
    );
    assert!(rendered_text_contains(&output, "Starting PCSX2…"));
    assert!(!rendered_text_contains(&output, "Launch PCSX2"));

    let ready = build_ready_pcsx2_fixture("running");
    let candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    let request = pcsx2_launch_request(&plan, &candidate, &ready.context).unwrap();
    let key = Pcsx2LaunchKey::from_request(&request);
    let process =
        spawn_test_pcsx2_process("/bin/sleep", &["1"], &ready.content_path, &ready.profile_id);
    let pid = process.pid;
    let mut running = Pcsx2LaunchState {
        tracked: Some((key, Pcsx2LaunchStage::Running { process })),
    };
    let output = render_with_pcsx2_state(&pcsx2_plan_input(plan, ready.context), &mut running);
    assert!(rendered_text_contains(&output, "PCSX2 running"));
    assert!(rendered_text_contains(&output, &pid.to_string()));
    assert!(!rendered_text_contains(&output, "Stop"));
    assert!(!rendered_text_contains(&output, "Kill"));
}

#[test]
fn pcsx2_exit_states_show_clean_or_nonzero_status_without_exposing_stderr_inline() {
    let ready = build_ready_pcsx2_fixture("clean-exit");
    let candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    let request = pcsx2_launch_request(&plan, &candidate, &ready.context).unwrap();
    let key = Pcsx2LaunchKey::from_request(&request);
    let mut process =
        spawn_test_pcsx2_process("/bin/true", &[], &ready.content_path, &ready.profile_id);
    wait_until_pcsx2_exited(&mut process);
    let mut state = Pcsx2LaunchState {
        tracked: Some((key, Pcsx2LaunchStage::Exited { process })),
    };
    let output = render_with_pcsx2_state(&pcsx2_plan_input(plan, ready.context), &mut state);
    assert!(rendered_text_contains(&output, "PCSX2 exited"));
    assert!(rendered_text_contains(&output, "closed normally"));

    let ready = build_ready_pcsx2_fixture("nonzero-exit");
    let candidate = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    let request = pcsx2_launch_request(&plan, &candidate, &ready.context).unwrap();
    let key = Pcsx2LaunchKey::from_request(&request);
    let mut process = spawn_test_pcsx2_process(
        "/bin/sh",
        &["-c", "echo pcsx2-failure-marker-9f31 >&2; exit 3"],
        &ready.content_path,
        &ready.profile_id,
    );
    wait_until_pcsx2_exited(&mut process);
    let mut state = Pcsx2LaunchState {
        tracked: Some((key, Pcsx2LaunchStage::Exited { process })),
    };
    let output = render_with_pcsx2_state(&pcsx2_plan_input(plan, ready.context), &mut state);
    assert!(rendered_text_contains(&output, "PCSX2 exited unexpectedly"));
    assert!(rendered_text_contains(&output, "Technical details"));
    assert!(!rendered_text_contains(
        &output,
        "pcsx2-failure-marker-9f31"
    ));
}

#[test]
fn pcsx2_preflight_failure_and_stale_selection_are_isolated_honestly() {
    let ready = build_ready_pcsx2_fixture("stale-selection");
    let missing = PathBuf::from("/nonexistent/pcsx2-launch-readiness-fixture.iso");
    let candidate = pcsx2_candidate(&ready.profile_id, &missing);
    let plan = pcsx2_plan_with(vec![candidate.clone()]);
    let request = Pcsx2LaunchRequest {
        selected_content_path: missing,
        expected_platform_id: PCSX2_SUPPORTED_PLATFORM_ID.to_string(),
        expected_game_key: PS2_SERIAL.to_string(),
        expected_ps2_serial: PS2_SERIAL.to_string(),
        profile_id: ready.profile_id.clone(),
        expected_executable: ready.executable.clone(),
        expected_user_directory_mode: Pcsx2UserDirectoryMode::DefaultNative,
    };
    let key = Pcsx2LaunchKey::from_request(&request);
    let error =
        Pcsx2LaunchExecutionError::Preflight(archivefs_core::launch::Pcsx2LaunchPreflightError {
            kind: Pcsx2LaunchPreflightErrorKind::ContentNotFound,
            detail: "fixture content was removed".to_string(),
        });
    let mut failed = Pcsx2LaunchState {
        tracked: Some((key, Pcsx2LaunchStage::Failed { error })),
    };
    let output = render_with_pcsx2_state(&pcsx2_plan_input(plan, ready.context), &mut failed);
    assert!(rendered_text_contains(&output, "Launch failed"));
    assert!(rendered_text_contains(
        &output,
        "Game file changed since readiness was checked."
    ));

    let ready = build_ready_pcsx2_fixture("stale-running");
    let first = pcsx2_candidate(&ready.profile_id, &ready.content_path);
    let first_plan = pcsx2_plan_with(vec![first.clone()]);
    let first_request = pcsx2_launch_request(&first_plan, &first, &ready.context).unwrap();
    let process =
        spawn_test_pcsx2_process("/bin/sleep", &["1"], &ready.content_path, &ready.profile_id);
    let mut state = Pcsx2LaunchState {
        tracked: Some((
            Pcsx2LaunchKey::from_request(&first_request),
            Pcsx2LaunchStage::Running { process },
        )),
    };
    let other_content = ready.fixture.path("games/other.iso");
    std::fs::write(&other_content, ps2_iso_bytes()).unwrap();
    let other_plan = pcsx2_plan_with(vec![pcsx2_candidate(&ready.profile_id, &other_content)]);
    let output = render_with_pcsx2_state(&pcsx2_plan_input(other_plan, ready.context), &mut state);
    assert!(!rendered_text_contains(&output, "PCSX2 running"));
    assert!(rendered_text_contains(&output, "Launch PCSX2"));
    assert!(
        state.tracked.is_some(),
        "the old watcher must remain owned for reaping"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        state.poll();
        if matches!(state.tracked, Some((_, Pcsx2LaunchStage::Exited { .. }))) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the old selection's PCSX2 watcher was not reaped"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn pcsx2_binding_and_bios_readiness_drift_have_honest_gamer_facing_messages() {
    let binding =
        Pcsx2LaunchExecutionError::Preflight(archivefs_core::launch::Pcsx2LaunchPreflightError {
            kind: Pcsx2LaunchPreflightErrorKind::BindingDrift,
            detail: "native executable no longer matches the approved binding".to_string(),
        });
    let (message, detail) = pcsx2_launch_error_message(&binding);
    assert_eq!(message, "PCSX2 installation changed; refresh readiness.");
    assert!(detail.contains("BindingDrift"));

    // Core deliberately reports a fresh BIOS verification regression as the
    // general readiness failure below; the GUI must not claim a more specific
    // diagnosis than core supplied, while still making the required refresh
    // action clear.
    let bios =
        Pcsx2LaunchExecutionError::Preflight(archivefs_core::launch::Pcsx2LaunchPreflightError {
            kind: Pcsx2LaunchPreflightErrorKind::CandidateNotReady,
            detail: "fresh firmware verification did not remain Verified".to_string(),
        });
    let (message, detail) = pcsx2_launch_error_message(&bios);
    assert_eq!(
        message,
        "PCSX2 is no longer ready to launch - re-check readiness and try again."
    );
    assert!(detail.contains("firmware verification"));
}
