use std::path::PathBuf;

use super::*;
use crate::launch::planning::{
    CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef, ResolvedIdentity,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::FlycastLaunchBlockerKind;

fn resolved() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Dreamcast".to_string(),
        game_key: "T-8109N".to_string(),
    })
}

fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "flycast",
            profile_id: "flycast-native".to_string(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: path,
            requires_mount: false,
            provenance: "already resolved content".to_string(),
        },
        firmware: FirmwareReadiness::Unknown,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn native_binding(executable: &str) -> Result<FlycastNativeLaunchBinding, FlycastLaunchBlocker> {
    Ok(FlycastNativeLaunchBinding {
        executable: PathBuf::from(executable),
    })
}

fn has_blocker(plan: &FlycastCommandPlan, kind: LaunchBlockerKind) -> bool {
    plan.blockers.iter().any(|blocker| blocker.kind == kind)
}

// --- 1: exact argv, no shell, content path as its own argument ---------------------------------

#[test]
fn native_produces_exact_argv() {
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &candidate(Some(PathBuf::from("/games/Crazy Taxi.iso"))),
        &native_binding("/usr/bin/flycast"),
    );
    let command = plan.command.expect("a valid native binding plans");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.executable, PathBuf::from("/usr/bin/flycast"));
    assert_eq!(
        command.arguments,
        vec![OsString::from("/games/Crazy Taxi.iso")]
    );
    assert_eq!(command.working_directory, None);
    assert_eq!(command.selection.platform_id, "Dreamcast");
    assert_eq!(command.selection.verified_dreamcast_product_code, "T-8109N");
}

// --- 2/3: spaces, shell metacharacters, unicode are inert data ----------------------------------

#[test]
fn path_with_spaces_is_one_argv_item() {
    let game = PathBuf::from("/games/Sonic Adventure Disc 1.cdi.iso");
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &candidate(Some(game.clone())),
        &native_binding("/usr/bin/flycast"),
    );
    let command = plan.command.unwrap();
    assert_eq!(command.arguments[0], game.into_os_string());
    assert_eq!(command.arguments.len(), 1);
}

#[test]
fn shell_metacharacters_and_unicode_paths_remain_data_not_shell_syntax() {
    let game = PathBuf::from("/games/odd $name; \"quoted\" 日本語.chd");
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &candidate(Some(game.clone())),
        &native_binding("/opt/Flycast/flycast; $safe"),
    );
    let command = plan
        .command
        .expect("special characters are path data, not shell syntax");
    assert_eq!(
        command.executable,
        PathBuf::from("/opt/Flycast/flycast; $safe")
    );
    assert_eq!(command.arguments.len(), 1);
    assert_eq!(command.arguments[0], game.into_os_string());
}

// --- 4: wrong platform rejected ------------------------------------------------------------------

#[test]
fn wrong_platform_is_rejected() {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PSX".to_string(),
        game_key: "SLUS-12345".to_string(),
    });
    let plan = build_flycast_command_plan(
        &identity,
        Some("T-8109N"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &native_binding("/usr/bin/flycast"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::FlycastPlatformMismatch
    ));
}

// --- 5: missing product code rejected --------------------------------------------------------------

#[test]
fn missing_product_code_is_rejected() {
    let plan = build_flycast_command_plan(
        &resolved(),
        None,
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &native_binding("/usr/bin/flycast"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::FlycastProductCodeMissing
    ));
}

// --- 6: unresolved/conflicting identity rejected -----------------------------------------------------

#[test]
fn unresolved_and_conflicting_identity_are_rejected() {
    let unresolved = build_flycast_command_plan(
        &CanonicalIdentityStatus::Unknown,
        Some("T-8109N"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &native_binding("/usr/bin/flycast"),
    );
    assert!(has_blocker(
        &unresolved,
        LaunchBlockerKind::IdentityUnresolved
    ));

    let conflicting = build_flycast_command_plan(
        &CanonicalIdentityStatus::Conflicting,
        Some("T-8109N"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &native_binding("/usr/bin/flycast"),
    );
    assert!(has_blocker(
        &conflicting,
        LaunchBlockerKind::IdentityConflict
    ));
}

// --- 7: unsafe/unsupported binding rejected ----------------------------------------------------------

#[test]
fn unsafe_or_unsupported_binding_is_rejected() {
    let ambiguous = Err(FlycastLaunchBlocker {
        kind: FlycastLaunchBlockerKind::AmbiguousExecutable,
        detail: "2 viable executables match this profile".to_string(),
    });
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &ambiguous,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::FlycastBindingUnavailable
    ));

    let ineligible = Err(FlycastLaunchBlocker {
        kind: FlycastLaunchBlockerKind::ProfileIneligible,
        detail: "profile is not eligible".to_string(),
    });
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &ineligible,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::FlycastBindingUnavailable
    ));
}

// --- 8: mounted/archive/unsupported-format content rejected --------------------------------------------

#[test]
fn mounted_or_archive_content_is_rejected() {
    let mut requires_mount = candidate(None);
    requires_mount.content.requires_mount = true;
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &requires_mount,
        &native_binding("/usr/bin/flycast"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));

    let archive = candidate(Some(PathBuf::from("/games/archive.zip")));
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &archive,
        &native_binding("/usr/bin/flycast"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::FlycastContentFormatUnsupported
    ));
}

#[test]
fn verified_direct_cdi_is_accepted_by_the_native_gate() {
    let extension = "cdi";
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &candidate(Some(PathBuf::from(format!("/games/game.{extension}")))),
        &native_binding("/usr/bin/flycast"),
    );
    assert!(plan.command.is_some(), "{extension} should be accepted");
}

#[test]
fn direct_supported_flycast_media_are_accepted() {
    for extension in ["iso", "cue", "gdi", "chd", "cdi"] {
        let plan = build_flycast_command_plan(
            &resolved(),
            Some("T-8109N"),
            &candidate(Some(PathBuf::from(format!("/games/game.{extension}")))),
            &native_binding("/usr/bin/flycast"),
        );
        assert!(plan.command.is_some(), "{extension} should be accepted");
    }
}

// --- 9: blocked candidate is not reauthorized -----------------------------------------------------

#[test]
fn blocked_candidate_is_not_reauthorized() {
    let mut blocked = candidate(Some(PathBuf::from("/games/game.iso")));
    blocked.readiness = LaunchReadiness::Blocked;
    blocked.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::RequiredFirmwareMissing,
        "required firmware is missing",
    ));
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &blocked,
        &native_binding("/usr/bin/flycast"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::RequiredFirmwareMissing
    ));
}

#[test]
fn non_flycast_candidate_is_rejected() {
    let mut other = candidate(Some(PathBuf::from("/games/game.iso")));
    other.target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "native".to_string(),
        profile_path: None,
    };
    let plan = build_flycast_command_plan(
        &resolved(),
        Some("T-8109N"),
        &other,
        &native_binding("/usr/bin/flycast"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::FlycastCandidateRequired
    ));
}
