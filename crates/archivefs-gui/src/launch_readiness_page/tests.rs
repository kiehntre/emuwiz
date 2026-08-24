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

use archivefs_core::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use archivefs_core::launch::{
    DolphinCommand, DolphinCommandSelection, LaunchBlocker, LaunchBlockerKind, LaunchContainerKind,
    LaunchContentKind, LaunchContentRef, LaunchPlanSummary, LaunchWarning, LaunchWarningKind,
    RetroArchCommand, RetroArchCommandSelection, spawn_dolphin, spawn_retroarch,
};
use archivefs_core::patch_manager::{
    DolphinLocalProfileDiscovery, DolphinNativeLaunchBinding, DolphinUserDirectoryMode,
    discover_dolphin_local_profiles,
};

use super::*;

static NEXT_DOLPHIN_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

fn plan_input(plan: LaunchPlan) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan,
        dolphin: None,
    }
}

fn dolphin_plan_input(plan: LaunchPlan, context: DolphinLaunchContext) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan,
        dolphin: Some(context),
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
    )
}

fn render_with_state(
    input: &LaunchReadinessInput,
    state: &mut RetroArchLaunchState,
) -> egui::FullOutput {
    render_with_states(input, state, &mut DolphinLaunchState::default())
}

fn render_with_dolphin_state(
    input: &LaunchReadinessInput,
    state: &mut DolphinLaunchState,
) -> egui::FullOutput {
    render_with_states(input, &mut RetroArchLaunchState::default(), state)
}

fn render_with_states(
    input: &LaunchReadinessInput,
    retroarch_state: &mut RetroArchLaunchState,
    dolphin_state: &mut DolphinLaunchState,
) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_launch_readiness_panel(ui, input, retroarch_state, dolphin_state);
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
fn wii_candidate_has_no_launch_dolphin_button_while_core_is_gamecube_only() {
    let candidate = dolphin_candidate("dolphin:/whatever", Path::new("/library/game.iso"));
    let mut plan = dolphin_plan_with(vec![candidate], "RALE01");
    plan.platform_id = Some("Wii".to_string());
    let output = render(&dolphin_plan_input(plan, empty_dolphin_context()));
    assert!(!rendered_text_contains(&output, "Launch Dolphin"));
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
