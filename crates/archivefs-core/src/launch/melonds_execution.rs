//! Fresh, read-only melonDS launch preflight and native process spawn.
//!
//! Preflight rechecks the exact selected file, profile, executable binding,
//! configuration evidence, firmware mode, and upstream identity supplied by
//! the caller. The spawn path uses the generic argv-shaped process helper;
//! it never invokes a shell or substitutes RetroArch/DeSmuME.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::melonds_command::{MELONDS_SUPPORTED_PLATFORM_ID, direct_melonds_extension};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::patch_manager::{
    MelonDsFirmwareMode, MelonDsProfileDiscoveryRoots, discover_melonds_profiles,
    resolve_melonds_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
    pub config_identity: Option<CapturedFileIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MelonDsLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    ContentNotRegularFile,
    ContentFormatUnsupported,
    ContentChangedBeforeSpawn,
    IdentityUnresolved,
    IdentityMismatch,
    DiscoveryFailed,
    ProfileNotFound,
    ProfileIneligible,
    BindingUnavailable,
    BindingDrift,
    ConfigurationChanged,
    FirmwareUnavailable,
    DsiUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MelonDsLaunchPreflightError {
    pub kind: MelonDsLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(
    kind: MelonDsLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> MelonDsLaunchPreflightError {
    MelonDsLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn check_file(path: &Path) -> Result<CapturedFileIdentity, MelonDsLaunchPreflightError> {
    let meta = fs::symlink_metadata(path).map_err(|e| {
        error(
            MelonDsLaunchPreflightErrorKind::ContentNotFound,
            e.to_string(),
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::ContentIsSymlink,
            "selected content is a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::ContentNotRegularFile,
            "selected content is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&meta))
}

fn firmware_ready(profile: &crate::patch_manager::MelonDsProfile) -> bool {
    match profile.firmware.mode {
        MelonDsFirmwareMode::DirectBoot => true,
        MelonDsFirmwareMode::ExternalFirmwareBoot => {
            use crate::patch_manager::MelonDsFirmwareState;
            [
                profile.firmware.bios7,
                profile.firmware.bios9,
                profile.firmware.firmware,
            ]
            .iter()
            .all(|s| matches!(s, MelonDsFirmwareState::PresentUnverified))
        }
        MelonDsFirmwareMode::Unknown => false,
    }
}

/// Revalidates and prepares one exact melonDS launch without starting it.
/// `identity` and `game_key` must be freshly produced by the authoritative
/// identity layer; this function never promotes `.nds` into identity.
pub fn preflight_melonds_launch(
    request: &MelonDsLaunchRequest,
    roots: &MelonDsProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
    game_key: Option<&str>,
) -> Result<PreparedProcessCommand, MelonDsLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content path must be absolute",
        ));
    }
    if !direct_melonds_extension(&request.selected_content_path) {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::ContentFormatUnsupported,
            "native melonDS Phase 1 accepts only .nds",
        ));
    }
    let current_content = check_file(&request.selected_content_path)?;
    if current_content != request.content_identity {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed since authorization",
        ));
    }
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => r,
        CanonicalIdentityStatus::Unknown => {
            return Err(error(
                MelonDsLaunchPreflightErrorKind::IdentityUnresolved,
                "fresh Nintendo DS identity is unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(error(
                MelonDsLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity evidence conflicts",
            ));
        }
    };
    if resolved.platform_id != request.expected_platform_id
        || resolved.platform_id != MELONDS_SUPPORTED_PLATFORM_ID
        || game_key != Some(request.expected_game_key.as_str())
    {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity no longer matches the authorized Nintendo DS content",
        ));
    }
    let discovery = discover_melonds_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                MelonDsLaunchPreflightErrorKind::ProfileNotFound,
                "authorized melonDS profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .clone()
                .unwrap_or_else(|| "profile is not eligible".into()),
        ));
    }
    let config = profile.config_path.as_ref().ok_or_else(|| {
        error(
            MelonDsLaunchPreflightErrorKind::ConfigurationChanged,
            "authorized profile has no readable configuration",
        )
    })?;
    if let Some(expected) = request.config_identity {
        let current = check_file(config).map_err(|e| {
            error(
                MelonDsLaunchPreflightErrorKind::ConfigurationChanged,
                e.detail,
            )
        })?;
        if current != expected {
            return Err(error(
                MelonDsLaunchPreflightErrorKind::ConfigurationChanged,
                "melonDS configuration changed since authorization",
            ));
        }
    }
    if !firmware_ready(profile) {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::FirmwareUnavailable,
            "fresh melonDS firmware mode/evidence is not launch-ready",
        ));
    }
    let binding = resolve_melonds_native_launch_binding(profile).map_err(|e| {
        error(
            MelonDsLaunchPreflightErrorKind::BindingUnavailable,
            e.detail,
        )
    })?;
    if binding.executable != request.expected_executable {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::BindingDrift,
            "melonDS executable binding changed since authorization",
        ));
    }
    let executable_meta = fs::symlink_metadata(&binding.executable).map_err(|e| {
        error(
            MelonDsLaunchPreflightErrorKind::BindingUnavailable,
            e.to_string(),
        )
    })?;
    let executable_identity = CapturedFileIdentity::capture(&executable_meta);
    if executable_identity != request.executable_identity {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::BindingDrift,
            "melonDS executable changed since authorization",
        ));
    }
    let final_content = check_file(&request.selected_content_path)?;
    if final_content != request.content_identity {
        return Err(error(
            MelonDsLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments: vec![request.selected_content_path.clone().into_os_string()],
        working_directory: None,
    })
}

pub fn spawn_melonds(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
    use crate::patch_manager::discover_melonds_profiles;
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn mark_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn melonds_preflight_rechecks_exact_content_and_profile_binding() {
        let dir = tempdir().unwrap();
        let config_dir = dir.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        let config = config_dir.join("melonDS.toml");
        fs::write(&config, "Emu.DirectBoot = true\n").unwrap();
        let executable_path = dir.path().join("melonDS");
        fs::write(&executable_path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        mark_executable(&executable_path);
        let content = dir.path().join("A DS Game.nds");
        fs::write(&content, b"nds fixture").unwrap();
        let roots = MelonDsProfileDiscoveryRoots {
            home: dir.path().to_path_buf(),
            xdg_config_home: dir.path().join("unused"),
            explicit_configuration_roots: vec![config_dir],
            portable_configuration_roots: vec![],
            explicit_executables: vec![executable_path.clone()],
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
        };
        let profile = &discover_melonds_profiles(&roots).profiles[0];
        let content_identity =
            CapturedFileIdentity::capture(&fs::symlink_metadata(&content).unwrap());
        let executable_identity =
            CapturedFileIdentity::capture(&fs::symlink_metadata(&executable_path).unwrap());
        let config_identity =
            CapturedFileIdentity::capture(&fs::symlink_metadata(&config).unwrap());
        let request = MelonDsLaunchRequest {
            selected_content_path: content,
            expected_platform_id: "Nintendo DS".into(),
            expected_game_key: "DS-TEST".into(),
            profile_id: profile.profile_id.clone(),
            expected_executable: executable_path,
            content_identity,
            executable_identity,
            config_identity: Some(config_identity),
        };
        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Nintendo DS".into(),
            game_key: "DS-TEST".into(),
        });
        let command =
            preflight_melonds_launch(&request, &roots, &identity, Some("DS-TEST")).unwrap();
        assert_eq!(command.arguments.len(), 1);
    }
}
