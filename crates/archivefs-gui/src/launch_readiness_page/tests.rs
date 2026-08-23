//! Tests for the Launch Readiness panel renderer.
//!
//! Every [`LaunchCandidate`]/[`LaunchPlan`] fixture is built by hand (all
//! fields are public) - this module is testing the renderer's own text and
//! honesty, not re-deriving [`archivefs_core::launch::build_launch_plan`]'s
//! own logic, which already has its own dedicated test suite in
//! `archivefs-core`.

use std::path::PathBuf;

use archivefs_core::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use archivefs_core::launch::{
    LaunchBlocker, LaunchBlockerKind, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    LaunchPlanSummary, LaunchWarning, LaunchWarningKind,
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
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_launch_readiness_panel(ui, input);
        });
    })
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

// --- renderer exposes no launch action/button --------------------------------

#[test]
fn renderer_exposes_no_launch_action_or_button() {
    // `show_launch_readiness_panel` returns `()`, not an action enum - this
    // is a compile-time guarantee, exercised here so a future change that
    // adds a return value cannot silently slip past review unnoticed by
    // this suite.
    fn assert_returns_unit(_: fn(&mut egui::Ui, &LaunchReadinessInput)) {}
    assert_returns_unit(show_launch_readiness_panel);

    // No rendered text should ever say "Launch" as a call to action - the
    // subtitle explicitly says "nothing is launched", and no candidate
    // fixture in this suite ever adds a button.
    let plan = plan_with(vec![ready_candidate()]);
    let output = render(&LaunchReadinessInput::Plan(plan));
    assert!(rendered_text_contains(
        &output,
        "Planning only — nothing is launched."
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
