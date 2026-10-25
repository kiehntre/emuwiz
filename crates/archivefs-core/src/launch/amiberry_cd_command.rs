//! Native Amiberry CD32/CDTV planning.
//!
//! This is deliberately separate from the ordinary-Amiga planner.  The
//! profile is the complete machine/media configuration; the command only
//! selects that already-reviewed profile and never synthesizes mounts.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::amiga_cd_evidence::{
    AMIGA_CD32_PLATFORM_ID, AMIGA_CDTV_PLATFORM_ID, AmigaCdFirmwareState, AmigaCdMachine,
    AmigaCdMachineReadiness, AmigaCdMediaFormat,
};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::AmigaMachineProfile;

pub const AMIBERRY_CD_CONFIG_FLAG: &str = "--config";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCdFileBinding {
    pub path: PathBuf,
    pub identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCdLaunchRequest {
    pub executable: PathBuf,
    pub profile: PathBuf,
    pub canonical_platform: String,
    pub machine: AmigaCdMachine,
    pub machine_model: String,
    pub selected_content: AmiberryCdFileBinding,
    pub media_format: AmigaCdMediaFormat,
    pub media_dependencies: Vec<AmiberryCdFileBinding>,
    pub firmware_main: AmiberryCdFileBinding,
    pub firmware_extended: AmiberryCdFileBinding,
    pub readiness: AmigaCdMachineReadiness,
    pub identity: CanonicalIdentityStatus,
    pub profile_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
    /// Evidence that the selected CD is already mounted by the profile.
    pub profile_media_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCdCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub machine: AmigaCdMachine,
    pub platform_id: String,
    pub profile: PathBuf,
    pub content: PathBuf,
    pub media_format: AmigaCdMediaFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiberryCdCommandPlan {
    pub command: Option<AmiberryCdCommand>,
    pub blockers: Vec<LaunchBlocker>,
    pub readiness: LaunchReadiness,
}

fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

fn platform(machine: AmigaCdMachine) -> Option<&'static str> {
    match machine {
        AmigaCdMachine::Cd32 => Some(AMIGA_CD32_PLATFORM_ID),
        AmigaCdMachine::Cdtv => Some(AMIGA_CDTV_PLATFORM_ID),
        AmigaCdMachine::OrdinaryAmiga => None,
    }
}

fn firmware_ok(state: AmigaCdFirmwareState) -> bool {
    matches!(
        state,
        AmigaCdFirmwareState::Verified | AmigaCdFirmwareState::PresentUnverified
    )
}

/// Builds the native profile-selection command for a proven CD32/CDTV setup.
/// CHD is intentionally blocked: the shared media evidence recognizes it,
/// but this build has no Amiberry-specific CHD command/profile proof.
pub fn build_amiberry_cd_command_plan(
    request: &AmiberryCdLaunchRequest,
    machine_profile: &AmigaMachineProfile,
) -> AmiberryCdCommandPlan {
    let mut blockers = Vec::new();
    let Some(expected_platform) = platform(request.machine) else {
        blockers.push(block(
            LaunchBlockerKind::AmiberryPlatformMismatch,
            "ordinary Amiga is not a CD launch target",
        ));
        return blocked(blockers);
    };
    if request.canonical_platform != expected_platform {
        blockers.push(block(
            LaunchBlockerKind::AmiberryPlatformMismatch,
            "CD machine and canonical platform do not match",
        ));
    }
    match &request.identity {
        CanonicalIdentityStatus::Resolved(identity)
            if identity.platform_id == expected_platform => {}
        CanonicalIdentityStatus::Resolved(_) => blockers.push(block(
            LaunchBlockerKind::AmiberryPlatformMismatch,
            "identity is not the selected CD machine",
        )),
        CanonicalIdentityStatus::Unknown => blockers.push(block(
            LaunchBlockerKind::IdentityUnresolved,
            "CD identity is unresolved",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(block(
            LaunchBlockerKind::IdentityConflict,
            "CD identity evidence conflicts",
        )),
    }
    if request.readiness.machine != request.machine
        || request.readiness.platform_id.as_deref() != Some(expected_platform)
        || !request.readiness.blockers.is_empty()
    {
        blockers.push(block(
            LaunchBlockerKind::AmiberryMachineAmbiguous,
            "shared CD evidence is not ready for the selected machine",
        ));
    }
    if machine_profile.machine_model.as_deref() != Some(request.machine_model.as_str()) {
        blockers.push(block(
            LaunchBlockerKind::AmiberryMachineAmbiguous,
            "machine model is not explicit in the selected profile",
        ));
    }
    if !firmware_ok(request.readiness.firmware_evidence.main_kickstart)
        || !firmware_ok(request.readiness.firmware_evidence.extended_rom)
    {
        blockers.push(block(
            LaunchBlockerKind::AmiberryKickstartUnavailable,
            "CD32/CDTV firmware evidence is not usable",
        ));
    }
    if !request.readiness.media_evidence.complete {
        blockers.push(block(
            LaunchBlockerKind::AmiberryMediaNotConfigured,
            "CD media is incomplete",
        ));
    }
    if !request.profile_media_configured {
        blockers.push(block(
            LaunchBlockerKind::AmiberryMediaNotConfigured,
            "selected CD media is not explicitly configured by the profile",
        ));
    }
    if request.media_format == AmigaCdMediaFormat::Chd {
        blockers.push(block(
            LaunchBlockerKind::AmiberryContentFormatUnsupported,
            "Amiberry CHD CD semantics are not proven",
        ));
    }
    if !matches!(
        request.media_format,
        AmigaCdMediaFormat::CueBin | AmigaCdMediaFormat::Iso
    ) {
        blockers.push(block(
            LaunchBlockerKind::AmiberryContentFormatUnsupported,
            "media format is outside the proven Amiberry CD slice",
        ));
    }
    if !blockers.is_empty() {
        return blocked(blockers);
    }
    let warning = request.readiness.readiness == LaunchReadiness::ReadyWithWarnings;
    AmiberryCdCommandPlan {
        command: Some(AmiberryCdCommand {
            executable: request.executable.clone(),
            arguments: vec![
                OsString::from(AMIBERRY_CD_CONFIG_FLAG),
                request.profile.clone().into_os_string(),
            ],
            working_directory: request
                .selected_content
                .path
                .parent()
                .map(Path::to_path_buf),
            machine: request.machine,
            platform_id: expected_platform.into(),
            profile: request.profile.clone(),
            content: request.selected_content.path.clone(),
            media_format: request.media_format,
        }),
        blockers,
        readiness: if warning {
            LaunchReadiness::ReadyWithWarnings
        } else {
            LaunchReadiness::Ready
        },
    }
}

fn blocked(blockers: Vec<LaunchBlocker>) -> AmiberryCdCommandPlan {
    AmiberryCdCommandPlan {
        command: None,
        blockers,
        readiness: LaunchReadiness::Blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amiga_cd_evidence::{
        AmigaCdEvidenceSource, AmigaCdFirmwareEvidence, AmigaCdIdentityEvidence,
        AmigaCdMediaEvidence, AmigaCdPlatformClaim, assess_amiga_cd_readiness,
    };
    use crate::launch::planning::ResolvedIdentity;
    use std::fs;

    fn binding(path: &str) -> AmiberryCdFileBinding {
        let meta = fs::metadata("/dev/null").unwrap();
        AmiberryCdFileBinding {
            path: path.into(),
            identity: CapturedFileIdentity::capture(&meta),
        }
    }
    fn request(
        machine: AmigaCdMachine,
        format: AmigaCdMediaFormat,
    ) -> (AmiberryCdLaunchRequest, AmigaMachineProfile) {
        let platform = platform(machine).unwrap();
        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: "disc".into(),
        });
        let evidence = AmigaCdIdentityEvidence {
            claims: vec![AmigaCdPlatformClaim {
                machine,
                source: AmigaCdEvidenceSource::ProviderDat,
            }],
        };
        let readiness = assess_amiga_cd_readiness(
            &identity,
            &evidence,
            machine,
            AmigaCdFirmwareEvidence {
                main_kickstart: AmigaCdFirmwareState::Verified,
                extended_rom: AmigaCdFirmwareState::Verified,
            },
            AmigaCdMediaEvidence {
                format,
                complete: true,
                identified_platform: Some(machine),
            },
        );
        let model = if machine == AmigaCdMachine::Cd32 {
            "CD32"
        } else {
            "CDTV"
        };
        let profile = "/profiles/cd.uae".into();
        let id = binding("/dev/null").identity;
        (
            AmiberryCdLaunchRequest {
                executable: "/bin/amiberry".into(),
                profile,
                canonical_platform: platform.into(),
                machine,
                machine_model: model.into(),
                selected_content: binding("/games/My Disc/game.iso"),
                media_format: format,
                media_dependencies: vec![],
                firmware_main: binding("/roms/main.rom"),
                firmware_extended: binding("/roms/extended.rom"),
                readiness,
                identity,
                profile_identity: id,
                executable_identity: id,
                profile_media_configured: true,
            },
            AmigaMachineProfile {
                machine_model: Some(model.into()),
                ..Default::default()
            },
        )
    }
    #[test]
    fn cd32_and_cdtv_are_distinct_native_targets() {
        for machine in [AmigaCdMachine::Cd32, AmigaCdMachine::Cdtv] {
            let (r, m) = request(machine, AmigaCdMediaFormat::Iso);
            let p = build_amiberry_cd_command_plan(&r, &m);
            assert!(p.command.is_some());
            assert_eq!(p.command.unwrap().machine, machine);
        }
    }
    #[test]
    fn ordinary_and_chd_fail_closed() {
        let (mut r, m) = request(AmigaCdMachine::Cd32, AmigaCdMediaFormat::Chd);
        assert!(build_amiberry_cd_command_plan(&r, &m).command.is_none());
        r.machine = AmigaCdMachine::OrdinaryAmiga;
        assert!(build_amiberry_cd_command_plan(&r, &m).command.is_none());
    }
    #[test]
    fn unverified_firmware_warns_but_missing_blocks() {
        let (mut r, m) = request(AmigaCdMachine::Cd32, AmigaCdMediaFormat::Iso);
        r.readiness.firmware_evidence.main_kickstart = AmigaCdFirmwareState::PresentUnverified;
        r.readiness.firmware_evidence.extended_rom = AmigaCdFirmwareState::PresentUnverified;
        r.readiness.readiness = LaunchReadiness::ReadyWithWarnings;
        assert_eq!(
            build_amiberry_cd_command_plan(&r, &m).readiness,
            LaunchReadiness::ReadyWithWarnings
        );
        r.readiness.firmware_evidence.extended_rom = AmigaCdFirmwareState::Missing;
        assert!(build_amiberry_cd_command_plan(&r, &m).command.is_none());
    }
    #[test]
    fn profile_media_and_identity_are_required() {
        let (mut r, m) = request(AmigaCdMachine::Cd32, AmigaCdMediaFormat::CueBin);
        r.profile_media_configured = false;
        assert!(build_amiberry_cd_command_plan(&r, &m).command.is_none());
        r.profile_media_configured = true;
        r.canonical_platform = crate::amiga_cd_evidence::AMIGA_CDTV_PLATFORM_ID.into();
        assert!(build_amiberry_cd_command_plan(&r, &m).command.is_none());
    }
}
