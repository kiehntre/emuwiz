//! Project the existing FS-UAE boot planner's refusal into both Blue entry points.
use super::*;

pub(crate) fn apply(plan: &mut LaunchPlan, context: &AmigaWHDLoadLaunchContext) {
    for candidate in &mut plan.candidates {
        let LaunchTarget::Standalone {
            adapter_id: "fsuae",
            profile_id,
            ..
        } = &candidate.target
        else {
            continue;
        };
        let Some((_, _, request)) = context
            .requests
            .iter()
            .find(|(adapter, profile, _)| adapter == "fsuae" && profile == profile_id)
        else {
            continue;
        };
        let boot = build_fsuae_whdload_command_plan(request);
        if !boot.blockers.is_empty() {
            candidate.blockers.extend(boot.blockers);
            candidate.readiness = LaunchReadiness::Blocked;
        }
    }
    plan.summary.ready = plan
        .candidates
        .iter()
        .filter(|c| c.readiness == LaunchReadiness::Ready)
        .count();
    plan.summary.ready_with_warnings = plan
        .candidates
        .iter()
        .filter(|c| c.readiness == LaunchReadiness::ReadyWithWarnings)
        .count();
    plan.summary.blocked = plan
        .candidates
        .iter()
        .filter(|c| c.readiness == LaunchReadiness::Blocked)
        .count();
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use archivefs_core::launch::{
        CandidatePreference, FirmwareReadiness, LaunchContentRef, LaunchPlanSummary,
    };

    #[test]
    fn missing_boot_template_blocks_panel_and_game_details_without_changing_adf() {
        let request = super::super::amiga_retry_tests::request();
        let context = AmigaWHDLoadLaunchContext {
            requests: vec![("fsuae".into(), request.profile.profile_id.clone(), request)],
            adf: vec![],
        };
        let candidate = LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "fsuae",
                profile_id: "synthetic".into(),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: None,
                container: None,
                resolved_path: Some("/disposable/game.lha".into()),
                requires_mount: false,
                provenance: "synthetic".into(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: vec![],
            warnings: vec![],
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::SoleEligible,
        };
        let mut adf = candidate.clone();
        if let LaunchTarget::Standalone { profile_id, .. } = &mut adf.target {
            *profile_id = "adf-only".into();
        }
        let mut plan = LaunchPlan {
            platform_id: Some("Amiga".into()),
            game_key: Some("synthetic".into()),
            candidates: vec![candidate, adf.clone()],
            summary: LaunchPlanSummary::default(),
            media_topology: None,
        };
        apply(&mut plan, &context);
        assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Blocked);
        assert!(
            plan.candidates[0]
                .blockers
                .iter()
                .any(|b| b.detail.contains("synthetic missing boot template"))
        );
        // Both Game Details and the Launch panel select through this shared
        // typed request gate; a missing template cannot expose a Play request.
        assert!(
            typed_launch_request(
                &plan,
                &plan.candidates[0],
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(&context),
            )
            .is_none()
        );
        assert_eq!(plan.candidates[1], adf);
        assert_eq!(plan.summary.ready, 1);
        assert_eq!(plan.summary.blocked, 1);
    }
}
