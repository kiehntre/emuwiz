use std::path::PathBuf;

use super::*;
use crate::launch::planning::{CandidatePreference, LaunchContentRef, ResolvedIdentity};
use crate::launch::readiness::LaunchWarningKind;
use crate::patch_manager::{XemuLaunchBlockerKind, XemuSystemFileState};

fn resolved() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Xbox".to_string(),
        game_key: "4D530058".to_string(),
    })
}

fn healthy() -> XemuHealth {
    XemuHealth {
        detected: true,
        config_readable: true,
        mcpx: XemuSystemFileState::Present,
        flash_bios: XemuSystemFileState::Present,
        eeprom: XemuSystemFileState::Present,
        hdd: XemuSystemFileState::Present,
        game_profile_mapping: crate::patch_manager::XemuGameIdMapping::VerifiedXboxTitleId,
        warnings: Vec::new(),
    }
}

fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "xemu",
            profile_id: "xemu-native".to_string(),
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

fn loose_xbe_candidate(path: Option<PathBuf>) -> LaunchCandidate {
    let mut candidate = candidate(path);
    candidate.content.kind = Some(LaunchContentKind::Executable);
    candidate
}

fn valid_binding(executable: &str) -> Result<XemuNativeLaunchBinding, XemuLaunchBlocker> {
    Ok(XemuNativeLaunchBinding {
        executable: PathBuf::from(executable),
    })
}

fn has_blocker(plan: &XemuCommandPlan, kind: LaunchBlockerKind) -> bool {
    plan.blockers.iter().any(|blocker| blocker.kind == kind)
}

#[test]
fn verified_stripped_xiso_produces_exact_command_plan() {
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/Halo/default.xiso"))),
        &valid_binding("/home/user/xemu/xemu"),
        &healthy(),
    );
    let command = plan.command.expect("verified disc image plans a command");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.executable, PathBuf::from("/home/user/xemu/xemu"));
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-dvd_path"),
            OsString::from("/games/Halo/default.xiso")
        ]
    );
    assert_eq!(command.working_directory, None);
    assert_eq!(command.selection.platform_id, "Xbox");
    assert_eq!(command.selection.verified_xbox_title_id, "4D530058");
    assert_eq!(
        command.selection.content_path,
        PathBuf::from("/games/Halo/default.xiso")
    );
}

#[test]
fn verified_redump_style_iso_produces_the_same_exact_command_shape() {
    // Redump-style full-disc dumps and stripped XISOs are indistinguishable
    // at this layer - both are already-resolved `.iso` paths by the time
    // identity/content resolution hands this planner a candidate. The
    // XGD-offset detection lives entirely in the identity layer.
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/Halo (Redump).iso"))),
        &valid_binding("/home/user/xemu/xemu"),
        &healthy(),
    );
    let command = plan.command.expect("verified disc image plans a command");
    assert!(plan.blockers.is_empty());
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-dvd_path"),
            OsString::from("/games/Halo (Redump).iso")
        ]
    );
}

#[test]
fn shell_looking_and_unicode_paths_remain_individual_arguments_never_shell_syntax() {
    let disc = PathBuf::from("/games/odd $name; \"quoted\" 日本語.iso");
    let executable = "/opt/xemu; $safe/xemu";
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &candidate(Some(disc.clone())),
        &valid_binding(executable),
        &healthy(),
    );
    let command = plan
        .command
        .expect("special characters are path data, not shell syntax");
    assert_eq!(command.executable, PathBuf::from(executable));
    assert_eq!(command.arguments.len(), 2);
    assert_eq!(command.arguments[0], OsString::from("-dvd_path"));
    assert_eq!(command.arguments[1], disc.into_os_string());
}

#[test]
fn loose_xbe_is_refused_as_runnable_content() {
    // Genuine, verified identity - but xemu cannot boot a loose XBE
    // directly, so it must never become runnable content here.
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &loose_xbe_candidate(Some(PathBuf::from("/games/default.xbe"))),
        &valid_binding("/home/user/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XemuContentFormatUnsupported
    ));
}

#[test]
fn zip_containing_one_xbe_is_refused_as_runnable_content() {
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &loose_xbe_candidate(Some(PathBuf::from("/games/Halo.zip"))),
        &valid_binding("/home/user/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XemuContentFormatUnsupported
    ));
}

