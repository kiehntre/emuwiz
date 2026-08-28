use std::path::PathBuf;

use super::*;
use crate::launch::planning::{
    CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef, ResolvedIdentity,
};
use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
use crate::patch_manager::DuckStationLaunchBlockerKind;

fn resolved() -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PSX".to_string(),
        game_key: "SLUS-12345".to_string(),
    })
}

fn candidate(path: Option<PathBuf>) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "duckstation",
            profile_id: "duckstation-native".to_string(),
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
) -> Result<DuckStationNativeLaunchBinding, DuckStationLaunchBlocker> {
    Ok(DuckStationNativeLaunchBinding {
        executable: PathBuf::from(executable),
        user_directory_mode: DuckStationUserDirectoryMode::DefaultNative,
    })
}

fn has_blocker(plan: &DuckStationCommandPlan, kind: LaunchBlockerKind) -> bool {
    plan.blockers.iter().any(|blocker| blocker.kind == kind)
}

// --- 1/2/3: exact argv, -batch, -- separator -----------------------------------------------------

#[test]
fn default_native_produces_exact_argv() {
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/Final Fantasy VII.iso"))),
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    let command = plan.command.expect("a valid native binding plans");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.executable, PathBuf::from("/usr/bin/duckstation-qt"));
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("-batch"),
            OsString::from("--"),
            OsString::from("/games/Final Fantasy VII.iso"),
        ]
    );
    assert_eq!(command.working_directory, None);
    assert_eq!(
        command.selection.user_directory_mode,
        DuckStationUserDirectoryMode::DefaultNative
    );
    assert_eq!(command.selection.platform_id, "PSX");
    assert_eq!(command.selection.verified_ps1_serial, "SLUS-12345");
}

#[test]
fn batch_flag_is_always_first_and_double_dash_always_present() {
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.chd"))),
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    let command = plan.command.unwrap();
    assert_eq!(command.arguments[0], OsString::from("-batch"));
    assert_eq!(command.arguments[1], OsString::from("--"));
    assert_eq!(command.arguments.len(), 3);
}

// --- 4/5/6: spaces, shell metacharacters, unicode are inert data ----------------------------------

#[test]
fn path_with_spaces_is_one_argv_item() {
    let game = PathBuf::from("/games/Final Fantasy VII Disc 1.iso");
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(game.clone())),
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    let command = plan.command.unwrap();
    assert_eq!(command.arguments[2], game.into_os_string());
    assert_eq!(command.arguments.len(), 3);
}

#[test]
fn shell_metacharacters_and_unicode_paths_remain_data_not_shell_syntax() {
    let game = PathBuf::from("/games/odd $name; \"quoted\" 日本語.chd");
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(game.clone())),
        &default_native_binding("/opt/DuckStation/duckstation-qt; $safe"),
    );
    let command = plan
        .command
        .expect("special characters are path data, not shell syntax");
    assert_eq!(
        command.executable,
        PathBuf::from("/opt/DuckStation/duckstation-qt; $safe")
    );
    assert_eq!(command.arguments.len(), 3);
    assert_eq!(command.arguments[2], game.into_os_string());
}

// --- 7: wrong platform rejected --------------------------------------------------------------------

#[test]
fn wrong_platform_is_rejected() {
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "PS2".to_string(),
        game_key: "SLUS-98765".to_string(),
    });
    let plan = build_duckstation_command_plan(
        &identity,
        Some("SLUS-98765"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DuckStationPlatformMismatch
    ));
}

// --- 8: missing PS1 serial rejected ----------------------------------------------------------------

#[test]
fn missing_ps1_serial_is_rejected() {
    let plan = build_duckstation_command_plan(
        &resolved(),
        None,
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DuckStationSerialMissing
    ));
}

// --- 9: unresolved identity rejected ----------------------------------------------------------------

