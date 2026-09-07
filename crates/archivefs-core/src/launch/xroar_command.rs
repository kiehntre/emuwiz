//! Typed, read-only XRoar command planning for Dragon/CoCo CAS images.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{
    FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchReadiness,
};
use crate::patch_manager::{XRoarMachine, XRoarProfile};

pub const XROAR_SUPPORTED_TAPE_PLATFORM_IDS: &[&str] = &["Dragon / Tandy CoCo"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XRoarMediaFormat {
    Cas,
}

pub fn xroar_media_format(path: &Path) -> Option<XRoarMediaFormat> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| extension.eq_ignore_ascii_case("cas"))
        .map(|_| XRoarMediaFormat::Cas)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub profile_id: String,
    pub machine: XRoarMachine,
    pub content_path: PathBuf,
    pub media_format: XRoarMediaFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XRoarCommandPlan {
    pub command: Option<XRoarCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

pub fn build_xroar_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    profile: &XRoarProfile,
) -> XRoarCommandPlan {
    let mut blockers = Vec::new();
    match identity {
        CanonicalIdentityStatus::Resolved(value)
            if value.platform_id == profile.machine.platform_id() => {}
        CanonicalIdentityStatus::Resolved(value) => blockers.push(blocker(
            LaunchBlockerKind::XRoarPlatformMismatch,
            format!(
                "selected XRoar machine targets {}, not {}",
                profile.machine.platform_id(),
                value.platform_id
            ),
        )),
        CanonicalIdentityStatus::Unknown => blockers.push(blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "Dragon/CoCo identity is unresolved",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(blocker(
            LaunchBlockerKind::IdentityConflict,
            "Dragon/CoCo identity evidence conflicts",
        )),
    }
    let LaunchTarget::Standalone { adapter_id, .. } = &candidate.target else {
        blockers.push(blocker(
            LaunchBlockerKind::XRoarCandidateRequired,
            "candidate is not a standalone XRoar target",
        ));
        return XRoarCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "xroar" {
        blockers.push(blocker(
            LaunchBlockerKind::XRoarCandidateRequired,
            "candidate does not target XRoar",
        ));
    }
    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
    }
    if !profile.eligible {
        blockers.push(blocker(
            LaunchBlockerKind::XRoarBindingUnavailable,
            profile
                .blocker
                .clone()
                .unwrap_or_else(|| "XRoar profile is not eligible".into()),
        ));
    }
    let Some(content) = candidate
        .content
        .resolved_path
        .clone()
        .filter(|_| !candidate.content.requires_mount)
    else {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "no direct Dragon/CoCo content path is available",
        ));
        return XRoarCommandPlan {
            command: None,
            blockers,
        };
    };
    let Some(media_format) = xroar_media_format(&content) else {
        blockers.push(blocker(
            LaunchBlockerKind::XRoarContentFormatUnsupported,
            "XRoar V1 accepts only direct .cas tape images",
        ));
        return XRoarCommandPlan {
            command: None,
            blockers,
        };
    };
    match candidate.firmware {
        FirmwareReadiness::Missing => blockers.push(blocker(
            LaunchBlockerKind::XRoarFirmwareMissing,
            "required XRoar machine firmware is missing",
        )),
        FirmwareReadiness::Unknown => blockers.push(blocker(
            LaunchBlockerKind::XRoarFirmwareUnavailable,
            "XRoar machine firmware readiness is unknown",
        )),
        FirmwareReadiness::Verified | FirmwareReadiness::PresentUnverified => {}
        FirmwareReadiness::NotRequired => blockers.push(blocker(
            LaunchBlockerKind::XRoarFirmwareUnavailable,
            "XRoar requires machine-specific firmware readiness",
        )),
    }
    if !blockers.is_empty() {
        return XRoarCommandPlan {
            command: None,
            blockers,
        };
    }
    XRoarCommandPlan {
        command: Some(XRoarCommand {
            executable: profile.executable.path.clone(),
            arguments: vec![
                "-machine".into(),
                profile.machine.flag().into(),
                "-load-tape".into(),
                content.clone().into_os_string(),
            ],
            working_directory: profile.executable.path.parent().map(Path::to_path_buf),
            profile_id: profile.profile_id.clone(),
            machine: profile.machine,
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
    use crate::patch_manager::{XRoarExecutable, XRoarFirmwareState, XRoarInstallationType};

    fn profile(machine: XRoarMachine) -> XRoarProfile {
        XRoarProfile {
            profile_id: format!("xroar:test:{}", machine.flag()),
            machine,
            installation_type: XRoarInstallationType::Explicit,
            executable: XRoarExecutable {
                path: "/opt/xroar/xroar".into(),
                installation_type: XRoarInstallationType::Explicit,
            },
            firmware: XRoarFirmwareState::PresentUnverified,
            eligible: true,
            blocker: None,
        }
    }

    fn candidate(path: &str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "xroar",
                profile_id: "xroar:test:dragon32".into(),
                profile_path: Some("/opt/xroar/xroar".into()),
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::Executable),
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
            game_key: "cas-test".into(),
        })
    }

    #[test]
    fn dragon_cas_argv_is_typed() {
        let plan = build_xroar_command_plan(
            &id("Dragon / Tandy CoCo"),
            &candidate("/media/Dragon Game.cas"),
            &profile(XRoarMachine::Dragon32),
        );
        let command = plan.command.unwrap();
        let expected: Vec<OsString> = [
            "-machine",
            "dragon32",
            "-load-tape",
            "/media/Dragon Game.cas",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(command.arguments, expected);
    }

    #[test]
    fn coco_machine_can_launch_family_level_cas() {
        let plan = build_xroar_command_plan(
            &id("Dragon / Tandy CoCo"),
            &candidate("game.cas"),
            &profile(XRoarMachine::CoCo3),
        );
        assert!(plan.command.is_some());
    }

    #[test]
    fn unsupported_media_and_wrong_machine_are_refused() {
        let unsupported = build_xroar_command_plan(
            &id("Dragon / Tandy CoCo"),
            &candidate("game.dsk"),
            &profile(XRoarMachine::Dragon32),
        );
        assert!(unsupported.command.is_none());
        assert!(
            unsupported
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::XRoarContentFormatUnsupported)
        );

        let wrong_machine = build_xroar_command_plan(
            &id("ZX Spectrum"),
            &candidate("game.cas"),
            &profile(XRoarMachine::CoCo3),
        );
        assert!(wrong_machine.command.is_none());
        assert!(
            wrong_machine
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::XRoarPlatformMismatch)
        );
    }
}