#[test]
fn unresolved_and_conflicting_identity_are_rejected() {
    let unresolved = build_xemu_command_plan(
        &CanonicalIdentityStatus::Unknown,
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(unresolved.command.is_none());
    assert!(has_blocker(
        &unresolved,
        LaunchBlockerKind::IdentityUnresolved
    ));

    let conflicting = build_xemu_command_plan(
        &CanonicalIdentityStatus::Conflicting,
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(conflicting.command.is_none());
    assert!(has_blocker(
        &conflicting,
        LaunchBlockerKind::IdentityConflict
    ));
}

#[test]
fn wrong_platform_is_rejected() {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PSX".to_string(),
        game_key: "SLUS-00001".to_string(),
    });
    let plan = build_xemu_command_plan(
        &identity,
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::XemuPlatformMismatch));
}

#[test]
fn xbox_360_platform_can_never_authorize_xemu() {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Xbox360".to_string(),
        game_key: "415607D2".to_string(),
    });
    let plan = build_xemu_command_plan(
        &identity,
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::XemuPlatformMismatch));
}

#[test]
fn missing_verified_title_id_is_rejected() {
    let plan = build_xemu_command_plan(
        &resolved(),
        None,
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::XemuTitleIdMissing));
}

#[test]
fn non_xemu_candidate_is_rejected() {
    let mut other = candidate(Some(PathBuf::from("/games/default.xiso")));
    other.target = LaunchTarget::Standalone {
        adapter_id: "xenia",
        profile_id: "native".to_string(),
        profile_path: None,
    };
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &other,
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::XemuCandidateRequired));
}

#[test]
fn candidate_required_when_target_is_not_standalone() {
    let mut retroarch_target = candidate(Some(PathBuf::from("/games/default.xiso")));
    // Simulate a caller supplying a non-standalone target by pointing the
    // adapter id at something xemu-specific but marking readiness blocked -
    // the `LaunchTarget::Standalone { .. } else { ... }` arm is exercised by
    // any non-Standalone variant; RetroArchCore is the only other one.
    retroarch_target.target = LaunchTarget::RetroArchCore {
        profile: crate::emulator_environment::retroarch::ProfileRef {
            profile_kind: crate::emulator_environment::retroarch::ProfileKind::Flatpak,
            scope: crate::emulator_environment::retroarch::ProfileScope::User,
        },
        core_stem: "some_core".to_string(),
        platform_id: "Xbox",
    };
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &retroarch_target,
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::XemuCandidateRequired));
}

#[test]
fn blocked_candidate_is_not_reauthorized() {
    let mut blocked = candidate(Some(PathBuf::from("/games/default.xiso")));
    blocked.readiness = LaunchReadiness::Blocked;
    blocked.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::NoInstallationCandidate,
        "no discovered standalone profile targets this platform",
    ));
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &blocked,
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::NoInstallationCandidate
    ));
}

#[test]
fn missing_executable_is_rejected() {
    let missing = Err(XemuLaunchBlocker {
        kind: XemuLaunchBlockerKind::ExecutableMissing,
        detail: "no native xemu executable was discovered for this profile".to_string(),
    });
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &missing,
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XemuBindingUnavailable
    ));
}

#[test]
fn symlinked_or_unsafe_executable_is_rejected() {
    let unsafe_executable = Err(XemuLaunchBlocker {
        kind: XemuLaunchBlockerKind::ExecutableUnsafe,
        detail: "xemu is a symlink".to_string(),
    });
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &unsafe_executable,
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XemuBindingUnavailable
    ));
}

#[test]
fn non_executable_binary_is_rejected() {
    let not_executable = Err(XemuLaunchBlocker {
        kind: XemuLaunchBlockerKind::ExecutableNotExecutable,
        detail: "xemu is not executable".to_string(),
    });
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &not_executable,
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XemuBindingUnavailable
    ));
}

