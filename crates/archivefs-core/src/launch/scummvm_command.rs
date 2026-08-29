//! Pure native ScummVM command planning for verified extracted game folders.
//!
//! The folder and game ID are accepted only as facts already established by
//! the ScummVM detector. This module performs no detection or filesystem I/O;
//! preflight in [`super::scummvm_execution`] revalidates both immediately
//! before a process can be spawned.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind};
use crate::scummvm_detection::is_valid_scummvm_game_id;

pub const SCUMMVM_SUPPORTED_PLATFORM_ID: &str = "ScummVM";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmNativeLaunchBinding {
    pub executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: ScummVmCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmCommandSelection {
    pub platform_id: String,
    pub game_id: String,
    pub game_folder: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmCommandPlan {
    pub command: Option<ScummVmCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

/// Resolves one safe native Linux ScummVM executable using the same bounded
/// local candidates as the identity detector.
pub fn resolve_scummvm_native_launch_binding() -> Result<ScummVmNativeLaunchBinding, String> {
    let executable = crate::scummvm_detection::resolve_scummvm_executable()
        .ok_or_else(|| "native ScummVM executable is unavailable or unsafe".to_string())?;
    Ok(ScummVmNativeLaunchBinding { executable })
}

/// Test seam for the binding checks used by production planning. It rejects
/// symlinks, non-regular files, and non-executable files.
pub fn resolve_scummvm_native_launch_binding_at(
    executable: &Path,
) -> Result<ScummVmNativeLaunchBinding, String> {
    let metadata = std::fs::symlink_metadata(executable)
        .map_err(|error| format!("ScummVM executable is unavailable: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("ScummVM executable is not a regular non-symlink file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("ScummVM executable is not executable".into());
        }
    }
    Ok(ScummVmNativeLaunchBinding {
        executable: executable.to_path_buf(),
    })
}

fn blocked(blockers: Vec<LaunchBlocker>) -> ScummVmCommandPlan {
    debug_assert!(!blockers.is_empty());
    ScummVmCommandPlan {
        command: None,
        blockers,
    }
}

/// Builds the reviewed ScummVM CLI shape: `-p <folder> <engine:game>`.
/// Each option is a separate [`OsString`]; no shell or concatenated command is
/// involved. The detector's qualified ID is the launch target, never the
/// folder name.
pub fn build_scummvm_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_game_id: Option<&str>,
    game_folder: &Path,
    binding: &Result<ScummVmNativeLaunchBinding, String>,
) -> ScummVmCommandPlan {
    let mut blockers = Vec::new();
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(value) => Some(value),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical ScummVM identity could not be resolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::IdentityConflict,
                "ScummVM identity evidence conflicts and was not resolved",
            ));
            None
        }
    };
    if let Some(resolved) = resolved
        && resolved.platform_id != SCUMMVM_SUPPORTED_PLATFORM_ID
    {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::ScummVmPlatformMismatch,
            format!(
                "resolved identity targets {}, not {SCUMMVM_SUPPORTED_PLATFORM_ID}",
                resolved.platform_id
            ),
        ));
    }
    let Some(game_id) = verified_game_id else {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::ScummVmGameIdMissing,
            "no verified ScummVM engine:game ID is available",
        ));
        return blocked_or_binding(blockers, binding, resolved, game_folder, None);
    };
    if !is_valid_scummvm_game_id(game_id) {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::ScummVmGameIdMissing,
            "verified ScummVM game ID is not a valid engine:game value",
        ));
    }
    if !game_folder.is_absolute() {
        blockers.push(LaunchBlocker::new(
            LaunchBlockerKind::ScummVmContentUnsupported,
            "ScummVM game folder must be an absolute path",
        ));
    }
    blocked_or_binding(blockers, binding, resolved, game_folder, Some(game_id))
}

fn blocked_or_binding(
    mut blockers: Vec<LaunchBlocker>,
    binding: &Result<ScummVmNativeLaunchBinding, String>,
    resolved: Option<&crate::launch::planning::ResolvedIdentity>,
    game_folder: &Path,
    game_id: Option<&str>,
) -> ScummVmCommandPlan {
    let binding = match binding {
        Ok(value) => Some(value),
        Err(detail) => {
            blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::ScummVmBindingUnavailable,
                detail.clone(),
            ));
            None
        }
    };
    if !blockers.is_empty() {
        return blocked(blockers);
    }
    let resolved = resolved.expect("resolved identity when unblocked");
    let game_id = game_id.expect("game ID when unblocked");
    let binding = binding.expect("binding when unblocked");
    ScummVmCommandPlan {
        command: Some(ScummVmCommand {
            executable: binding.executable.clone(),
            arguments: vec![
                OsString::from("-p"),
                game_folder.as_os_str().to_os_string(),
                OsString::from(game_id),
            ],
            working_directory: None,
            selection: ScummVmCommandSelection {
                platform_id: resolved.platform_id.clone(),
                game_id: game_id.to_string(),
                game_folder: game_folder.to_path_buf(),
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
