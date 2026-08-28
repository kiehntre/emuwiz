//! Read-only native DuckStation command planning.
//!
//! This module turns one already-authorized DuckStation [`LaunchCandidate`]
//! and an already-computed [`DuckStationNativeLaunchBinding`] into
//! argv-shaped data. It never re-discovers a profile, checks the live
//! filesystem, mounts content, writes a configuration file, or starts a
//! process - the binding it is handed must already have come from a fresh
//! call to
//! [`crate::patch_manager::resolve_duckstation_native_launch_binding`] (see
//! [`crate::launch::duckstation_execution`] for where that happens).
//!
//! # Scope
//!
//! Only the first supported native DuckStation launch slice: `PSX`
//! platform, a direct regular `.iso`, validated complete `.cue`/`.bin`, or
//! `.chd` file, a verified PS1 serial,
//! and an exact eligible [`DuckStationNativeLaunchBinding`]. Mounted/archive
//! content, Flatpak, Portable/AppImage, and `Explicit` installs are all
//! refused here - never silently widened.
//!
//! `.chd` is included alongside `.iso` (unlike PCSX2's PS2 slice, which is
//! `.iso`-only today) because it is already safely represented by the
//! current archive-kind registry as a non-mount-input
//! [`crate::ArchiveKind::DirectGameImage`] - `media_registry::kind_for_extension`
//! maps both `"iso"` and `"chd"` to it, so both already resolve through
//! [`crate::launch::evidence_bridge::launch_content_ref_from_archive_record`]
//! as [`LaunchContainerKind::PlainFile`] with `requires_mount: false`. Other
//! formats DuckStation itself can read directly (`.pbp`,
//! `.ecm`, `.mds`/`.mdf`, `.ccd`) are not yet classified by that registry at
//! all, so they are refused here rather than guessed at.
//!
//! # Exact argv contract
//!
//! `[executable] -batch -- [content]` - see
//! [`crate::launch::duckstation_execution`]'s own module doc comment for
//! why `-batch` is used (proven from upstream source: it makes the
//! DuckStation Qt frontend process exit when the emulation session ends,
//! rather than returning to an open game-list window, which is what an
//! EmuWiz watcher needs). `--` is always included (not conditional on the
//! content path's own text) so a future content path that happens to start
//! with `-` is still parsed as the boot filename, never as a flag -
//! [`DuckStationUserDirectoryMode`] has no variant that adds any further
//! argument (DuckStation's current CLI has no directory-override flag at
//! all - see `duckstation_local.rs`'s own binding doc comment), so the
//! trailing content path is always the last argument.
//!
//! Every argument is carried as its own `OsString` - spaces, quotes, and
//! shell-looking characters in a path are inert data, never shell syntax.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchCandidate, LaunchContainerKind, LaunchTarget,
};
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{
    DuckStationLaunchBlocker, DuckStationNativeLaunchBinding, DuckStationUserDirectoryMode,
};

/// The only platform this native launch slice supports.
pub const DUCKSTATION_SUPPORTED_PLATFORM_ID: &str = "PSX";

/// The only direct content extensions this slice supports (lowercase, no
/// dot). A `.cue` is accepted only when the caller carries the explicit
/// [`LaunchContainerKind::CueBin`] marker produced after complete-release
/// validation; a lone `.bin` is always refused because its track geometry is
/// not authoritative.
const DUCKSTATION_SUPPORTED_EXTENSIONS: &[&str] = &["iso", "cue", "chd"];

/// The executable invocation data for a DuckStation launch that has passed
/// every fail-closed check. This is data only: no type in this module
/// implements process spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: DuckStationCommandSelection,
}

/// The facts that produced the command's argv - profile/binding, platform,
/// verified PS1 serial, and content path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationCommandSelection {
    pub profile_id: String,
    pub user_directory_mode: DuckStationUserDirectoryMode,
    pub platform_id: String,
    pub verified_ps1_serial: String,
    pub content_path: PathBuf,
}

/// A successful command, or the structured reasons a command was withheld.
/// `command` is `None` whenever `blockers` is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationCommandPlan {
    pub command: Option<DuckStationCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

impl DuckStationCommandPlan {
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

pub(crate) fn direct_ps1_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            DUCKSTATION_SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn direct_ps1_content_is_supported(
    path: &std::path::Path,
    container: Option<LaunchContainerKind>,
) -> bool {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("cue") => {
            container == Some(LaunchContainerKind::CueBin)
        }
        Some(extension) => DUCKSTATION_SUPPORTED_EXTENSIONS.iter().any(|supported| {
            !supported.eq_ignore_ascii_case("cue") && extension.eq_ignore_ascii_case(supported)
        }),
        None => false,
    }
}

/// Builds a safe DuckStation argv plan from only an already-authorized
/// launch candidate, an already-computed launch binding result, and the
/// verified PS1 serial the caller freshly re-confirmed.
///
/// `binding` is a `Result` rather than a bare [`DuckStationNativeLaunchBinding`]
/// so a caller's fresh
/// [`crate::patch_manager::resolve_duckstation_native_launch_binding`]
/// failure (ambiguous executable, unsupported install type, portable-marker
/// conflict, etc.) flows straight into this plan's blockers instead of
/// forcing the caller to invent a placeholder success value.
pub fn build_duckstation_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_ps1_serial: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<DuckStationNativeLaunchBinding, DuckStationLaunchBlocker>,
) -> DuckStationCommandPlan {
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
        && resolved.platform_id != DUCKSTATION_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::DuckStationPlatformMismatch,
            format!(
                "resolved identity targets {}, but only {DUCKSTATION_SUPPORTED_PLATFORM_ID} is \
                 supported by this native DuckStation launch slice",
                resolved.platform_id
            ),
        ));
    }
    if verified_ps1_serial.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::DuckStationSerialMissing,
            "no verified PS1 serial is available for this content",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::DuckStationCandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return DuckStationCommandPlan::blocked(blockers);
    };
    if *adapter_id != "duckstation" {
        blockers.push(blocker(
            LaunchBlockerKind::DuckStationCandidateRequired,
            format!(
                "the supplied launch candidate targets adapter `{adapter_id}`, not `duckstation`"
            ),
        ));
    }

    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied DuckStation launch candidate is marked blocked",
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
                LaunchBlockerKind::DuckStationContentFormatUnsupported,
                "content path is an outer archive/mount-input path, not direct content",
            ));
        } else if !direct_ps1_content_is_supported(path, candidate.content.container) {
            blockers.push(blocker(
                LaunchBlockerKind::DuckStationContentFormatUnsupported,
                "only a direct .iso/.chd or a structurally validated complete .cue/.bin release \
                 is supported by this native DuckStation launch slice",
            ));
        }
    }

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::DuckStationBindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    if !blockers.is_empty() {
        return DuckStationCommandPlan::blocked(blockers);
    }

    let resolved = resolved.expect("identity is Resolved when no blockers exist");
    let verified_ps1_serial =
        verified_ps1_serial.expect("a verified PS1 serial is required when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let binding = binding.expect("a launch binding is required when no blockers exist");

    let arguments = vec![
        OsString::from("-batch"),
        OsString::from("--"),
        content_path.clone().into_os_string(),
    ];

    DuckStationCommandPlan {
        command: Some(DuckStationCommand {
            executable: binding.executable.clone(),
            arguments,
            working_directory: None,
            selection: DuckStationCommandSelection {
                profile_id: profile_id.clone(),
                user_directory_mode: binding.user_directory_mode,
                platform_id: resolved.platform_id.clone(),
                verified_ps1_serial: verified_ps1_serial.to_string(),
                content_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
