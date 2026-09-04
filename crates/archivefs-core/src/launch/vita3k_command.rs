//! Read-only Vita3K command planning for already-installed Vita titles.
//!
//! Vita3K's `--installed-path` accepts the installed app directory name.  The
//! planner therefore emits `--installed-path <TITLE_ID>` and never passes a
//! VPK/PKG or an arbitrary folder to the emulator.  Installation remains out
//! of scope.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{Vita3kLaunchBlocker, Vita3kNativeLaunchBinding};

pub const VITA3K_SUPPORTED_PLATFORM_ID: &str = "PlayStation Vita";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: Vita3kCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub title_id: String,
    pub installed_title_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kCommandPlan {
    pub command: Option<Vita3kCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

pub fn build_vita3k_command_plan(
    identity: &CanonicalIdentityStatus,
    title_id: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<Vita3kNativeLaunchBinding, Vita3kLaunchBlocker>,
) -> Vita3kCommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(identity) => Some(identity),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical Vita identity could not be resolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityConflict,
                "canonical Vita identity conflicts",
            ));
            None
        }
    };
    if let Some(identity) = resolved
        && identity.platform_id != VITA3K_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::Vita3kPlatformMismatch,
            "resolved identity is not PlayStation Vita",
        ));
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::Vita3kCandidateRequired,
            "candidate is not a standalone Vita3K target",
        ));
        return Vita3kCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "vita3k" {
        blockers.push(blocker(
            LaunchBlockerKind::Vita3kCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `vita3k`"),
        ));
    }
    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "Vita3K candidate is blocked",
            ));
        }
    }
    let Some(title_id) = title_id.filter(|value| !value.is_empty()) else {
        blockers.push(blocker(
            LaunchBlockerKind::Vita3kTitleIdMissing,
            "no trusted Vita title ID is available",
        ));
        return Vita3kCommandPlan {
            command: None,
            blockers,
        };
    };
    let Some(installed_path) = candidate.content.resolved_path.clone() else {
        blockers.push(blocker(
            LaunchBlockerKind::Vita3kInstalledTitleMissing,
            "the installed Vita title path is unavailable",
        ));
        return Vita3kCommandPlan {
            command: None,
            blockers,
        };
    };
    if !installed_path.is_absolute() {
        blockers.push(blocker(
            LaunchBlockerKind::Vita3kContentUnsupported,
            "installed Vita title path must be absolute",
        ));
    }
    let Some(binding) = binding.as_ref().ok() else {
        let error = binding.as_ref().unwrap_err();
        blockers.push(blocker(
            LaunchBlockerKind::Vita3kBindingUnavailable,
            format!("{:?}: {}", error.kind, error.detail),
        ));
        return Vita3kCommandPlan {
            command: None,
            blockers,
        };
    };
    if !blockers.is_empty() {
        return Vita3kCommandPlan {
            command: None,
            blockers,
        };
    }
    let resolved = resolved.expect("identity was checked above");
    Vita3kCommandPlan {
        command: Some(Vita3kCommand {
            executable: binding.executable.clone(),
            arguments: vec!["--installed-path".into(), title_id.into()],
            working_directory: None,
            selection: Vita3kCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                title_id: title_id.to_string(),
                installed_title_path: installed_path,
            },
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
    use crate::launch::readiness::FirmwareReadiness;

    fn identity(platform: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: "key".into(),
        })
    }
    fn candidate() -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "vita3k",
                profile_id: "profile".into(),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: Some(LaunchContentKind::Executable),
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some("/data/Vita3K/ux0/app/PCSF00001".into()),
                requires_mount: false,
                provenance: "test".into(),
            },
            firmware: FirmwareReadiness::PresentUnverified,
            blockers: vec![],
            warnings: vec![],
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::SoleEligible,
        }
    }
    fn binding() -> Result<Vita3kNativeLaunchBinding, Vita3kLaunchBlocker> {
        Ok(Vita3kNativeLaunchBinding {
            executable: "/opt/Vita3K".into(),
            profile_id: "profile".into(),
        })
    }

    #[test]
    fn installed_title_plan_is_exact_and_shell_free() {
        let plan = build_vita3k_command_plan(
            &identity(VITA3K_SUPPORTED_PLATFORM_ID),
            Some("PCSF00001"),
            &candidate(),
            &binding(),
        );
        let command = plan.command.unwrap();
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("--installed-path"),
                OsString::from("PCSF00001")
            ]
        );
    }

    #[test]
    fn psp_conflict_and_missing_title_id_fail_closed() {
        assert!(
            build_vita3k_command_plan(
                &identity("PSP"),
                Some("PCSF00001"),
                &candidate(),
                &binding()
            )
            .command
            .is_none()
        );
        assert!(
            build_vita3k_command_plan(
                &identity(VITA3K_SUPPORTED_PLATFORM_ID),
                None,
                &candidate(),
                &binding()
            )
            .command
            .is_none()
        );
    }
}
