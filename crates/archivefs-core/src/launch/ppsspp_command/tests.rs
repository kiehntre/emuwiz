use std::ffi::OsString;
use std::path::PathBuf;

use super::*;
use crate::launch::planning::{
    CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef, ResolvedIdentity,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::PpssppLaunchBlockerKind;

fn resolved() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PSP".to_string(),
        game_key: "ULUS-10000".to_string(),
    })
}

fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "ppsspp",
            profile_id: "ppsspp-native".to_string(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: path,
            requires_mount: false,
            provenance: "authoritatively resolved PSP content".to_string(),
        },
        firmware: FirmwareReadiness::NotRequired,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn binding() -> Result<PpssppNativeLaunchBinding, PpssppLaunchBlocker> {
    Ok(PpssppNativeLaunchBinding {
        executable: PathBuf::from("/opt/ppsspp/PPSSPP"),
    })
}

fn has_blocker(plan: &PpssppCommandPlan, kind: LaunchBlockerKind) -> bool {
    plan.blockers.iter().any(|blocker| blocker.kind == kind)
}

#[test]
fn verified_iso_produces_exact_structured_argv() {
    let path = PathBuf::from("/games/PSP titles/日本語 $game.iso");
    let plan = build_ppsspp_command_plan(
        &resolved(),
        Some("ULUS-10000"),
        &candidate(Some(path.clone())),
        &binding(),
    );
    let command = plan
        .command
        .expect("verified direct ISO should be launchable");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.executable, PathBuf::from("/opt/ppsspp/PPSSPP"));
    assert_eq!(command.arguments, vec![OsString::from(path)]);
    assert_eq!(command.selection.verified_psp_disc_id, "ULUS-10000");
}

#[test]
fn identity_must_be_resolved_for_psp() {
    for identity in [
        CanonicalIdentityStatus::Unknown,
        CanonicalIdentityStatus::Conflicting,
    ] {
        let plan = build_ppsspp_command_plan(
            &identity,
            Some("ULUS-10000"),
            &candidate(Some(PathBuf::from("/games/game.iso"))),
            &binding(),
        );
        assert!(plan.command.is_none());
    }
}

#[test]
fn wrong_platform_and_other_identity_facts_cannot_cross_authorize() {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PS2".to_string(),
        game_key: "SLUS-10000".to_string(),
    });
    let plan = build_ppsspp_command_plan(
        &identity,
        None,
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &binding(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::PpssppPlatformMismatch
    ));
    assert!(has_blocker(&plan, LaunchBlockerKind::PpssppDiscIdMissing));
}

#[test]
fn missing_disc_id_and_filename_only_content_are_refused() {
    let plan = build_ppsspp_command_plan(
        &resolved(),
        None,
        &candidate(Some(PathBuf::from("/games/ULUS-10000.iso"))),
        &binding(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::PpssppDiscIdMissing));
}

#[test]
fn only_direct_plain_iso_is_supported() {
    for (container, extension) in [
        (LaunchContainerKind::Archive, "game.iso"),
        (LaunchContainerKind::Chd, "game.chd"),
        (LaunchContainerKind::CueBin, "game.cue"),
        (LaunchContainerKind::PlainFile, "game.cso"),
        (LaunchContainerKind::PlainFile, "EBOOT.PBP"),
    ] {
        let mut game = candidate(Some(PathBuf::from(format!("/games/{extension}"))));
        game.content.container = Some(container);
        let plan = build_ppsspp_command_plan(&resolved(), Some("ULUS-10000"), &game, &binding());
        assert!(plan.command.is_none(), "{extension} must remain deferred");
        assert!(has_blocker(
            &plan,
            LaunchBlockerKind::PpssppContentFormatUnsupported
        ));
    }
}

#[test]
fn unresolved_mount_and_blocked_candidate_are_refused() {
    let mut mounted = candidate(None);
    mounted.content.requires_mount = true;
    let plan = build_ppsspp_command_plan(&resolved(), Some("ULUS-10000"), &mounted, &binding());
    assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));

    let mut blocked = candidate(Some(PathBuf::from("/games/game.iso")));
    blocked.readiness = LaunchReadiness::Blocked;
    let plan = build_ppsspp_command_plan(&resolved(), Some("ULUS-10000"), &blocked, &binding());
    assert!(has_blocker(&plan, LaunchBlockerKind::CandidateBlocked));
}

#[test]
fn unavailable_or_ambiguous_binding_is_fail_closed() {
    for kind in [
        PpssppLaunchBlockerKind::ExecutableMissing,
        PpssppLaunchBlockerKind::ExecutableUnsafe,
        PpssppLaunchBlockerKind::AmbiguousExecutable,
    ] {
        let failure = Err(PpssppLaunchBlocker {
            kind,
            detail: "test failure".to_string(),
        });
        let plan = build_ppsspp_command_plan(
            &resolved(),
            Some("ULUS-10000"),
            &candidate(Some(PathBuf::from("/games/game.iso"))),
            &failure,
        );
        assert!(plan.command.is_none());
        assert!(has_blocker(
            &plan,
            LaunchBlockerKind::PpssppBindingUnavailable
        ));
    }
}

#[test]
fn non_ppsspp_candidate_is_refused() {
    let mut game = candidate(Some(PathBuf::from("/games/game.iso")));
    game.target = LaunchTarget::Standalone {
        adapter_id: "pcsx2",
        profile_id: "pcsx2".to_string(),
        profile_path: None,
    };
    let plan = build_ppsspp_command_plan(&resolved(), Some("ULUS-10000"), &game, &binding());
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::PpssppCandidateRequired
    ));
}
