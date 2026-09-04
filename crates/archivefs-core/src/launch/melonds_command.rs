//! Read-only native melonDS command planning.
//!
//! Phase 1 is Nintendo DS only. The adapter consumes resolved canonical
//! identity and an independently verified opaque game key; `.nds` is a format
//! gate, not identity evidence. The native command is one positional path.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{MelonDsLaunchBlocker, MelonDsNativeLaunchBinding};

pub const MELONDS_SUPPORTED_PLATFORM_ID: &str = "Nintendo DS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: MelonDsCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_game_key: String,
    pub content_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsCommandPlan {
    pub command: Option<MelonDsCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

pub(crate) fn direct_melonds_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("nds"))
}

pub fn build_melonds_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_game_key: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<MelonDsNativeLaunchBinding, MelonDsLaunchBlocker>,
) -> MelonDsCommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => Some(r),
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
    if let Some(r) = resolved
        && r.platform_id != MELONDS_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::MelonDsPlatformMismatch,
            format!("resolved platform is {}, not Nintendo DS", r.platform_id),
        ));
    }
    if verified_game_key.is_none_or(str::is_empty) {
        blockers.push(blocker(
            LaunchBlockerKind::MelonDsGameKeyMissing,
            "no independently verified Nintendo DS game identity key is available",
        ));
    }
    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::MelonDsCandidateRequired,
            "candidate is not a standalone launch target",
        ));
        return MelonDsCommandPlan {
            command: None,
            blockers,
        };
    };
    if *adapter_id != "melonds" {
        blockers.push(blocker(
            LaunchBlockerKind::MelonDsCandidateRequired,
            format!("candidate targets `{adapter_id}`, not `melonds`"),
        ));
    }
    if candidate.readiness == crate::launch::readiness::LaunchReadiness::Blocked {
        blockers.extend(candidate.blockers.iter().cloned());
    }
    let content = candidate
        .content
        .resolved_path
        .as_ref()
        .filter(|_| !candidate.content.requires_mount);
    let Some(content) = content else {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "no direct runnable content path is available",
        ));
        return MelonDsCommandPlan {
            command: None,
            blockers,
        };
    };
    if crate::archive_kind(content).is_some_and(|k| k.is_mount_input())
        || !direct_melonds_extension(content)
    {
        blockers.push(blocker(
            LaunchBlockerKind::MelonDsContentFormatUnsupported,
            "only a direct .nds file is supported by native melonDS Phase 1",
        ));
    }
    let binding = match binding {
        Ok(b) => Some(b),
        Err(e) => {
            blockers.push(blocker(
                LaunchBlockerKind::MelonDsBindingUnavailable,
                format!("{:?}: {}", e.kind, e.detail),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return MelonDsCommandPlan {
            command: None,
            blockers,
        };
    }
    let r = resolved.expect("resolved when unblocked");
    let key = verified_game_key.expect("key when unblocked");
    let b = binding.expect("binding when unblocked");
    MelonDsCommandPlan {
        command: Some(MelonDsCommand {
            executable: b.executable.clone(),
            arguments: vec![content.clone().into_os_string()],
            working_directory: None,
            selection: MelonDsCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: r.platform_id.clone(),
                verified_game_key: key.to_string(),
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
    use crate::patch_manager::{MelonDsLaunchBlocker, MelonDsLaunchBlockerKind};

    fn identity() -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Nintendo DS".into(),
            game_key: "DS-TEST".into(),
        })
    }
    fn candidate(path: &str) -> LaunchCandidate {
        LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "melonds",
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
    fn binding() -> Result<MelonDsNativeLaunchBinding, MelonDsLaunchBlocker> {
        Ok(MelonDsNativeLaunchBinding {
            executable: "/usr/bin/melonDS".into(),
        })
    }

    #[test]
    fn melonds_command_is_exact_positional_argv() {
        let plan = build_melonds_command_plan(
            &identity(),
            Some("DS-TEST"),
            &candidate("/games/DS Game.nds"),
            &binding(),
        );
        let command = plan.command.unwrap();
        assert_eq!(
            command.arguments,
            vec![OsString::from("/games/DS Game.nds")]
        );
        assert_eq!(command.selection.platform_id, "Nintendo DS");
        assert!(plan.blockers.is_empty());
    }

    #[test]
    fn melonds_rejects_dsi_and_other_platforms() {
        let dsi = build_melonds_command_plan(
            &identity(),
            Some("DS-TEST"),
            &candidate("/games/game.dsi"),
            &binding(),
        );
        assert!(
            dsi.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::MelonDsContentFormatUnsupported)
        );
        let other = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Nintendo 3DS".into(),
            game_key: "3DS".into(),
        });
        let plan = build_melonds_command_plan(
            &other,
            Some("DS-TEST"),
            &candidate("/games/game.nds"),
            &binding(),
        );
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::MelonDsPlatformMismatch)
        );
    }

    #[test]
    fn melonds_never_accepts_missing_identity() {
        let plan = build_melonds_command_plan(
            &identity(),
            None,
            &candidate("/games/game.nds"),
            &binding(),
        );
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::MelonDsGameKeyMissing)
        );
        let _ = MelonDsLaunchBlockerKind::ExecutableMissing;
    }
}
