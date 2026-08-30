//! Read-only native Dolphin command planning.
//!
//! This module turns one already-authorized Dolphin [`LaunchCandidate`] and
//! an already-computed [`DolphinNativeLaunchBinding`] into argv-shaped data.
//! It never re-discovers a profile, checks the live filesystem, mounts
//! content, writes a configuration file, or starts a process - the binding
//! it is handed must already have come from a fresh call to
//! [`crate::patch_manager::resolve_dolphin_native_launch_binding`] (see
//! [`crate::launch::dolphin_execution`] for where that happens).
//!
//! # Scope
//!
//! Native Dolphin launch accepts direct regular GameCube/Wii disc images,
//! a verified Dolphin GameID, and an exact eligible
//! [`DolphinNativeLaunchBinding`]. Mounted/archive content, Flatpak, and
//! AppImage remain refused here - never silently widened.
//!
//! # Exact argv
//!
//! - [`DolphinUserDirectoryMode::DefaultNative`] -> `["-e", content]`
//! - [`DolphinUserDirectoryMode::ExplicitRoot(root)`] -> `["-u", root, "-e", content]`
//!
//! Every argument is carried as its own `OsString` - spaces, quotes, and
//! shell-looking characters in a path are inert data, never shell syntax.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::patch_manager::{
    DolphinLaunchBlocker, DolphinNativeLaunchBinding, DolphinUserDirectoryMode,
};

/// The historical primary platform identifier retained for compatibility.
pub const DOLPHIN_SUPPORTED_PLATFORM_ID: &str = "GameCube";
pub const DOLPHIN_SUPPORTED_PLATFORM_IDS: &[&str] = &["GameCube", "Wii"];

/// Direct Dolphin disc-image extensions (lowercase, no dot). Identity remains
/// authoritative; an extension alone never makes a candidate launchable.
const DOLPHIN_SUPPORTED_EXTENSIONS: &[&str] = &["iso", "gcm", "rvz", "ciso", "wbfs"];

/// The executable invocation data for a Dolphin launch that has passed
/// every fail-closed check. This is data only: no type in this module
/// implements process spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: DolphinCommandSelection,
}

/// The facts that produced the command's argv - profile/binding, platform,
/// verified GameID, and content path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinCommandSelection {
    pub profile_id: String,
    pub user_directory_mode: DolphinUserDirectoryMode,
    pub platform_id: String,
    pub game_id: String,
    pub content_path: PathBuf,
}

/// A successful command, or the structured reasons a command was withheld.
/// `command` is `None` whenever `blockers` is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinCommandPlan {
    pub command: Option<DolphinCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

impl DolphinCommandPlan {
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

pub(crate) fn direct_dolphin_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            DOLPHIN_SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

pub(crate) fn dolphin_supported_platform(platform_id: &str) -> bool {
    DOLPHIN_SUPPORTED_PLATFORM_IDS.contains(&platform_id)
}

/// Builds a safe Dolphin argv plan from only an already-authorized launch
/// candidate and an already-computed launch binding result.
///
/// `binding` is a `Result` rather than a bare [`DolphinNativeLaunchBinding`]
/// so a caller's fresh
/// [`crate::patch_manager::resolve_dolphin_native_launch_binding`] failure
/// (ambiguous executable, unsafe/unsupported install type, unsafe explicit
/// root, etc.) flows straight into this plan's blockers instead of forcing
/// the caller to invent a placeholder success value.
pub fn build_dolphin_command_plan(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    binding: &Result<DolphinNativeLaunchBinding, DolphinLaunchBlocker>,
) -> DolphinCommandPlan {
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
        && !dolphin_supported_platform(&resolved.platform_id)
    {
        blockers.push(blocker(
            LaunchBlockerKind::DolphinPlatformMismatch,
            format!(
                "resolved identity targets {}, but only GameCube or Wii is supported by this \
                 native Dolphin launch path",
                resolved.platform_id
            ),
        ));
    }
    if let Some(resolved) = resolved
        && resolved.game_key.is_empty()
    {
        blockers.push(blocker(
            LaunchBlockerKind::DolphinGameIdMissing,
            "a verified Dolphin Game ID is required for native Dolphin launch",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::DolphinCandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return DolphinCommandPlan::blocked(blockers);
    };
    if *adapter_id != "dolphin" {
        blockers.push(blocker(
            LaunchBlockerKind::DolphinCandidateRequired,
            format!("the supplied launch candidate targets adapter `{adapter_id}`, not `dolphin`"),
        ));
    }

    if candidate.readiness == crate::launch::readiness::LaunchReadiness::Blocked
        || !candidate.blockers.is_empty()
    {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied Dolphin launch candidate is marked blocked",
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
                LaunchBlockerKind::DolphinContentFormatUnsupported,
                "content path is an outer archive/mount-input path, not direct content",
            ));
        } else if !direct_dolphin_extension(path) {
            blockers.push(blocker(
                LaunchBlockerKind::DolphinContentFormatUnsupported,
                "only a direct .iso, .gcm, .rvz, .ciso, or .wbfs file is supported by this native Dolphin launch path",
            ));
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wbfs"))
            && resolved.is_some_and(|identity| identity.platform_id != "Wii")
        {
            blockers.push(blocker(
                LaunchBlockerKind::DolphinContentFormatUnsupported,
                "WBFS content is supported only for a verified Wii identity",
            ));
        }
    }

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::DolphinBindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return DolphinCommandPlan::blocked(blockers);
    }

    let resolved = resolved.expect("identity is Resolved when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let binding = binding.expect("a launch binding is required when no blockers exist");

    let mut arguments = Vec::with_capacity(4);
    if let DolphinUserDirectoryMode::ExplicitRoot(root) = &binding.user_directory_mode {
        arguments.push(OsString::from("-u"));
        arguments.push(root.clone().into_os_string());
    }
    arguments.push(OsString::from("-e"));
    arguments.push(content_path.clone().into_os_string());

    DolphinCommandPlan {
        command: Some(DolphinCommand {
            executable: binding.executable.clone(),
            arguments,
            working_directory: None,
            selection: DolphinCommandSelection {
                profile_id: profile_id.clone(),
                user_directory_mode: binding.user_directory_mode.clone(),
                platform_id: resolved.platform_id.clone(),
                game_id: resolved.game_key.clone(),
                content_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
