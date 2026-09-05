//! Exact Mesen 2 command planning.  Upstream's `CommandLineHelper` loads a
//! bare existing path; `--doNotSaveSettings` is documented there and prevents
//! configuration serialization.  No other switch is required or added.
use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{MesenLaunchBlocker, MesenNativeLaunchBinding};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const MESEN_SUPPORTED_PLATFORM_IDS: &[&str] = &[
    "NES",
    "SNES",
    "Game Boy",
    "Game Boy Color",
    "Game Boy Advance",
    "PC Engine",
    "WonderSwan",
    "WonderSwan Color",
];
pub const MESEN_DO_NOT_SAVE_SETTINGS: &str = "--doNotSaveSettings";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: MesenCommandSelection,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_game_key: String,
    pub content_path: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenCommandPlan {
    pub command: Option<MesenCommand>,
    pub blockers: Vec<LaunchBlocker>,
}
fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}
pub(crate) fn direct_mesen_extension(path: &Path, platform: &str) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match platform {
        "NES" => matches!(ext.to_ascii_lowercase().as_str(), "nes" | "fds" | "unf"),
        "SNES" => matches!(ext.to_ascii_lowercase().as_str(), "sfc" | "smc"),
        "Game Boy" => ext.eq_ignore_ascii_case("gb"),
        "Game Boy Color" => ext.eq_ignore_ascii_case("gbc"),
        "Game Boy Advance" => ext.eq_ignore_ascii_case("gba"),
        "PC Engine" => matches!(ext.to_ascii_lowercase().as_str(), "pce" | "sgx"),
        "WonderSwan" => ext.eq_ignore_ascii_case("ws"),
        "WonderSwan Color" => ext.eq_ignore_ascii_case("wsc"),
        _ => false,
    }
}
pub fn build_mesen_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<MesenNativeLaunchBinding, MesenLaunchBlocker>,
) -> MesenCommandPlan {
    let mut blockers = vec![];
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(x) => Some(x),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(block(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical game identity could not be resolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(block(
                LaunchBlockerKind::IdentityConflict,
                "canonical game identity conflicts",
            ));
            None
        }
    };
    if let Some(x) = resolved {
        if !MESEN_SUPPORTED_PLATFORM_IDS.contains(&x.platform_id.as_str()) {
            blockers.push(block(
                LaunchBlockerKind::MesenPlatformMismatch,
                "resolved platform is not supported by Mesen 2",
            ));
        }
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(block(
            LaunchBlockerKind::MesenCandidateRequired,
            "candidate is not a standalone Mesen target",
        ));
        return MesenCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "mesen" {
        blockers.push(block(
            LaunchBlockerKind::MesenCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `mesen`"),
        ));
    }
    if candidate.readiness == crate::launch::readiness::LaunchReadiness::Blocked {
        blockers.extend(candidate.blockers.iter().cloned());
    }
    let Some(content) = candidate
        .content
        .resolved_path
        .as_ref()
        .filter(|_| !candidate.content.requires_mount)
    else {
        blockers.push(block(
            LaunchBlockerKind::ContentNotResolved,
            "no direct runnable content path is available",
        ));
        return MesenCommandPlan {
            command: None,
            blockers,
        };
    };
    let platform = resolved.map(|x| x.platform_id.as_str()).unwrap_or("");
    if crate::archive_kind(content).is_some_and(|k| k.is_mount_input())
        || !direct_mesen_extension(content, platform)
    {
        blockers.push(block(
            LaunchBlockerKind::MesenContentFormatUnsupported,
            "Mesen accepts only direct canonical cartridge content for the resolved platform",
        ));
    }
    let binding = match binding {
        Ok(x) => Some(x),
        Err(e) => {
            blockers.push(block(
                LaunchBlockerKind::MesenBindingUnavailable,
                format!("{:?}: {}", e.kind, e.detail),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return MesenCommandPlan {
            command: None,
            blockers,
        };
    }
    let r = resolved.unwrap();
    let b = binding.unwrap();
    MesenCommandPlan {
        command: Some(MesenCommand {
            executable: b.executable.clone(),
            arguments: vec![
                OsString::from(MESEN_DO_NOT_SAVE_SETTINGS),
                content.clone().into_os_string(),
            ],
            working_directory: None,
            selection: MesenCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: r.platform_id.clone(),
                verified_game_key: r.game_key.clone(),
                content_path: content.clone(),
            },
        }),
        blockers: vec![],
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::{
        CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
        ResolvedIdentity,
    };
    use crate::launch::readiness::{FirmwareReadiness, LaunchReadiness};
    fn c(path: &str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "mesen",
                profile_id: "p".into(),
                profile_path: None,
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
    fn id(p: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: p.into(),
            game_key: "key".into(),
        })
    }
    fn b() -> Result<MesenNativeLaunchBinding, MesenLaunchBlocker> {
        Ok(MesenNativeLaunchBinding {
            executable: "/opt/Mesen".into(),
        })
    }
    #[test]
    fn all_supported_platforms_have_exact_no_shell_argv() {
        for (p, f) in [
            ("NES", "a.nes"),
            ("SNES", "a.sfc"),
            ("Game Boy", "a.gb"),
            ("Game Boy Color", "a.gbc"),
            ("Game Boy Advance", "a.gba"),
            ("PC Engine", "a.pce"),
            ("WonderSwan", "a.ws"),
            ("WonderSwan Color", "a.wsc"),
        ] {
            let x = build_mesen_command_plan(&id(p), &c(&format!("/roms/{f}")), &b());
            let cmd = x.command.unwrap();
            assert_eq!(
                cmd.arguments,
                vec![
                    OsString::from("--doNotSaveSettings"),
                    OsString::from(format!("/roms/{f}"))
                ]
            );
            assert_ne!(cmd.executable, PathBuf::from("sh"));
        }
    }
    #[test]
    fn unsupported_platform_content_and_fallback_are_refused() {
        assert!(
            build_mesen_command_plan(&id("N64"), &c("/x.n64"), &b())
                .command
                .is_none()
        );
        assert!(
            build_mesen_command_plan(&id("NES"), &c("/x.zip"), &b())
                .command
                .is_none()
        );
        let wrong = LaunchCandidate {
            target: LaunchTarget::RetroArch {
                core_path: "/x".into(),
                core_stem: "x".into(),
            },
            ..c("/x.nes")
        };
        assert!(
            build_mesen_command_plan(&id("NES"), &wrong, &b())
                .command
                .is_none()
        );
    }
}
