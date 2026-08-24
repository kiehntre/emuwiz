use std::path::PathBuf;

use super::*;
use crate::launch::planning::{
    CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef, ResolvedIdentity,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::Pcsx2LaunchBlockerKind;

fn resolved() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PS2".to_string(),
        game_key: "SLUS-12345".to_string(),
    })
}

fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "pcsx2",
            profile_id: "pcsx2-native".to_string(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: path,
            requires_mount: false,
            provenance: "already resolved content".to_string(),
        },
        firmware: FirmwareReadiness::Verified,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn default_native_binding(
    executable: &str,
) -> Result<Pcsx2NativeLaunchBinding, Pcsx2LaunchBlocker> {
    Ok(Pcsx2NativeLaunchBinding {
        executable: PathBuf::from(executable),
        user_directory_mode: Pcsx2UserDirectoryMode::DefaultNative,
    })
}

fn explicit_datapath_binding(
    executable: &str,
    root: &str,
) -> Result<Pcsx2NativeLaunchBinding, Pcsx2LaunchBlocker> {
    Ok(Pcsx2NativeLaunchBinding {
        executable: PathBuf::from(executable),
        user_directory_mode: Pcsx2UserDirectoryMode::ExplicitDataPath(PathBuf::from(root)),
    })
}

fn has_blocker(plan: &Pcsx2CommandPlan, kind: LaunchBlockerKind) -> bool {
    plan.blockers.iter().any(|blocker| blocker.kind == kind)
}

#[test]
fn default_native_produces_exact_argv() {
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/Final Fantasy X.iso"))),
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    let command = plan.command.expect("a valid native binding plans");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.executable, PathBuf::from("/usr/bin/pcsx2-qt"));
    assert_eq!(
        command.arguments,
        vec![OsString::from("/games/Final Fantasy X.iso")]
    );
    assert_eq!(command.working_directory, None);
    assert_eq!(
        command.selection.user_directory_mode,
        Pcsx2UserDirectoryMode::DefaultNative
    );
    assert_eq!(command.selection.platform_id, "PS2");
    assert_eq!(command.selection.verified_ps2_serial, "SLUS-12345");
}

#[test]
fn explicit_datapath_produces_exact_argv() {
    // Not currently produced by `resolve_pcsx2_native_launch_binding`, but
    // the planner must still render it correctly if a binding ever legally
    // returns it.
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &explicit_datapath_binding("/opt/pcsx2/pcsx2-qt", "/profiles/pcsx2-portable"),
    );
    let command = plan
        .command
        .expect("a valid explicit-datapath binding plans");
    assert!(plan.blockers.is_empty());
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-datapath"),
            OsString::from("/profiles/pcsx2-portable"),
            OsString::from("/games/game.iso"),
        ]
    );
}

#[test]
fn direct_iso_is_accepted() {
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(plan.command.is_some());
}

#[test]
fn wrong_platform_is_rejected() {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PSX".to_string(),
        game_key: "SLUS-00001".to_string(),
    });
    let plan = build_pcsx2_command_plan(
        &identity,
        Some("SLUS-00001"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::Pcsx2PlatformMismatch));
}

#[test]
fn missing_ps2_serial_is_rejected() {
    let plan = build_pcsx2_command_plan(
        &resolved(),
        None,
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::Pcsx2SerialMissing));
}

#[test]
fn unresolved_and_conflicting_identity_are_rejected() {
    let unresolved = build_pcsx2_command_plan(
        &CanonicalIdentityStatus::Unknown,
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(has_blocker(
        &unresolved,
        LaunchBlockerKind::IdentityUnresolved
    ));

    let conflicting = build_pcsx2_command_plan(
        &CanonicalIdentityStatus::Conflicting,
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(has_blocker(
        &conflicting,
        LaunchBlockerKind::IdentityConflict
    ));
}

#[test]
fn unsafe_or_unsupported_binding_is_rejected() {
    let ambiguous = Err(crate::patch_manager::Pcsx2LaunchBlocker {
        kind: Pcsx2LaunchBlockerKind::AmbiguousExecutable,
        detail: "2 viable executables match this profile".to_string(),
    });
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &ambiguous,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Pcsx2BindingUnavailable
    ));

    let unsupported = Err(crate::patch_manager::Pcsx2LaunchBlocker {
        kind: Pcsx2LaunchBlockerKind::UnsupportedInstallationType,
        detail: "only Native PCSX2 installations are supported".to_string(),
    });
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &unsupported,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Pcsx2BindingUnavailable
    ));
}

#[test]
fn mounted_or_archive_content_is_rejected() {
    let mut requires_mount = candidate(None);
    requires_mount.content.requires_mount = true;
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &requires_mount,
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));

    let archive = candidate(Some(PathBuf::from("/games/archive.zip")));
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &archive,
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Pcsx2ContentFormatUnsupported
    ));

    let chd = candidate(Some(PathBuf::from("/games/game.chd")));
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &chd,
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Pcsx2ContentFormatUnsupported
    ));
}

#[test]
fn shell_looking_and_unicode_paths_remain_individual_arguments() {
    let game = PathBuf::from("/games/odd $name; \"quoted\" 日本語.iso");
    let root = "/profiles/odd root; $value \"日本語\"";
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(game.clone())),
        &explicit_datapath_binding("/opt/PCSX2/pcsx2-qt; $safe", root),
    );
    let command = plan
        .command
        .expect("special characters are path data, not shell syntax");
    assert_eq!(
        command.executable,
        PathBuf::from("/opt/PCSX2/pcsx2-qt; $safe")
    );
    assert_eq!(command.arguments.len(), 3);
    assert_eq!(command.arguments[0], OsString::from("-datapath"));
    assert_eq!(command.arguments[1], OsString::from(root));
    assert_eq!(command.arguments[2], game.into_os_string());
}

#[test]
fn non_pcsx2_candidate_is_rejected() {
    let mut other = candidate(Some(PathBuf::from("/games/game.iso")));
    other.target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "native".to_string(),
        profile_path: None,
    };
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &other,
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Pcsx2CandidateRequired
    ));
}

#[test]
fn blocked_candidate_is_not_reauthorized() {
    let mut blocked = candidate(Some(PathBuf::from("/games/game.iso")));
    blocked.readiness = LaunchReadiness::Blocked;
    blocked.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::RequiredFirmwareMissing,
        "required firmware is missing",
    ));
    let plan = build_pcsx2_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &blocked,
        &default_native_binding("/usr/bin/pcsx2-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::RequiredFirmwareMissing
    ));
}
