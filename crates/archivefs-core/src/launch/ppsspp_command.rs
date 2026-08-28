//! Read-only native PPSSPP command planning.
//!
//! PPSSPP's documented application usage is `PPSSPP [options] [FILE]`; this
//! slice uses the direct positional-file form for a verified PSP ISO.  The
//! executable and content are kept as separate `OsString` values and are
//! never interpreted by a shell.
//!
//! This planner is deliberately narrower than PPSSPP itself.  It does not
//! claim support for CSO, CHD, PBP, ZIP, mounted content, or other layouts
//! until EmuWiz can both inspect and bind those representations
//! authoritatively.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{PpssppLaunchBlocker, PpssppNativeLaunchBinding};

pub const PPSSPP_SUPPORTED_PLATFORM_ID: &str = "PSP";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: PpssppCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_psp_disc_id: String,
    pub content_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpssppCommandPlan {
    pub command: Option<PpssppCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

impl PpssppCommandPlan {
    fn blocked(blockers: Vec<LaunchBlocker>) -> Self {
        debug_assert!(!blockers.is_empty());
        Self {
            command: None,
            blockers,
        }
    }
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

pub(crate) fn direct_psp_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("iso"))
}

/// Builds a native PPSSPP command from an already resolved identity,
/// planner candidate, freshly projected verified disc ID, and freshly
/// revalidated executable binding.
pub fn build_ppsspp_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_psp_disc_id: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<PpssppNativeLaunchBinding, PpssppLaunchBlocker>,
) -> PpssppCommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(resolved) => Some(resolved),
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
                "canonical game identity evidence conflicts and was not resolved to one answer",
            ));
            None
        }
    };
    if let Some(resolved) = resolved
        && resolved.platform_id != PPSSPP_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::PpssppPlatformMismatch,
            format!(
                "resolved identity targets {}, but only {PPSSPP_SUPPORTED_PLATFORM_ID} is supported",
                resolved.platform_id
            ),
        ));
    }
    if verified_psp_disc_id.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::PpssppDiscIdMissing,
            "no verified PSP disc ID is available for this content",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::PpssppCandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return PpssppCommandPlan::blocked(blockers);
    };
    if *adapter_id != "ppsspp" {
        blockers.push(blocker(
            LaunchBlockerKind::PpssppCandidateRequired,
            format!("the supplied launch candidate targets adapter `{adapter_id}`, not `ppsspp`"),
        ));
    }
    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied PPSSPP launch candidate is marked blocked",
            ));
        }
    }

    if candidate.content.requires_mount {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "content requires a mount that has not been performed",
        ));
    }
    let content_path = match (
        &candidate.content.resolved_path,
        candidate.content.container,
    ) {
        (Some(path), Some(crate::launch::planning::LaunchContainerKind::PlainFile))
            if !candidate.content.requires_mount
                && candidate.content.kind
                    == Some(crate::launch::planning::LaunchContentKind::OpticalDisc)
                && direct_psp_extension(path) =>
        {
            Some(path.clone())
        }
        (Some(_), _) => {
            blockers.push(blocker(
                LaunchBlockerKind::PpssppContentFormatUnsupported,
                "only a direct PSP .iso file is supported; archives, mounts, and other PSP layouts are refused",
            ));
            None
        }
        (None, _) => {
            blockers.push(blocker(
                LaunchBlockerKind::ContentNotResolved,
                "no resolved runnable PSP content path is available",
            ));
            None
        }
    };

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::PpssppBindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return PpssppCommandPlan::blocked(blockers);
    }
    let resolved = resolved.expect("resolved identity when no blockers exist");
    let verified_psp_disc_id = verified_psp_disc_id.expect("verified PSP ID when unblocked");
    let content_path = content_path.expect("resolved content when unblocked");
    let binding = binding.expect("binding when unblocked");
    PpssppCommandPlan {
        command: Some(PpssppCommand {
            executable: binding.executable.clone(),
            arguments: vec![content_path.clone().into_os_string()],
            working_directory: None,
            selection: PpssppCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                verified_psp_disc_id: verified_psp_disc_id.to_string(),
                content_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
