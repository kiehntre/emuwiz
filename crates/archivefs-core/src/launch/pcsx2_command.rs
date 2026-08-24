//! Read-only native PCSX2 command planning.
//!
//! This module turns one already-authorized PCSX2 [`LaunchCandidate`] and
//! an already-computed [`Pcsx2NativeLaunchBinding`] into argv-shaped data.
//! It never re-discovers a profile, checks the live filesystem, mounts
//! content, writes a configuration file, or starts a process - the binding
//! it is handed must already have come from a fresh call to
//! [`crate::patch_manager::resolve_pcsx2_native_launch_binding`] (see
//! [`crate::launch::pcsx2_execution`] for where that happens).
//!
//! # Scope
//!
//! Only the first supported native PCSX2 launch slice: `PS2` platform, a
//! direct regular `.iso` file, a verified PS2 serial, and an exact eligible
//! [`Pcsx2NativeLaunchBinding`]. CHD, mounted/archive content, Flatpak,
//! Portable/AppImage, and `NativeAlternate` installs are all refused here -
//! never silently widened.
//!
//! # Exact argv
//!
//! - [`Pcsx2UserDirectoryMode::DefaultNative`] -> `[content]`
//! - [`Pcsx2UserDirectoryMode::ExplicitDataPath(root)`] -> `["-datapath", root, content]`
//!
//! The second form is modeled for forward compatibility only: as of this
//! slice, [`crate::patch_manager::resolve_pcsx2_native_launch_binding`]
//! never actually returns [`Pcsx2UserDirectoryMode::ExplicitDataPath`] (see
//! that type's own doc comment for why), so in practice every produced
//! command uses the first form. This planner never invents the mode itself
//! - it only ever renders whatever the binding it was handed already
//! proved.
//!
//! Every argument is carried as its own `OsString` - spaces, quotes, and
//! shell-looking characters in a path are inert data, never shell syntax.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::planning::{CanonicalIdentityStatus, LaunchCandidate, LaunchTarget};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{Pcsx2LaunchBlocker, Pcsx2NativeLaunchBinding, Pcsx2UserDirectoryMode};

/// The only platform this native launch slice supports.
pub const PCSX2_SUPPORTED_PLATFORM_ID: &str = "PS2";

/// The only direct content extension this slice supports (lowercase, no
/// dot) - CHD and any archive/mount-input format are refused.
const PCSX2_SUPPORTED_EXTENSIONS: &[&str] = &["iso"];

/// The executable invocation data for a PCSX2 launch that has passed every
/// fail-closed check. This is data only: no type in this module implements
/// process spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2Command {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: Pcsx2CommandSelection,
}

/// The facts that produced the command's argv - profile/binding, platform,
/// verified PS2 serial, and content path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2CommandSelection {
    pub profile_id: String,
    pub user_directory_mode: Pcsx2UserDirectoryMode,
    pub platform_id: String,
    pub verified_ps2_serial: String,
    pub content_path: PathBuf,
}

/// A successful command, or the structured reasons a command was withheld.
/// `command` is `None` whenever `blockers` is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcsx2CommandPlan {
    pub command: Option<Pcsx2Command>,
    pub blockers: Vec<LaunchBlocker>,
}

impl Pcsx2CommandPlan {
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

pub(crate) fn direct_ps2_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            PCSX2_SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

/// Builds a safe PCSX2 argv plan from only an already-authorized launch
/// candidate, an already-computed launch binding result, and the verified
/// PS2 serial the caller freshly re-confirmed.
///
/// `binding` is a `Result` rather than a bare [`Pcsx2NativeLaunchBinding`]
/// so a caller's fresh
/// [`crate::patch_manager::resolve_pcsx2_native_launch_binding`] failure
/// (ambiguous executable, unsupported install type, portable-marker
/// conflict, etc.) flows straight into this plan's blockers instead of
/// forcing the caller to invent a placeholder success value.
///
/// `verified_ps2_serial` is required, not merely preferred: even though
/// [`crate::patch_manager::Pcsx2GameRequest`]/`project_pcsx2_launch_input`
/// can authorize on a verified executable CRC alone, this narrower launch
/// slice always requires a verified serial - see the module doc comment.
pub fn build_pcsx2_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_ps2_serial: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<Pcsx2NativeLaunchBinding, Pcsx2LaunchBlocker>,
) -> Pcsx2CommandPlan {
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
        && resolved.platform_id != PCSX2_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::Pcsx2PlatformMismatch,
            format!(
                "resolved identity targets {}, but only {PCSX2_SUPPORTED_PLATFORM_ID} is \
                 supported by this native PCSX2 launch slice",
                resolved.platform_id
            ),
        ));
    }
    if verified_ps2_serial.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::Pcsx2SerialMissing,
            "no verified PS2 serial is available for this content",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::Pcsx2CandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return Pcsx2CommandPlan::blocked(blockers);
    };
    if *adapter_id != "pcsx2" {
        blockers.push(blocker(
            LaunchBlockerKind::Pcsx2CandidateRequired,
            format!("the supplied launch candidate targets adapter `{adapter_id}`, not `pcsx2`"),
        ));
    }

    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied PCSX2 launch candidate is marked blocked",
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
                LaunchBlockerKind::Pcsx2ContentFormatUnsupported,
                "content path is an outer archive/mount-input path, not direct content",
            ));
        } else if !direct_ps2_extension(path) {
            blockers.push(blocker(
                LaunchBlockerKind::Pcsx2ContentFormatUnsupported,
                "only a direct .iso file is supported by this native PCSX2 launch slice",
            ));
        }
    }

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::Pcsx2BindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return Pcsx2CommandPlan::blocked(blockers);
    }

    let resolved = resolved.expect("identity is Resolved when no blockers exist");
    let verified_ps2_serial =
        verified_ps2_serial.expect("a verified PS2 serial is required when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let binding = binding.expect("a launch binding is required when no blockers exist");

    let mut arguments = Vec::with_capacity(3);
    if let Pcsx2UserDirectoryMode::ExplicitDataPath(root) = &binding.user_directory_mode {
        arguments.push(OsString::from("-datapath"));
        arguments.push(root.clone().into_os_string());
    }
    arguments.push(content_path.clone().into_os_string());

    Pcsx2CommandPlan {
        command: Some(Pcsx2Command {
            executable: binding.executable.clone(),
            arguments,
            working_directory: None,
            selection: Pcsx2CommandSelection {
                profile_id: profile_id.clone(),
                user_directory_mode: binding.user_directory_mode.clone(),
                platform_id: resolved.platform_id.clone(),
                verified_ps2_serial: verified_ps2_serial.to_string(),
                content_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
