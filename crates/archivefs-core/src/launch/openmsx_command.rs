//! Read-only openMSX cartridge command planning.
//!
//! Only canonical MSX `.mx1` and MSX2 `.mx2` cartridges are accepted. The
//! machine profile is explicit and deterministic; generic `.rom`, disk, and
//! cassette inputs are refused.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{FirmwareReadiness, LaunchBlocker, LaunchBlockerKind};

pub const OPENMSX_SUPPORTED_PLATFORM_IDS: &[&str] = &["MSX", "MSX2"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMsxCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub machine: &'static str,
    pub content_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMsxCommandPlan {
    pub command: Option<OpenMsxCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(expected))
}

pub fn build_openmsx_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
) -> OpenMsxCommandPlan {
    let mut blockers = Vec::new();
    let platform = match identity {
        CanonicalIdentityStatus::Resolved(identity) => identity.platform_id.as_str(),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical game identity could not be resolved",
            ));
            ""
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::IdentityConflict,
                "canonical game identity evidence conflicts",
            ));
            ""
        }
    };
    let expected_ext = match platform {
        "MSX" => Some(("mx1", "C-BIOS_MSX1")),
        "MSX2" => Some(("mx2", "C-BIOS_MSX2")),
        _ => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::OpenMsxPlatformMismatch,
                "openMSX V1 supports only MSX and MSX2",
            ));
            None
        }
    };
    let LaunchTarget::Standalone {
        adapter_id,
        profile_path,
        ..
    } = &candidate.target
    else {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::OpenMsxCandidateRequired,
            "the supplied candidate is not a standalone openMSX adapter",
        ));
        return OpenMsxCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "openmsx" {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::OpenMsxCandidateRequired,
            "the supplied candidate targets another adapter",
        ));
    }
    if candidate.firmware != FirmwareReadiness::NotRequired {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::OpenMsxBindingUnavailable,
            "openMSX cartridge profiles use explicit bundled C-BIOS machine bindings",
        ));
    }
    if candidate.readiness == crate::launch::readiness::LaunchReadiness::Blocked
        || !candidate.blockers.is_empty()
    {
        blockers.extend(candidate.blockers.iter().cloned());
    }
    let content = candidate
        .content
        .resolved_path
        .clone()
        .filter(|_| !candidate.content.requires_mount);
    let Some(content) = content else {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::ContentNotResolved,
            "no resolved runnable cartridge path is available",
        ));
        return OpenMsxCommandPlan {
            command: None,
            blockers,
        };
    };
    if let Some((expected_ext, _)) = expected_ext
        && !extension(&content, expected_ext)
    {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::OpenMsxContentFormatUnsupported,
            format!("only .{expected_ext} is accepted for {platform}"),
        ));
    }
    let Some(profile_path) = profile_path.clone() else {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::OpenMsxBindingUnavailable,
            "no verified openMSX executable binding is available",
        ));
        return OpenMsxCommandPlan {
            command: None,
            blockers,
        };
    };
    let Some((_, machine)) = expected_ext else {
        return OpenMsxCommandPlan {
            command: None,
            blockers,
        };
    };
    if !blockers.is_empty() {
        return OpenMsxCommandPlan {
            command: None,
            blockers,
        };
    }
    OpenMsxCommandPlan {
        command: Some(OpenMsxCommand {
            executable: profile_path,
            arguments: vec![
                "-machine".into(),
                machine.into(),
                "-carta".into(),
                content.clone().into_os_string(),
            ],
            machine,
            content_path: content,
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
    fn candidate(path: &'static str, profile: &'static str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "openmsx",
                profile_id: "openmsx:test".into(),
                profile_path: Some(profile.into()),
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::Cartridge),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(path.into()),
                requires_mount: false,
                provenance: "test".into(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: vec![],
            warnings: vec![],
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::SoleEligible,
        }
    }
    fn id(platform: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: "key".into(),
        })
    }
    #[test]
    fn msx_argv_is_machine_bound() {
        let c =
            build_openmsx_command_plan(&id("MSX"), &candidate("/rom/game.mx1", "/usr/bin/openmsx"))
                .command
                .unwrap();
        let expected: Vec<OsString> = ["-machine", "C-BIOS_MSX1", "-carta", "/rom/game.mx1"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert_eq!(c.arguments, expected);
    }
    #[test]
    fn msx2_argv_is_machine_bound() {
        let c = build_openmsx_command_plan(
            &id("MSX2"),
            &candidate("/rom/game.mx2", "/usr/bin/openmsx"),
        )
        .command
        .unwrap();
        let expected: Vec<OsString> = ["-machine", "C-BIOS_MSX2", "-carta", "/rom/game.mx2"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert_eq!(c.arguments, expected);
    }
    #[test]
    fn generic_rom_and_mismatched_extensions_refuse() {
        for (platform, path) in [
            ("MSX", "/rom/game.rom"),
            ("MSX2", "/rom/game.mx1"),
            ("MSX", "/rom/game.dsk"),
            ("MSX2", "/rom/game.cas"),
        ] {
            assert!(
                build_openmsx_command_plan(&id(platform), &candidate(path, "/usr/bin/openmsx"))
                    .command
                    .is_none()
            );
        }
    }
}
