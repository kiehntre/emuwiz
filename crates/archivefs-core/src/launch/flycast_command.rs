//! Read-only native Flycast command planning.
//!
//! This module turns one already-authorized Flycast [`LaunchCandidate`] and
//! an already-computed [`FlycastNativeLaunchBinding`] into argv-shaped data.
//! It never re-discovers a profile, checks the live filesystem, mounts
//! content, writes a configuration file, or starts a process - the binding
//! it is handed must already have come from a fresh call to
//! [`crate::patch_manager::resolve_flycast_native_launch_binding`] (see
//! [`crate::launch::flycast_execution`] for where that happens).
//!
//! # Scope
//!
//! Only the first supported native Flycast launch slice: `Dreamcast`
//! platform, a direct regular `.iso`, `.cue`, `.gdi`, `.chd`, or `.cdi` file,
//! a verified Dreamcast product code, and an exact eligible
//! [`FlycastNativeLaunchBinding`]. Mounted/archive content and unsupported
//! multi-track CHD GD-ROM images are refused here - never silently widened.
//!
//! `.iso`, `.cue`, `.gdi`, and `.chd` are exactly the formats
//! [`crate::game_identity`]'s Dreamcast IP.BIN identity check already
//! verifies authoritatively (see `inspect_dreamcast_source`'s dispatch in
//! that module, and [`crate::ingestion::gdi`] for how `.gdi`'s own
//! high-density data track is resolved) - this slice never accepts a
//! format the identity layer could not already prove. CDI is accepted only
//! when the existing bounded DiscJuggler reader has already produced verified
//! Dreamcast IP.BIN evidence. Multi-track CHD GD-ROM remains conditional on
//! the existing optional specialist backend; the default build fails closed.
//!
//! # Argv contract
//!
//! `[flycast] [content]` - a single positional content-path argument, no
//! flags. Unlike DuckStation's Qt frontend (which needs `-batch` to exit
//! after the emulated session ends rather than returning to an open
//! game-list window - see [`crate::launch::duckstation_execution`]'s own
//! module doc comment for the exact upstream citation), Flycast's own CLI
//! opens and runs the given content directly and exits when the emulation
//! window closes, with no separate frontend to return to - the same
//! "one process, one play session" model RetroArch/Dolphin/PCSX2 already
//! use. This is standard, widely documented Flycast usage; unlike the
//! DuckStation `-batch` citation, it has not been independently re-verified
//! against upstream Flycast source in this change, and is worth confirming
//! before this path is exposed to a real user-facing Launch button.
//!
//! Every argument is carried as its own `OsString` - spaces, quotes, and
//! shell-looking characters in a path are inert data, never shell syntax.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{FlycastLaunchBlocker, FlycastNativeLaunchBinding};

/// The only platform this native launch slice supports.
pub const FLYCAST_SUPPORTED_PLATFORM_ID: &str = "Dreamcast";

/// The only direct content extensions this slice supports (lowercase, no
/// dot) - exactly the formats [`crate::game_identity`]'s Dreamcast IP.BIN
/// identity check already verifies authoritatively. Multi-track GD-ROM CHD
/// still depends on the identity layer's optional specialist backend.
const FLYCAST_SUPPORTED_EXTENSIONS: &[&str] = &["iso", "cue", "gdi", "chd", "cdi"];

/// The executable invocation data for a Flycast launch that has passed every
/// fail-closed check. This is data only: no type in this module implements
/// process spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: FlycastCommandSelection,
}

/// The facts that produced the command's argv - profile/binding, platform,
/// verified Dreamcast product code, and content path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_dreamcast_product_code: String,
    pub content_path: PathBuf,
}

/// A successful command, or the structured reasons a command was withheld.
/// `command` is `None` whenever `blockers` is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlycastCommandPlan {
    pub command: Option<FlycastCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

impl FlycastCommandPlan {
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

pub(crate) fn direct_dreamcast_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            FLYCAST_SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

/// Builds a safe Flycast argv plan from only an already-authorized launch
/// candidate, an already-computed launch binding result, and the verified
/// Dreamcast product code the caller freshly re-confirmed.
///
/// `binding` is a `Result` rather than a bare [`FlycastNativeLaunchBinding`]
/// so a caller's fresh
/// [`crate::patch_manager::resolve_flycast_native_launch_binding`] failure
/// flows straight into this plan's blockers instead of forcing the caller
/// to invent a placeholder success value.
pub fn build_flycast_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_dreamcast_product_code: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<FlycastNativeLaunchBinding, FlycastLaunchBlocker>,
) -> FlycastCommandPlan {
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
        && resolved.platform_id != FLYCAST_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::FlycastPlatformMismatch,
            format!(
                "resolved identity targets {}, but only {FLYCAST_SUPPORTED_PLATFORM_ID} is \
                 supported by this native Flycast launch slice",
                resolved.platform_id
            ),
        ));
    }
    if verified_dreamcast_product_code.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::FlycastProductCodeMissing,
            "no verified Dreamcast product code is available for this content",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::FlycastCandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return FlycastCommandPlan::blocked(blockers);
    };
    if *adapter_id != "flycast" {
        blockers.push(blocker(
            LaunchBlockerKind::FlycastCandidateRequired,
            format!("the supplied launch candidate targets adapter `{adapter_id}`, not `flycast`"),
        ));
    }

    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied Flycast launch candidate is marked blocked",
            ));
        }
    }

    if candidate.content.requires_mount {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "content requires a mount that has not been performed, so no command can be produced",
        ));
    }
    let content_path = match &candidate.content.resolved_path {
        Some(path) if !candidate.content.requires_mount => Some(path.clone()),
        _ => {
            blockers.push(blocker(
                LaunchBlockerKind::ContentNotResolved,
                "no resolved runnable game/content path is available",
            ));
            None
        }
    };
    if let Some(path) = &content_path {
        if crate::archive_kind(path).is_some_and(|kind| kind.is_mount_input()) {
            blockers.push(blocker(
                LaunchBlockerKind::FlycastContentFormatUnsupported,
                "content path is an outer archive/mount-input path, not direct content",
            ));
        } else if !direct_dreamcast_extension(path) {
            blockers.push(blocker(
                LaunchBlockerKind::FlycastContentFormatUnsupported,
                "only a direct .iso, .cue, .gdi, .chd, or .cdi file is supported by this native \
                 Flycast launch slice; unsupported multi-track CHD GD-ROM remains refused",
            ));
        }
    }

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::FlycastBindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return FlycastCommandPlan::blocked(blockers);
    }

    let resolved = resolved.expect("identity is Resolved when no blockers exist");
    let verified_dreamcast_product_code = verified_dreamcast_product_code
        .expect("a verified Dreamcast product code is required when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let binding = binding.expect("a launch binding is required when no blockers exist");

    let arguments = vec![content_path.clone().into_os_string()];

    FlycastCommandPlan {
        command: Some(FlycastCommand {
            executable: binding.executable.clone(),
            arguments,
            working_directory: None,
            selection: FlycastCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                verified_dreamcast_product_code: verified_dreamcast_product_code.to_string(),
                content_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
