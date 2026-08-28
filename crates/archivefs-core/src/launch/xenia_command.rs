//! Read-only native Xenia Canary command planning.
//!
//! This module turns one already-authorized Xenia [`LaunchCandidate`] and
//! an already-computed [`XeniaLaunchBinding`] into argv-shaped data. It
//! never re-discovers a profile, checks the live filesystem, mounts
//! content, writes a configuration file, or starts a process - the binding
//! it is handed must already have come from a fresh call to
//! [`crate::patch_manager::resolve_xenia_launch_binding`].
//!
//! # Scope
//!
//! Only the first supported native Xenia launch slice: `Xbox360` platform,
//! a direct regular `.xex` file, at least one verified XEX identity fact
//! (title ID or media ID - see [`crate::game_identity::GameIdentityReport::verified_xex_title_id`]/
//! [`crate::game_identity::GameIdentityReport::verified_xex_media_id`]), and
//! an exact eligible [`XeniaLaunchBinding`].
//!
//! A ZIP containing exactly one XEX member is already genuine, verified
//! identity (see [`crate::game_identity`]'s `inspect_zip_xex`), but is
//! refused here exactly like every other native launch slice in this crate
//! refuses archive/mount-input content - see [`crate::archive_kind`] and
//! [`LaunchContainerKind::Archive`]. Xbox 360 ISO/disc-image launch is not
//! modeled by this build at all (no XDVDFS-on-optical-media reader exists
//! for Xbox 360 discs), so it is refused as an unsupported content format
//! rather than guessed at.
//!
//! # Exact argv
//!
//! `[content]` - Xenia Canary's own documented CLI contract takes the game
//! path as its only required argument for a plain launch; no additional
//! flag is modeled here, and none is invented.
//!
//! Every argument is carried as its own `OsString` - spaces, quotes, and
//! shell-looking characters in a path are inert data, never shell syntax.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{XeniaLaunchBinding, XeniaLaunchBlocker};

/// The only platform this native launch slice supports.
pub const XENIA_SUPPORTED_PLATFORM_ID: &str = "Xbox360";

/// The only direct content extension this slice supports (lowercase, no
/// dot) - a ZIP-contained XEX and any other archive/mount-input format are
/// refused.
const XENIA_SUPPORTED_EXTENSIONS: &[&str] = &["xex"];

/// The executable invocation data for a Xenia launch that has passed every
/// fail-closed check. This is data only: no type in this module implements
/// process spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XeniaCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: XeniaCommandSelection,
}

/// The facts that produced the command's argv - profile, platform, verified
/// XEX identity, and content path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XeniaCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_xex_title_id: Option<String>,
    pub verified_xex_media_id: Option<String>,
    pub content_path: PathBuf,
}

/// A successful command, or the structured reasons a command was withheld.
/// `command` is `None` whenever `blockers` is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XeniaCommandPlan {
    pub command: Option<XeniaCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

impl XeniaCommandPlan {
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

pub(crate) fn direct_xex_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            XENIA_SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

/// Builds a safe Xenia argv plan from only an already-authorized launch
/// candidate, an already-computed launch binding result, and the verified
/// XEX title/media IDs the caller freshly re-confirmed.
///
/// `binding` is a `Result` rather than a bare [`XeniaLaunchBinding`] so a
/// caller's fresh [`crate::patch_manager::resolve_xenia_launch_binding`]
/// failure (missing/unsafe executable, stale profile, etc.) flows straight
/// into this plan's blockers instead of forcing the caller to invent a
/// placeholder success value.
///
/// At least one of `verified_xex_title_id`/`verified_xex_media_id` is
/// required - exactly the same "either is sufficient" condition
/// [`crate::launch::evidence_bridge`] already uses to resolve Xbox 360
/// canonical identity at all, so this plan never refuses a command that
/// evidence resolution itself already considered `Resolved`.
pub fn build_xenia_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_xex_title_id: Option<&str>,
    verified_xex_media_id: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<XeniaLaunchBinding, XeniaLaunchBlocker>,
) -> XeniaCommandPlan {
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
        && resolved.platform_id != XENIA_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::XeniaPlatformMismatch,
            format!(
                "resolved identity targets {}, but only {XENIA_SUPPORTED_PLATFORM_ID} is \
                 supported by this native Xenia launch slice",
                resolved.platform_id
            ),
        ));
    }
    if verified_xex_title_id.is_none() && verified_xex_media_id.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::XeniaTitleIdMissing,
            "no verified Xbox 360 XEX title ID or media ID is available for this content",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::XeniaCandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return XeniaCommandPlan::blocked(blockers);
    };
    if *adapter_id != "xenia" {
        blockers.push(blocker(
            LaunchBlockerKind::XeniaCandidateRequired,
            format!("the supplied launch candidate targets adapter `{adapter_id}`, not `xenia`"),
        ));
    }

    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied Xenia launch candidate is marked blocked",
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
                LaunchBlockerKind::XeniaContentFormatUnsupported,
                "content path is an outer archive/mount-input path, not direct content",
            ));
        } else if !direct_xex_extension(path) {
            blockers.push(blocker(
                LaunchBlockerKind::XeniaContentFormatUnsupported,
                "only a direct .xex file is supported by this native Xenia launch slice",
            ));
        }
    }

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::XeniaBindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return XeniaCommandPlan::blocked(blockers);
    }

    let resolved = resolved.expect("identity is Resolved when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let binding = binding.expect("a launch binding is required when no blockers exist");

    let arguments = vec![content_path.clone().into_os_string()];

    XeniaCommandPlan {
        command: Some(XeniaCommand {
            executable: binding.executable.clone(),
            arguments,
            working_directory: None,
            selection: XeniaCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                verified_xex_title_id: verified_xex_title_id.map(str::to_string),
                verified_xex_media_id: verified_xex_media_id.map(str::to_string),
                content_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
