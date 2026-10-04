use std::ffi::OsString;
use std::path::PathBuf;

use super::*;
use crate::launch::planning::{
    CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef, ResolvedIdentity,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::{Rpcs3LaunchBlocker, Rpcs3LaunchBlockerKind};

fn identity() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PS3".to_string(),
        game_key: "BLUS30000".to_string(),
    })
}

fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "rpcs3",
            profile_id: "rpcs3:/home/user/.config/rpcs3".to_string(),
            profile_path: Some(PathBuf::from("/home/user/.config/rpcs3")),
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: path,
            requires_mount: false,
            provenance: "verified PS3 content".to_string(),
        },
        firmware: FirmwareReadiness::PresentUnverified,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: LaunchReadiness::ReadyWithWarnings,
        preference: CandidatePreference::SoleEligible,
    }
}

fn binding() -> Result<Rpcs3LaunchBinding, Rpcs3LaunchBlocker> {
    Ok(Rpcs3LaunchBinding {
        executable: PathBuf::from("/opt/rpcs3/rpcs3"),
        installation: crate::launch::installation::LaunchInstallation::Native,
    })
}

fn has_blocker(plan: &Rpcs3CommandPlan, kind: LaunchBlockerKind) -> bool {
    plan.blockers.iter().any(|blocker| blocker.kind == kind)
}

#[test]
fn verified_ps3_content_produces_exact_structured_argv() {
    let content = PathBuf::from("/games/PS3 – Collection/Metal; Gear.iso");
    let plan = build_rpcs3_command_plan(
        &identity(),
        Some("BLUS30000"),
        &candidate(Some(content.clone())),
        &binding(),
    );
    let command = plan.command.expect("verified PS3 content should plan");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.executable, PathBuf::from("/opt/rpcs3/rpcs3"));
    assert_eq!(command.arguments, vec![OsString::from(content)]);
    assert_eq!(command.selection.verified_ps3_title_id, "BLUS30000");
    let spec = command.command_spec();
    assert_eq!(spec.executable, command.executable);
    assert_eq!(spec.arguments, command.arguments);
    assert_eq!(spec.working_directory, command.working_directory);
}

#[test]
fn appimage_and_flatpak_keep_rpc_content_arguments_and_visibility_narrow() {
    let content = PathBuf::from("/games/PS3 Collection/BLUS00001.iso");
    let appimage = build_rpcs3_command_plan(
        &identity(),
        Some("BLUS00001"),
        &candidate(Some(content.clone())),
        &Ok(Rpcs3LaunchBinding {
            executable: "/apps/RPCS3.AppImage".into(),
            installation: crate::launch::installation::LaunchInstallation::AppImage {
                extract_and_run: false,
            },
        }),
    )
    .command
    .unwrap();
    assert_eq!(appimage.executable, PathBuf::from("/apps/RPCS3.AppImage"));
    assert_eq!(appimage.arguments, vec![content.clone().into_os_string()]);

    let flatpak = build_rpcs3_command_plan(
        &identity(),
        Some("BLUS00001"),
        &candidate(Some(content.clone())),
        &Ok(Rpcs3LaunchBinding {
            executable: "/usr/bin/flatpak".into(),
            installation: crate::launch::installation::LaunchInstallation::flatpak(
                "net.rpcs3.RPCS3",
            )
            .unwrap(),
        }),
    )
    .command
    .unwrap();
    assert_eq!(
        flatpak.arguments,
        vec![
            "run".into(),
            "--filesystem=/games/PS3 Collection:ro".into(),
            "net.rpcs3.RPCS3".into(),
            content.into_os_string(),
        ]
    );
}

#[test]
fn flatpak_directory_content_is_granted_at_the_directory_root() {
    let dir = tempfile::tempdir().unwrap();
    let content = dir.path().join("PS3_GAME");
    std::fs::create_dir(&content).unwrap();
    let command = build_rpcs3_command_plan(
        &identity(),
        Some("BLUS00001"),
        &candidate(Some(content.clone())),
        &Ok(Rpcs3LaunchBinding {
            executable: "/usr/bin/flatpak".into(),
            installation: crate::launch::installation::LaunchInstallation::flatpak(
                "net.rpcs3.RPCS3",
            )
            .unwrap(),
        }),
    )
    .command
    .unwrap();
    assert_eq!(
        command.arguments[1],
        std::ffi::OsString::from(format!("--filesystem={}:ro", content.display()))
    );
    assert_eq!(command.arguments.last().unwrap(), &content.as_os_str());
}

#[test]
fn wrong_platform_and_psp_fact_cannot_authorize_rpcs3() {
    let wrong = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PSP".to_string(),
        game_key: "ULUS10000".to_string(),
    });
    let plan = build_rpcs3_command_plan(
        &wrong,
        None,
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &binding(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::Rpcs3PlatformMismatch));
    assert!(has_blocker(&plan, LaunchBlockerKind::Rpcs3TitleIdMissing));
}

#[test]
fn unresolved_identity_missing_title_and_unsafe_content_fail_closed() {
    let plan = build_rpcs3_command_plan(
        &CanonicalIdentityStatus::Unknown,
        None,
        &candidate(Some(PathBuf::from("/games/game.pkg"))),
        &binding(),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::IdentityUnresolved));
    assert!(has_blocker(&plan, LaunchBlockerKind::Rpcs3TitleIdMissing));
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Rpcs3ContentFormatUnsupported
    ));
}

#[test]
fn missing_or_unsafe_profile_binding_is_refused() {
    let missing = Err(Rpcs3LaunchBlocker {
        kind: Rpcs3LaunchBlockerKind::ExecutableMissing,
        detail: "no executable".to_string(),
    });
    let plan = build_rpcs3_command_plan(
        &identity(),
        Some("BLUS30000"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &missing,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Rpcs3BindingUnavailable
    ));
}

#[test]
fn unsupported_container_and_unknown_firmware_are_not_launchable() {
    let mut candidate = candidate(Some(PathBuf::from("/games/game.iso")));
    candidate.content.container = Some(LaunchContainerKind::Archive);
    candidate.firmware = FirmwareReadiness::Unknown;
    let plan = build_rpcs3_command_plan(&identity(), Some("BLUS30000"), &candidate, &binding());
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Rpcs3ContentFormatUnsupported
    ));
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Rpcs3FirmwareUnavailable
    ));
}

#[test]
fn missing_content_and_non_standalone_candidate_are_refused() {
    let mut candidate = candidate(None);
    candidate.target = LaunchTarget::RetroArchCore {
        profile: crate::emulator_environment::retroarch::ProfileRef {
            profile_kind: crate::emulator_environment::retroarch::ProfileKind::Native,
            scope: crate::emulator_environment::retroarch::ProfileScope::User,
        },
        core_stem: "some_core".to_string(),
        platform_id: "PS3",
    };
    let plan = build_rpcs3_command_plan(&identity(), Some("BLUS30000"), &candidate, &binding());
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::Rpcs3CandidateRequired
    ));
}
