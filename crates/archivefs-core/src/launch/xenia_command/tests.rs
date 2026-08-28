use std::path::PathBuf;

use super::*;
use crate::launch::planning::{
    CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef, ResolvedIdentity,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness, LaunchWarningKind};
use crate::patch_manager::XeniaLaunchBlockerKind;

fn resolved() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Xbox360".to_string(),
        game_key: "415607D2".to_string(),
    })
}

fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "xenia",
            profile_id: "xenia-explicit-abc123".to_string(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::Executable),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: path,
            requires_mount: false,
            provenance: "already resolved content".to_string(),
        },
        firmware: FirmwareReadiness::NotRequired,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn valid_binding(executable: &str) -> Result<XeniaLaunchBinding, XeniaLaunchBlocker> {
    Ok(XeniaLaunchBinding {
        executable: PathBuf::from(executable),
    })
}

fn has_blocker(plan: &XeniaCommandPlan, kind: LaunchBlockerKind) -> bool {
    plan.blockers.iter().any(|blocker| blocker.kind == kind)
}

#[test]
fn verified_supported_content_produces_an_exact_argv_launch_plan() {
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &candidate(Some(PathBuf::from("/games/Halo 3/default.xex"))),
        &valid_binding("/home/user/xenia-canary/xenia_canary.exe"),
    );
    let command = plan.command.expect("verified content plans a command");
    assert!(plan.blockers.is_empty());
    assert_eq!(
        command.executable,
        PathBuf::from("/home/user/xenia-canary/xenia_canary.exe")
    );
    assert_eq!(
        command.arguments,
        vec![OsString::from("/games/Halo 3/default.xex")]
    );
    assert_eq!(command.working_directory, None);
    assert_eq!(command.selection.platform_id, "Xbox360");
    assert_eq!(
        command.selection.verified_xex_title_id,
        Some("415607D2".to_string())
    );
    assert_eq!(
        command.selection.verified_xex_media_id,
        Some("4C27792A".to_string())
    );
}

#[test]
fn either_title_id_or_media_id_alone_is_sufficient() {
    // Matches evidence_bridge's own "either is sufficient" resolution.
    let title_only = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        None,
        &candidate(Some(PathBuf::from("/games/game.xex"))),
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(title_only.command.is_some());

    let media_only = build_xenia_command_plan(
        &resolved(),
        None,
        Some("4C27792A"),
        &candidate(Some(PathBuf::from("/games/game.xex"))),
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(media_only.command.is_some());
}

#[test]
fn unresolved_and_conflicting_identity_are_rejected() {
    let unresolved = build_xenia_command_plan(
        &CanonicalIdentityStatus::Unknown,
        Some("415607D2"),
        Some("4C27792A"),
        &candidate(Some(PathBuf::from("/games/game.xex"))),
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(unresolved.command.is_none());
    assert!(has_blocker(
        &unresolved,
        LaunchBlockerKind::IdentityUnresolved
    ));

    let conflicting = build_xenia_command_plan(
        &CanonicalIdentityStatus::Conflicting,
        Some("415607D2"),
        Some("4C27792A"),
        &candidate(Some(PathBuf::from("/games/game.xex"))),
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(conflicting.command.is_none());
    assert!(has_blocker(
        &conflicting,
        LaunchBlockerKind::IdentityConflict
    ));
}

#[test]
fn filename_only_evidence_never_substitutes_for_verified_xex_ids() {
    // No verified title/media ID supplied at all - as if the only "evidence"
    // available were a filename - must refuse, never guess from the path.
    let plan = build_xenia_command_plan(
        &resolved(),
        None,
        None,
        &candidate(Some(PathBuf::from("/games/Halo 3 (USA)/default.xex"))),
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::XeniaTitleIdMissing));
}

#[test]
fn wrong_platform_is_rejected() {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Xbox".to_string(),
        game_key: "some-original-xbox-key".to_string(),
    });
    let plan = build_xenia_command_plan(
        &identity,
        Some("415607D2"),
        Some("4C27792A"),
        &candidate(Some(PathBuf::from("/games/game.xex"))),
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::XeniaPlatformMismatch));
}

#[test]
fn missing_emulator_executable_is_rejected() {
    let missing = Err(crate::patch_manager::XeniaLaunchBlocker {
        kind: XeniaLaunchBlockerKind::ExecutableMissing,
        detail: "xenia_canary.exe was not found in the profile's configuration directory"
            .to_string(),
    });
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &candidate(Some(PathBuf::from("/games/game.xex"))),
        &missing,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XeniaBindingUnavailable
    ));
}

#[test]
fn unsafe_binding_is_rejected() {
    let unsafe_executable = Err(crate::patch_manager::XeniaLaunchBlocker {
        kind: XeniaLaunchBlockerKind::ExecutableUnsafe,
        detail: "xenia_canary.exe is a symlink".to_string(),
    });
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &candidate(Some(PathBuf::from("/games/game.xex"))),
        &unsafe_executable,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XeniaBindingUnavailable
    ));
}