#[test]
fn each_firmware_file_missing_independently_blocks_launch() {
    for (mutate, name) in [
        (
            (|health: &mut XemuHealth| health.mcpx = XemuSystemFileState::Missing)
                as fn(&mut XemuHealth),
            "MCPX",
        ),
        (
            |health: &mut XemuHealth| health.flash_bios = XemuSystemFileState::Missing,
            "flash BIOS",
        ),
        (
            |health: &mut XemuHealth| health.eeprom = XemuSystemFileState::Missing,
            "EEPROM",
        ),
        (
            |health: &mut XemuHealth| health.hdd = XemuSystemFileState::Missing,
            "HDD",
        ),
    ] {
        let mut health = healthy();
        mutate(&mut health);
        let plan = build_xemu_command_plan(
            &resolved(),
            Some("4D530058"),
            &candidate(Some(PathBuf::from("/games/default.xiso"))),
            &valid_binding("/opt/xemu/xemu"),
            &health,
        );
        assert!(plan.command.is_none(), "{name} missing must block launch");
        assert!(
            has_blocker(&plan, LaunchBlockerKind::RequiredFirmwareMissing),
            "{name} missing must produce a RequiredFirmwareMissing blocker"
        );
    }
}

#[test]
fn present_unverified_or_unknown_firmware_never_blocks() {
    // `PresentUnverified`/`Unreadable`/`NotConfigured`/`Unknown` are never
    // treated as a proven failure - only `Missing` blocks, exactly matching
    // `xemu_firmware_readiness`'s own existing, unchanged projection.
    let mut health = healthy();
    health.mcpx = XemuSystemFileState::Present;
    health.flash_bios = XemuSystemFileState::Unreadable;
    health.eeprom = XemuSystemFileState::NotConfigured;
    health.hdd = XemuSystemFileState::Unknown;
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &candidate(Some(PathBuf::from("/games/default.xiso"))),
        &valid_binding("/opt/xemu/xemu"),
        &health,
    );
    assert!(plan.command.is_some());
    assert!(plan.blockers.is_empty());
}

#[test]
fn ambiguous_profile_is_surfaced_as_a_warning_by_the_shared_planner_not_silently_picked() {
    // Ambiguity among several eligible xemu profiles for the same identity
    // is handled once, generically, by `planning::build_launch_plan`/
    // `apply_preference` - proven here to already apply correctly to xemu
    // candidates without a second, xemu-specific ambiguity mechanism.
    use crate::emulator_environment::retroarch::RetroArchEnvironmentReport;
    use crate::launch::planning::{StandaloneProfileInput, build_launch_plan};

    let profiles = vec![
        StandaloneProfileInput {
            adapter_id: "xemu",
            profile_id: "xemu-native-one".to_string(),
            profile_path: Some(PathBuf::from("/profiles/one")),
            eligible: true,
            firmware: FirmwareReadiness::Verified,
        },
        StandaloneProfileInput {
            adapter_id: "xemu",
            profile_id: "xemu-native-two".to_string(),
            profile_path: Some(PathBuf::from("/profiles/two")),
            eligible: true,
            firmware: FirmwareReadiness::Verified,
        },
    ];
    let content = LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(PathBuf::from("/games/default.xiso")),
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
fn mounted_content_and_missing_disc_are_rejected() {
    let mut requires_mount = candidate(None);
    requires_mount.content.requires_mount = true;
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &requires_mount,
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));

    // No resolved path at all - covers both "never had one" and "the disc
    // image disappeared after planning and a fresh content-resolution pass
    // now reports nothing": this planner is pure and re-callable, so a
    // caller that re-derives `LaunchContentRef` after the fact and finds
    // `resolved_path: None` gets the identical, correct refusal here. xemu
    // has no execution/preflight abstraction to extend (unlike PCSX2/
    // DuckStation/Flycast/Dolphin) - this is the level of revalidation the
    // current architecture actually supports for this adapter.
    let missing_disc = candidate(None);
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &missing_disc,
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));
}

#[test]
fn archive_content_is_rejected_not_extracted() {
    let zip = candidate(Some(PathBuf::from("/games/Halo.zip")));
    let plan = build_xemu_command_plan(
        &resolved(),
        Some("4D530058"),
        &zip,
        &valid_binding("/opt/xemu/xemu"),
        &healthy(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::XemuContentFormatUnsupported
    ));
}
