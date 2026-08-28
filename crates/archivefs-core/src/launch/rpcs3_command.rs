//! Read-only native RPCS3 command planning.
//!
//! This is the narrow launch seam for an already verified PS3 identity and a
//! freshly resolved native RPCS3 binding. It does not inspect or infer
//! identity, mount media, write configuration, or spawn a process.
//!
//! RPCS3's documented native invocation is `[rpcs3] [content-path]`: the
//! executable accepts a game directory or disc image as a positional boot
//! path. EmuWiz keeps both values as separate `OsString` data and never
//! constructs a shell command.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{Rpcs3LaunchBinding, Rpcs3LaunchBlocker};

/// The canonical platform id used by the existing PS3 identity bridge.
pub const RPCS3_SUPPORTED_PLATFORM_ID: &str = "PS3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3Command {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: Rpcs3CommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3CommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_ps3_title_id: String,
    pub content_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rpcs3CommandPlan {
    pub command: Option<Rpcs3Command>,
    pub blockers: Vec<LaunchBlocker>,
}

impl Rpcs3CommandPlan {
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

/// Returns whether the already-resolved content reference has a direct PS3
/// shape that RPCS3 can boot: a `.iso` image or an extracted PS3 game folder.
/// An extensionless path is accepted only as the latter shape; identity still
/// must come from verified PS3 content and never from that path spelling.
fn direct_ps3_content_is_supported(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map_or(true, |extension| extension.eq_ignore_ascii_case("iso"))
}

/// Builds `[rpcs3] [content]` from a resolved PS3 candidate and a fresh safe
/// native binding. A CUE/BIN, archive, mounted member, or other unmodeled
/// representation is refused unless the caller has first projected it into a
/// directly runnable PS3 path supported by this contract.
pub fn build_rpcs3_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_ps3_title_id: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<Rpcs3LaunchBinding, Rpcs3LaunchBlocker>,
) -> Rpcs3CommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(resolved) => Some(resolved),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical PS3 identity could not be resolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityConflict,
                "canonical PS3 identity evidence conflicts and was not resolved",
            ));
            None
        }
    };
    if let Some(resolved) = resolved
        && resolved.platform_id != RPCS3_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::Rpcs3PlatformMismatch,
            format!(
                "resolved identity targets {}, not {RPCS3_SUPPORTED_PLATFORM_ID}",
                resolved.platform_id
            ),
        ));
    }
    let title_id = verified_ps3_title_id.filter(|value| !value.is_empty());
    if title_id.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::Rpcs3TitleIdMissing,
            "no verified PS3 TITLE_ID is available for this content",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::Rpcs3CandidateRequired,
            "the supplied launch candidate is not a standalone RPCS3 target",
        ));
        return Rpcs3CommandPlan::blocked(blockers);
    };
    if *adapter_id != "rpcs3" {
        blockers.push(blocker(
            LaunchBlockerKind::Rpcs3CandidateRequired,
            format!("the supplied candidate targets `{adapter_id}`, not `rpcs3`"),
        ));
    }
    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied RPCS3 launch candidate is blocked",
            ));
        }
    }
    if candidate.firmware == crate::launch::readiness::FirmwareReadiness::Unknown {
        blockers.push(blocker(
            LaunchBlockerKind::Rpcs3FirmwareUnavailable,
            "RPCS3 firmware readiness is unknown; the firmware location could not be verified",
        ));
    }
    if candidate.content.requires_mount {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "content requires a mount and has no directly runnable path",
        ));
    }
    let content_path = match &candidate.content.resolved_path {
        Some(path) if !candidate.content.requires_mount => Some(path.clone()),
        _ => {
            blockers.push(blocker(
                LaunchBlockerKind::ContentNotResolved,
                "no resolved runnable PS3 content path is available",
            ));
            None
        }
    };
    if let Some(path) = &content_path {
        if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input())
            || !direct_ps3_content_is_supported(path)
            || candidate.content.container
                != Some(crate::launch::planning::LaunchContainerKind::PlainFile)
        {
            blockers.push(blocker(
                LaunchBlockerKind::Rpcs3ContentFormatUnsupported,
                "only a direct PS3 ISO or an already-resolved extracted PS3 folder is supported",
            ));
        }
    }
    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::Rpcs3BindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return Rpcs3CommandPlan::blocked(blockers);
    }
    let resolved = resolved.expect("resolved identity required without blockers");
    let verified_ps3_title_id = title_id
        .expect("title ID required without blockers")
        .to_string();
    let content_path = content_path.expect("content required without blockers");
    let binding = binding.expect("binding required without blockers");
    Rpcs3CommandPlan {
        command: Some(Rpcs3Command {
            executable: binding.executable.clone(),
            arguments: vec![content_path.clone().into_os_string()],
            working_directory: None,
            selection: Rpcs3CommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                verified_ps3_title_id,
                content_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