#[test]
fn ambiguous_profile_is_surfaced_as_a_warning_by_the_shared_planner_not_silently_picked() {
    // Ambiguity among several eligible Xenia profiles for the same identity
    // is handled once, generically, by `planning::build_launch_plan`/
    // `apply_preference` - this proves that existing, unmodified machinery
    // already applies correctly to Xenia candidates, without inventing a
    // second, Xenia-specific ambiguity mechanism.
    use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
    use crate::launch::planning::{StandaloneProfileInput, build_launch_plan};

    let profiles = vec![
        StandaloneProfileInput {
            adapter_id: "xenia",
            profile_id: "xenia-explicit-one".to_string(),
            profile_path: Some(PathBuf::from("/profiles/one")),
            eligible: true,
            firmware: FirmwareReadiness::NotRequired,
        },
        StandaloneProfileInput {
            adapter_id: "xenia",
            profile_id: "xenia-explicit-two".to_string(),
            profile_path: Some(PathBuf::from("/profiles/two")),
            eligible: true,
            firmware: FirmwareReadiness::NotRequired,
        },
    ];
    let content = LaunchContentRef {
        kind: Some(LaunchContentKind::Executable),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(PathBuf::from("/games/game.xex")),
        requires_mount: false,
        provenance: "already resolved content".to_string(),
    };
    let empty_retroarch = RetroArchEnvironmentReport {
        format_version: 1,
        profiles: Vec::new(),
        diagnostics: Vec::new(),
    };
    let plan = build_launch_plan(&resolved(), &content, &profiles, &empty_retroarch, &[]);
    assert_eq!(plan.candidates.len(), 2);
    for candidate in &plan.candidates {
        assert_eq!(candidate.readiness, LaunchReadiness::ReadyWithWarnings);
        assert!(
            candidate
                .warnings
                .iter()
                .any(|warning| warning.kind == LaunchWarningKind::MultipleEligibleProfiles)
        );
        assert_eq!(candidate.preference, CandidatePreference::Undetermined);
    }
}

#[test]
fn mounted_zip_and_iso_content_are_rejected_not_treated_as_runnable() {
    let mut requires_mount = candidate(None);
    requires_mount.content.requires_mount = true;
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &requires_mount,
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));

    // A ZIP containing exactly one XEX member is genuine, verified identity
    // (`game_identity::inspect_zip_xex`) but is never treated as runnable
    // content here - the archive container path itself is refused, exactly
    // like every other native launch slice in this crate.
    let zip = candidate(Some(PathBuf::from("/games/Halo 3.zip")));
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &zip,
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XeniaContentFormatUnsupported
    ));

    // Xbox 360 ISO/disc-image parsing is not modeled by this build at all -
    // refused as an unsupported format, never guessed at.
    let iso = candidate(Some(PathBuf::from("/games/Halo 3.iso")));
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &iso,
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XeniaContentFormatUnsupported
    ));
}

#[test]
fn shell_looking_and_unicode_paths_remain_individual_arguments_never_shell_syntax() {
    let game = PathBuf::from("/games/odd $name; \"quoted\" 日本語.xex");
    let executable = "/opt/Xenia Canary; $safe/xenia_canary.exe";
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &candidate(Some(game.clone())),
        &valid_binding(executable),
    );
    let command = plan
        .command
        .expect("special characters are path data, not shell syntax");
    assert_eq!(command.executable, PathBuf::from(executable));
    assert_eq!(command.arguments.len(), 1);
    assert_eq!(command.arguments[0], game.into_os_string());
}

#[test]
fn non_xenia_candidate_is_rejected() {
    let mut other = candidate(Some(PathBuf::from("/games/game.xex")));
    other.target = LaunchTarget::Standalone {
        adapter_id: "pcsx2",
        profile_id: "native".to_string(),
        profile_path: None,
    };
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &other,
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XeniaCandidateRequired
    ));
}

#[test]
fn blocked_candidate_is_not_reauthorized() {
    let mut blocked = candidate(Some(PathBuf::from("/games/game.xex")));
    blocked.readiness = LaunchReadiness::Blocked;
    blocked.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::NoInstallationCandidate,
        "no discovered standalone profile targets this platform",
    ));
    let plan = build_xenia_command_plan(
        &resolved(),
        Some("415607D2"),
        Some("4C27792A"),
        &blocked,
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::NoInstallationCandidate
    ));
}

#[test]
fn original_xbox_platform_and_facts_are_never_conflated_with_xbox_360() {
    // `VerifiedIdentityFact::XboxTitleId` names the original Xbox, not the
    // 360 - this command plan never reads or produces that variant, and a
    // resolved identity naming the original `Xbox` platform id is rejected
    // as a platform mismatch (see `wrong_platform_is_rejected`), never
    // silently accepted as if it were Xbox 360.
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Xbox".to_string(),
        game_key: "original-xbox-key".to_string(),
    });
    let plan = build_xenia_command_plan(
        &identity,
        Some("415607D2"),
        Some("4C27792A"),
        &candidate(Some(PathBuf::from("/games/game.xex"))),
        &valid_binding("/opt/xenia/xenia_canary.exe"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::XeniaPlatformMismatch));
}
