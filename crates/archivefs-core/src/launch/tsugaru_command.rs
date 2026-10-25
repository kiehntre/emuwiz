//! Typed Tsugaru FM Towns command planning.
//!
//! V1 uses only the documented CD-image command path. Tsugaru's `-FD0` and
//! `-HD0` inputs are deliberately not inferred from EmuWiz D88/HDI/NHD
//! evidence because the upstream contract names raw/D77 and UNZ-compatible
//! hard-disk images, not those container extensions.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{FirmwareReadiness, LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::TsugaruProfile;

pub const TSUGARU_SUPPORTED_PLATFORM_ID: &str = "FM Towns";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsugaruMediaFormat {
    Iso,
    Cue,
    Mds,
}

pub fn tsugaru_media_format(path: &Path) -> Option<TsugaruMediaFormat> {
    match path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "iso" => Some(TsugaruMediaFormat::Iso),
        "cue" => Some(TsugaruMediaFormat::Cue),
        "mds" => Some(TsugaruMediaFormat::Mds),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsugaruCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub rom_directory: PathBuf,
    pub content_path: PathBuf,
    pub media_format: TsugaruMediaFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsugaruCommandPlan {
    pub command: Option<TsugaruCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

pub fn build_tsugaru_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    profile: &TsugaruProfile,
) -> TsugaruCommandPlan {
    let mut blockers = Vec::new();
    match identity {
        CanonicalIdentityStatus::Resolved(value)
            if value.platform_id == TSUGARU_SUPPORTED_PLATFORM_ID => {}
        CanonicalIdentityStatus::Resolved(value) => blockers.push(blocker(
            LaunchBlockerKind::TsugaruPlatformMismatch,
            format!("resolved platform is {}, not FM Towns", value.platform_id),
        )),
        CanonicalIdentityStatus::Unknown => blockers.push(blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "FM Towns identity is unresolved",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(blocker(
            LaunchBlockerKind::IdentityConflict,
            "FM Towns identity evidence conflicts",
        )),
    }
    let LaunchTarget::Standalone { adapter_id, .. } = &candidate.target else {
        blockers.push(blocker(
            LaunchBlockerKind::TsugaruCandidateRequired,
            "candidate is not a standalone Tsugaru target",
        ));
        return TsugaruCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "tsugaru" {
        blockers.push(blocker(
            LaunchBlockerKind::TsugaruCandidateRequired,
            "candidate does not target Tsugaru",
        ));
    }
    if candidate.readiness == crate::launch::readiness::LaunchReadiness::Blocked
        || !candidate.blockers.is_empty()
    {
        blockers.extend(candidate.blockers.iter().cloned());
    }
    if !profile.eligible {
        blockers.push(blocker(
            LaunchBlockerKind::TsugaruProfileUnavailable,
            profile
                .blocker
                .clone()
                .unwrap_or_else(|| "Tsugaru profile is not eligible".into()),
        ));
    }
    let Some(rom_directory) = profile.rom_directory.clone() else {
        blockers.push(blocker(
            LaunchBlockerKind::TsugaruProfileUnavailable,
            "Tsugaru requires an explicit ROM directory",
        ));
        return TsugaruCommandPlan {
            command: None,
            blockers,
        };
    };
    let Some(content) = candidate
        .content
        .resolved_path
        .clone()
        .filter(|_| !candidate.content.requires_mount)
    else {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "no direct FM Towns content path is available",
        ));
        return TsugaruCommandPlan {
            command: None,
            blockers,
        };
    };
    let Some(media_format) = tsugaru_media_format(&content) else {
        blockers.push(blocker(
            LaunchBlockerKind::TsugaruContentFormatUnsupported,
            "Tsugaru V1 accepts only ISO, CUE, or MDS CD images; D88, HDI, and NHD are not inferred",
        ));
        return TsugaruCommandPlan {
            command: None,
            blockers,
        };
    };
    match candidate.firmware {
        FirmwareReadiness::Missing => blockers.push(blocker(
            LaunchBlockerKind::TsugaruFirmwareMissing,
            "Tsugaru ROM/firmware assets are missing",
        )),
        FirmwareReadiness::Unknown => blockers.push(blocker(
            LaunchBlockerKind::TsugaruFirmwareUnavailable,
            "Tsugaru ROM/firmware readiness is unknown",
        )),
        FirmwareReadiness::Verified | FirmwareReadiness::PresentUnverified => {}
        FirmwareReadiness::NotRequired => blockers.push(blocker(
            LaunchBlockerKind::TsugaruFirmwareUnavailable,
            "Tsugaru requires an explicit ROM directory",
        )),
    }
    if !blockers.is_empty() {
        return TsugaruCommandPlan {
            command: None,
            blockers,
        };
    }
    TsugaruCommandPlan {
        command: Some(TsugaruCommand {
            executable: profile.executable.path.clone(),
            arguments: vec![
                rom_directory.clone().into_os_string(),
                "-DONTAUTOSAVECMOS".into(),
                "-CD".into(),
                content.clone().into_os_string(),
            ],
            working_directory: profile.executable.path.parent().map(Path::to_path_buf),
            rom_directory,
            content_path: content,
            media_format,
        }),
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::{
        CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
        ResolvedIdentity,
    };
    use crate::launch::readiness::LaunchReadiness;
    use crate::patch_manager::{TsugaruExecutable, TsugaruFirmwareState, TsugaruInstallationType};

    fn profile() -> TsugaruProfile {
        TsugaruProfile {
            profile_id: "tsugaru:test".into(),
            installation_type: TsugaruInstallationType::Explicit,
            executable: TsugaruExecutable {
                path: "/opt/tsugaru/Tsugaru_CUI".into(),
                installation_type: TsugaruInstallationType::Explicit,
            },
            rom_directory: Some("/opt/tsugaru/roms".into()),
            firmware: TsugaruFirmwareState::PresentUnverified,
            eligible: true,
            blocker: None,
        }
    }

    fn candidate(path: &str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "tsugaru",
                profile_id: "tsugaru:test".into(),
                profile_path: Some("/opt/tsugaru/Tsugaru_CUI".into()),
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::OpticalDisc),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(path.into()),
                requires_mount: false,
                provenance: "test".into(),
            },
            firmware: FirmwareReadiness::PresentUnverified,
            blockers: Vec::new(),
            warnings: Vec::new(),
            readiness: LaunchReadiness::ReadyWithWarnings,
            preference: CandidatePreference::SoleEligible,
        }
    }

    fn id(platform: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: "towns-test".into(),
        })
    }

    #[test]
    fn documented_cd_argv_is_typed_and_disables_cmos_autosave() {
        let plan = build_tsugaru_command_plan(
            &id(TSUGARU_SUPPORTED_PLATFORM_ID),
            &candidate("/media/Example Game.cue"),
            &profile(),
        );
        let command = plan.command.unwrap();
        let expected: Vec<OsString> = [
            "/opt/tsugaru/roms",
            "-DONTAUTOSAVECMOS",
            "-CD",
            "/media/Example Game.cue",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(command.arguments, expected);
    }

    #[test]
    fn unproven_fm_towns_disk_extensions_are_refused() {
        for path in ["game.d88", "game.hdi", "game.nhd"] {
            let plan = build_tsugaru_command_plan(
                &id(TSUGARU_SUPPORTED_PLATFORM_ID),
                &candidate(path),
                &profile(),
            );
            assert!(plan.command.is_none(), "{path} must not be inferred");
            assert!(
                plan.blockers
                    .iter()
                    .any(|b| b.kind == LaunchBlockerKind::TsugaruContentFormatUnsupported)
            );
        }
    }

    #[test]
    fn wrong_identity_is_refused() {
        let plan = build_tsugaru_command_plan(&id("PC-98"), &candidate("game.iso"), &profile());
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::TsugaruPlatformMismatch)
        );
    }
}
