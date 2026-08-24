use std::path::PathBuf;

use super::*;
use crate::launch::planning::{
    CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef, ResolvedIdentity,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::DolphinLaunchBlockerKind;

fn resolved() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "GameCube".to_string(),
        game_key: "GALE01".to_string(),
    })
}

fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "dolphin",
            profile_id: "dolphin-native".to_string(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
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

fn default_native_binding(
    executable: &str,
) -> Result<DolphinNativeLaunchBinding, DolphinLaunchBlocker> {
    Ok(DolphinNativeLaunchBinding {
        executable: PathBuf::from(executable),
        user_directory_mode: DolphinUserDirectoryMode::DefaultNative,
    })
}

fn explicit_root_binding(
    executable: &str,
    root: &str,
) -> Result<DolphinNativeLaunchBinding, DolphinLaunchBlocker> {
    Ok(DolphinNativeLaunchBinding {
        executable: PathBuf::from(executable),
        user_directory_mode: DolphinUserDirectoryMode::ExplicitRoot(PathBuf::from(root)),
    })
}

fn has_blocker(plan: &DolphinCommandPlan, kind: LaunchBlockerKind) -> bool {
    plan.blockers.iter().any(|blocker| blocker.kind == kind)
}

#[test]
fn default_native_produces_exact_argv() {
    let plan = build_dolphin_command_plan(
        &resolved(),
        &candidate(Some(PathBuf::from("/games/Wind Waker.iso"))),
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    let command = plan.command.expect("a valid native binding plans");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.executable, PathBuf::from("/usr/bin/dolphin-emu"));
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-e"),
            OsString::from("/games/Wind Waker.iso"),
        ]
    );
    assert_eq!(command.working_directory, None);
    assert_eq!(
        command.selection.user_directory_mode,
        DolphinUserDirectoryMode::DefaultNative
    );
    assert_eq!(command.selection.platform_id, "GameCube");
    assert_eq!(command.selection.game_id, "GALE01");
}

#[test]
fn explicit_root_produces_exact_argv() {
    let plan = build_dolphin_command_plan(
        &resolved(),
        &candidate(Some(PathBuf::from("/games/game.gcm"))),
        &explicit_root_binding("/opt/dolphin/dolphin-emu", "/profiles/dolphin-portable"),
    );
    let command = plan.command.expect("a valid explicit-root binding plans");
    assert!(plan.blockers.is_empty());
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-u"),
            OsString::from("/profiles/dolphin-portable"),
            OsString::from("-e"),
            OsString::from("/games/game.gcm"),
        ]
    );
}

#[test]
fn gcm_extension_is_accepted() {
    let plan = build_dolphin_command_plan(
        &resolved(),
        &candidate(Some(PathBuf::from("/games/game.gcm"))),
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(plan.command.is_some());
}

#[test]
fn non_gamecube_extension_is_rejected() {
    let plan = build_dolphin_command_plan(
        &resolved(),
        &candidate(Some(PathBuf::from("/games/game.rvz"))),
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DolphinContentFormatUnsupported
    ));
}

#[test]
fn wrong_platform_is_rejected() {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Wii".to_string(),
        game_key: "RALE01".to_string(),
    });
    let plan = build_dolphin_command_plan(
        &identity,
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DolphinPlatformMismatch
    ));
}

#[test]
fn unresolved_and_conflicting_identity_are_rejected() {
    let unresolved = build_dolphin_command_plan(
        &CanonicalIdentityStatus::Unknown,
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(has_blocker(
        &unresolved,
        LaunchBlockerKind::IdentityUnresolved
    ));

    let conflicting = build_dolphin_command_plan(
        &CanonicalIdentityStatus::Conflicting,
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(has_blocker(
        &conflicting,
        LaunchBlockerKind::IdentityConflict
    ));
}

#[test]
fn unsafe_or_unsupported_binding_is_rejected() {
    let ambiguous = Err(crate::patch_manager::DolphinLaunchBlocker {
        kind: DolphinLaunchBlockerKind::AmbiguousExecutable,
        detail: "2 viable executables match this profile".to_string(),
    });
    let plan = build_dolphin_command_plan(
        &resolved(),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &ambiguous,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DolphinBindingUnavailable
    ));

    let unsupported = Err(crate::patch_manager::DolphinLaunchBlocker {
        kind: DolphinLaunchBlockerKind::UnsupportedInstallationType,
        detail: "Flatpak Dolphin installations are not supported".to_string(),
    });
    let plan = build_dolphin_command_plan(
        &resolved(),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &unsupported,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DolphinBindingUnavailable
    ));
}

#[test]
fn mounted_or_archive_content_is_rejected() {
    let mut requires_mount = candidate(None);
    requires_mount.content.requires_mount = true;
    let plan = build_dolphin_command_plan(
        &resolved(),
        &requires_mount,
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));

    let archive = candidate(Some(PathBuf::from("/games/archive.zip")));
    let plan = build_dolphin_command_plan(
        &resolved(),
        &archive,
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DolphinContentFormatUnsupported
    ));
}

#[test]
fn shell_looking_and_unicode_paths_remain_individual_arguments() {
    let game = PathBuf::from("/games/odd $name; \"quoted\" 日本語.iso");
    let root = "/profiles/odd root; $value \"日本語\"";
    let plan = build_dolphin_command_plan(
        &resolved(),
        &candidate(Some(game.clone())),
        &explicit_root_binding("/opt/Dolphin Emu/dolphin-emu; $safe", root),
    );
    let command = plan
        .command
        .expect("special characters are path data, not shell syntax");
    assert_eq!(
        command.executable,
        PathBuf::from("/opt/Dolphin Emu/dolphin-emu; $safe")
    );
    assert_eq!(command.arguments.len(), 4);
    assert_eq!(command.arguments[0], OsString::from("-u"));
    assert_eq!(command.arguments[1], OsString::from(root));
    assert_eq!(command.arguments[2], OsString::from("-e"));
    assert_eq!(command.arguments[3], game.into_os_string());
}

#[test]
fn non_dolphin_candidate_is_rejected() {
    let mut other = candidate(Some(PathBuf::from("/games/game.iso")));
    other.target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "native".to_string(),
        profile_path: None,
    };
    let plan = build_dolphin_command_plan(
        &resolved(),
        &other,
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DolphinCandidateRequired
    ));
}

#[test]
fn blocked_candidate_is_not_reauthorized() {
    let mut blocked = candidate(Some(PathBuf::from("/games/game.iso")));
    blocked.readiness = LaunchReadiness::Blocked;
    blocked.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::ProfileIneligible,
        "the discovered profile is not eligible",
    ));
    let plan = build_dolphin_command_plan(
        &resolved(),
        &blocked,
        &default_native_binding("/usr/bin/dolphin-emu"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::ProfileIneligible));
}