#[test]
fn unresolved_and_conflicting_identity_are_rejected() {
    let unresolved = build_duckstation_command_plan(
        &CanonicalIdentityStatus::Unknown,
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(has_blocker(
        &unresolved,
        LaunchBlockerKind::IdentityUnresolved
    ));

    let conflicting = build_duckstation_command_plan(
        &CanonicalIdentityStatus::Conflicting,
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(has_blocker(
        &conflicting,
        LaunchBlockerKind::IdentityConflict
    ));
}

// --- 10: unsupported binding rejected --------------------------------------------------------------

#[test]
fn unsafe_or_unsupported_binding_is_rejected() {
    let ambiguous = Err(DuckStationLaunchBlocker {
        kind: DuckStationLaunchBlockerKind::AmbiguousExecutable,
        detail: "2 viable executables match this profile".to_string(),
    });
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &ambiguous,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DuckStationBindingUnavailable
    ));

    let unsupported = Err(DuckStationLaunchBlocker {
        kind: DuckStationLaunchBlockerKind::UnsupportedInstallationType,
        detail: "only Native DuckStation installations are supported".to_string(),
    });
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &candidate(Some(PathBuf::from("/games/game.iso"))),
        &unsupported,
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DuckStationBindingUnavailable
    ));
}

// --- 11: mounted/archive content rejected -----------------------------------------------------------

#[test]
fn mounted_or_archive_content_is_rejected() {
    let mut requires_mount = candidate(None);
    requires_mount.content.requires_mount = true;
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &requires_mount,
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(&plan, LaunchBlockerKind::ContentNotResolved));

    let archive = candidate(Some(PathBuf::from("/games/archive.zip")));
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &archive,
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DuckStationContentFormatUnsupported
    ));

    // A format DuckStation itself can read directly (cue/bin), but not yet
    // classified by the current archive-kind registry as direct content -
    // refused, never guessed.
    let cue = candidate(Some(PathBuf::from("/games/game.cue")));
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &cue,
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DuckStationContentFormatUnsupported
    ));
}

#[test]
fn direct_iso_and_chd_are_both_accepted() {
    for extension in ["iso", "chd"] {
        let plan = build_duckstation_command_plan(
            &resolved(),
            Some("SLUS-12345"),
            &candidate(Some(PathBuf::from(format!("/games/game.{extension}")))),
            &default_native_binding("/usr/bin/duckstation-qt"),
        );
        assert!(plan.command.is_some(), "{extension} should be accepted");
    }
}

#[test]
fn structurally_validated_cue_bin_is_accepted() {
    let mut cue = candidate(Some(PathBuf::from("/games/game.cue")));
    cue.content.container = Some(LaunchContainerKind::CueBin);
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &cue,
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(plan.command.is_some());
    assert!(plan.blockers.is_empty());
}

// --- 12: warnings/blockers rejected -----------------------------------------------------------------

#[test]
fn blocked_candidate_is_not_reauthorized() {
    let mut blocked = candidate(Some(PathBuf::from("/games/game.iso")));
    blocked.readiness = LaunchReadiness::Blocked;
    blocked.blockers.push(LaunchBlocker::new(
        LaunchBlockerKind::RequiredFirmwareMissing,
        "required firmware is missing",
    ));
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &blocked,
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::RequiredFirmwareMissing
    ));
}

#[test]
fn non_duckstation_candidate_is_rejected() {
    let mut other = candidate(Some(PathBuf::from("/games/game.iso")));
    other.target = LaunchTarget::Standalone {
        adapter_id: "pcsx2",
        profile_id: "native".to_string(),
        profile_path: None,
    };
    let plan = build_duckstation_command_plan(
        &resolved(),
        Some("SLUS-12345"),
        &other,
        &default_native_binding("/usr/bin/duckstation-qt"),
    );
    assert!(plan.command.is_none());
    assert!(has_blocker(
        &plan,
        LaunchBlockerKind::DuckStationCandidateRequired
    ));
}
