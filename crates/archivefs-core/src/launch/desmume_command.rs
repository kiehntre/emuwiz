//! Exact, no-shell native DeSmuME command planning.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{DesmumeLaunchBlocker, DesmumeNativeLaunchBinding};

pub const DESMUME_SUPPORTED_PLATFORM_ID: &str = "Nintendo DS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: DesmumeCommandSelection,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub game_key: String,
    pub content_path: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesmumeCommandPlan {
    pub command: Option<DesmumeCommand>,
    pub blockers: Vec<LaunchBlocker>,
}
fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}
pub(crate) fn direct_desmume_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("nds"))
}

pub fn build_desmume_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<DesmumeNativeLaunchBinding, DesmumeLaunchBlocker>,
) -> DesmumeCommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(identity) => Some(identity),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical game identity could not be resolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityConflict,
                "canonical game identity is conflicting",
            ));
            None
        }
    };
    if resolved.is_some_and(|identity| identity.platform_id != DESMUME_SUPPORTED_PLATFORM_ID) {
        blockers.push(blocker(
            LaunchBlockerKind::DesmumePlatformMismatch,
            "DeSmuME accepts only canonical Nintendo DS content",
        ));
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        return DesmumeCommandPlan {
            command: None,
            blockers: vec![blocker(
                LaunchBlockerKind::DesmumeCandidateRequired,
                "candidate is not a standalone DeSmuME target",
            )],
        };
    };
    if *adapter_id != "desmume" {
        blockers.push(blocker(
            LaunchBlockerKind::DesmumeCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `desmume`"),
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
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "no direct runnable content path is available",
        ));
        return DesmumeCommandPlan {
            command: None,
            blockers,
        };
    };
    if crate::archive_kind(content).is_some_and(|kind| kind.is_mount_input())
        || !direct_desmume_extension(content)
    {
        blockers.push(blocker(
            LaunchBlockerKind::DesmumeContentFormatUnsupported,
            "DeSmuME V1 accepts only a direct .nds file",
        ));
    }
    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::DesmumeBindingUnavailable,
                error.detail.clone(),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return DesmumeCommandPlan {
            command: None,
            blockers,
        };
    }
    let identity = resolved.expect("resolved when unblocked");
    let binding = binding.expect("binding when unblocked");
    DesmumeCommandPlan {
        command: Some(DesmumeCommand {
            executable: binding.executable.clone(),
            arguments: vec![content.clone().into_os_string()],
            working_directory: None,
            selection: DesmumeCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: identity.platform_id.clone(),
                game_key: identity.game_key.clone(),
                content_path: content.clone(),
            },
        }),
        blockers: Vec::new(),
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
    fn identity(platform: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: "DS-TEST".into(),
        })
    }
    fn candidate(path: &str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "desmume",
                profile_id: "desmume:native:/usr/bin/desmume".into(),
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
    fn binding() -> Result<DesmumeNativeLaunchBinding, DesmumeLaunchBlocker> {
        Ok(DesmumeNativeLaunchBinding {
            executable: "/usr/bin/desmume".into(),
        })
    }
    #[test]
    fn exact_nds_argv_is_never_shell_wrapped() {
        let plan = build_desmume_command_plan(
            &identity("Nintendo DS"),
            &candidate("/games/DS Game.nds"),
            &binding(),
        );
        let command = plan.command.unwrap();
        assert_eq!(
            command.arguments,
            vec![OsString::from("/games/DS Game.nds")]
        );
        assert_ne!(command.executable, PathBuf::from("sh"));
        assert_eq!(command.working_directory, None);
    }
    #[test]
    fn wrong_platform_and_non_nds_content_are_refused() {
        let wrong = build_desmume_command_plan(
            &identity("Nintendo 3DS"),
            &candidate("/games/game.nds"),
            &binding(),
        );
        assert!(wrong.command.is_none());
        assert!(
            wrong
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::DesmumePlatformMismatch)
        );
        let format = build_desmume_command_plan(
            &identity("Nintendo DS"),
            &candidate("/games/game.zip"),
            &binding(),
        );
        assert!(
            format
                .blockers
                .iter()
                .any(|blocker| blocker.kind == LaunchBlockerKind::DesmumeContentFormatUnsupported)
        );
    }
}
