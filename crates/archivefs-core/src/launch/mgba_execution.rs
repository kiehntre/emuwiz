//! Fresh native mGBA launch preflight and process spawn.
//!
//! Only direct, loose `.gb`, `.gbc`, and `.gba` files are accepted.  The
//! canonical identity and platform are supplied by the authoritative identity
//! layer and are checked again immediately before spawn.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::launch::mgba_command::{MGBA_SUPPORTED_PLATFORM_IDS, direct_mgba_extension};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::patch_manager::{
    MgbaProfileDiscoveryRoots, discover_mgba_profiles, resolve_mgba_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
    pub executable_sha256: [u8; 32],
    pub expected_config_path: Option<PathBuf>,
    pub config_identity: Option<CapturedFileIdentity>,
    pub config_sha256: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgbaLaunchPreflightErrorKind {
    ContentPathNotAbsolute,
    ContentNotFound,
    ContentIsSymlink,
    ContentNotRegularFile,
    ContentFormatUnsupported,
    ContentChangedBeforeSpawn,
    IdentityUnresolved,
    IdentityMismatch,
    ProfileNotFound,
    ProfileIneligible,
    BindingUnavailable,
    BindingDrift,
    ConfigurationChanged,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgbaLaunchPreflightError {
    pub kind: MgbaLaunchPreflightErrorKind,
    pub detail: String,
}
fn error(
    kind: MgbaLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> MgbaLaunchPreflightError {
    MgbaLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}
fn file(path: &Path) -> Result<CapturedFileIdentity, MgbaLaunchPreflightError> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| error(MgbaLaunchPreflightErrorKind::ContentNotFound, e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentIsSymlink,
            "selected content is a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentNotRegularFile,
            "selected content is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&meta))
}

const MAX_MGBA_VERIFIED_FILE_BYTES: u64 = crate::patch_manager::MGBA_MAX_EXECUTABLE_BYTES;

fn digest_regular(path: &Path) -> io::Result<(CapturedFileIdentity, [u8; 32])> {
    let mut opened = process_spawn::open_regular_file_nonblocking(path)?;
    let before = CapturedFileIdentity::capture(&opened.metadata()?);
    if before.size > MAX_MGBA_VERIFIED_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mGBA launch file is too large to verify",
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut read_total = 0u64;
    loop {
        let count = opened.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        read_total += count as u64;
        if read_total > MAX_MGBA_VERIFIED_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mGBA launch file grew beyond the verification limit",
            ));
        }
        digest.update(&buffer[..count]);
    }
    if CapturedFileIdentity::capture(&opened.metadata()?) != before
        || file(path).ok() != Some(before)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mGBA launch file changed during inspection",
        ));
    }
    Ok((before, digest.finalize().into()))
}

/// Capture a bounded digest through one non-following regular file handle.
pub fn capture_mgba_executable(path: &Path) -> io::Result<(CapturedFileIdentity, [u8; 32])> {
    digest_regular(path)
}

pub fn capture_mgba_config(path: &Path) -> io::Result<(CapturedFileIdentity, [u8; 32])> {
    digest_regular(path)
}

pub fn preflight_mgba_launch(
    request: &MgbaLaunchRequest,
    roots: &MgbaProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, MgbaLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content path must be absolute",
        ));
    }
    if !MGBA_SUPPORTED_PLATFORM_IDS.contains(&request.expected_platform_id.as_str())
        || !direct_mgba_extension(
            &request.selected_content_path,
            &request.expected_platform_id,
        )
    {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentFormatUnsupported,
            "native mGBA accepts only direct .gb, .gbc, or .gba content matching the selected platform",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed since authorization",
        ));
    }
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => r,
        CanonicalIdentityStatus::Unknown => {
            return Err(error(
                MgbaLaunchPreflightErrorKind::IdentityUnresolved,
                "fresh mGBA identity is unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(error(
                MgbaLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity evidence conflicts",
            ));
        }
    };
    if resolved.platform_id != request.expected_platform_id
        || !MGBA_SUPPORTED_PLATFORM_IDS.contains(&resolved.platform_id.as_str())
        || resolved.game_key != request.expected_game_key
    {
        return Err(error(
            MgbaLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity no longer matches the authorized mGBA content",
        ));
    }
    let profile = discover_mgba_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                MgbaLaunchPreflightErrorKind::ProfileNotFound,
                "authorized mGBA profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "profile is not eligible".into()),
        ));
    }
    let mut current_config_path = None;
    for name in ["config.ini", "config.ini.bak"] {
        let path = profile.configuration_path.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                if current_config_path.is_none() {
                    current_config_path = Some(path);
                }
            }
            Ok(_) => {
                return Err(error(
                    MgbaLaunchPreflightErrorKind::ConfigurationChanged,
                    "mGBA configuration is no longer a regular, non-symlink file",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(error(
                    MgbaLaunchPreflightErrorKind::ConfigurationChanged,
                    format!("mGBA configuration could not be rechecked: {e}"),
                ));
            }
        }
    }
    if current_config_path != request.expected_config_path {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ConfigurationChanged,
            "mGBA configuration selection changed immediately before launch",
        ));
    }
    if profile.config_path != request.expected_config_path
        || request.expected_config_path.is_some() != request.config_identity.is_some()
        || request.config_identity.is_some() != request.config_sha256.is_some()
    {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ConfigurationChanged,
            "mGBA configuration selection changed since authorization",
        ));
    }
    if let Some(expected) = request.config_identity {
        let config = request
            .expected_config_path
            .as_ref()
            .expect("checked above");
        let current = capture_mgba_config(config).map_err(|e| {
            error(
                MgbaLaunchPreflightErrorKind::ConfigurationChanged,
                e.to_string(),
            )
        })?;
        if current != (expected, request.config_sha256.expect("checked above")) {
            return Err(error(
                MgbaLaunchPreflightErrorKind::ConfigurationChanged,
                "mGBA configuration changed since authorization",
            ));
        }
    }
    let binding = resolve_mgba_native_launch_binding(&profile)
        .map_err(|e| error(MgbaLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if binding.executable != request.expected_executable {
        return Err(error(
            MgbaLaunchPreflightErrorKind::BindingDrift,
            "mGBA executable binding changed since authorization",
        ));
    }
    let (executable_identity, executable_sha256) = capture_mgba_executable(&binding.executable)
        .map_err(|e| {
            error(
                MgbaLaunchPreflightErrorKind::BindingUnavailable,
                e.to_string(),
            )
        })?;
    if executable_identity != request.executable_identity
        || executable_sha256 != request.executable_sha256
    {
        return Err(error(
            MgbaLaunchPreflightErrorKind::BindingDrift,
            "mGBA executable changed since authorization",
        ));
    }
    let (content_identity, content_sha256) = digest_regular(&request.selected_content_path)
        .map_err(|e| {
            error(
                MgbaLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
                format!("selected content could not be verified: {e}"),
            )
        })?;
    let content_key = content_sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if content_identity != request.content_identity || content_key != request.expected_game_key {
        return Err(error(
            MgbaLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content no longer matches the verified game bytes immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments: vec![request.selected_content_path.clone().into_os_string()],
        working_directory: None,
    })
}

pub fn spawn_mgba(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn capture(path: &Path) -> CapturedFileIdentity {
        CapturedFileIdentity::capture(&fs::symlink_metadata(path).unwrap())
    }

    #[cfg(unix)]
    fn executable(path: &Path, code: i32) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn fixture(
        extension: &str,
        platform: &str,
    ) -> (
        TempDir,
        MgbaProfileDiscoveryRoots,
        MgbaLaunchRequest,
        CanonicalIdentityStatus,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("game.{extension}"));
        fs::write(&path, b"synthetic cartridge").unwrap();
        let executable_path = dir.path().join("mgba-test");
        executable(&executable_path, 0);
        let config_root = dir.path().join("config");
        fs::create_dir(&config_root).unwrap();
        let config = config_root.join("config.ini");
        fs::write(&config, b"[mGBA]\n").unwrap();
        let roots = MgbaProfileDiscoveryRoots {
            home: dir.path().to_path_buf(),
            xdg_config_home: dir.path().join("unused-xdg"),
            explicit_configuration_roots: vec![config_root.clone()],
            portable_configuration_roots: vec![],
            explicit_executables: vec![executable_path.clone()],
            known_version_outputs: BTreeMap::new(),
        };
        let game_key = digest_regular(&path)
            .unwrap()
            .1
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let request = MgbaLaunchRequest {
            selected_content_path: path.clone(),
            expected_platform_id: platform.into(),
            expected_game_key: game_key.clone(),
            profile_id: format!("mgba:{}", config_root.display()),
            expected_executable: executable_path.clone(),
            content_identity: capture(&path),
            executable_identity: capture(&executable_path),
            executable_sha256: capture_mgba_executable(&executable_path).unwrap().1,
            expected_config_path: Some(config.clone()),
            config_identity: Some(capture(&config)),
            config_sha256: Some(capture_mgba_config(&config).unwrap().1),
        };
        let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key,
        });
        (dir, roots, request, identity)
    }

    #[test]
    fn direct_cartridges_preflight_and_watched_exit() {
        for (ext, platform) in [
            ("gb", "Game Boy"),
            ("gbc", "Game Boy Color"),
            ("gba", "Game Boy Advance"),
        ] {
            let (_dir, roots, request, identity) = fixture(ext, platform);
            let command = preflight_mgba_launch(&request, &roots, &identity).unwrap();
            assert_eq!(
                command.arguments,
                vec![request.selected_content_path.as_os_str().to_os_string()]
            );
            let mut process = spawn_mgba(&command).unwrap();
            let report = loop {
                if let Some(report) = process.poll() {
                    break report;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert!(report.status.as_ref().unwrap().success());
        }
    }

    #[test]
    fn changed_file_archive_and_ambiguous_identity_refuse() {
        let (_dir, roots, request, identity) = fixture("gba", "Game Boy Advance");
        fs::write(&request.selected_content_path, b"changed game bytes").unwrap();
        assert_eq!(
            preflight_mgba_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MgbaLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );
        let (_dir, roots, mut request, identity) = fixture("gba", "Game Boy Advance");
        request.selected_content_path.set_extension("zip");
        assert_eq!(
            preflight_mgba_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MgbaLaunchPreflightErrorKind::ContentFormatUnsupported
        );
        let (_dir, roots, request, _) = fixture("gbc", "Game Boy Color");
        assert_eq!(
            preflight_mgba_launch(&request, &roots, &CanonicalIdentityStatus::Conflicting)
                .unwrap_err()
                .kind,
            MgbaLaunchPreflightErrorKind::IdentityMismatch
        );
    }

    #[test]
    fn same_size_restored_mtime_cannot_hide_changed_content_or_config() {
        let (_dir, roots, request, identity) = fixture("gba", "Game Boy Advance");
        let path = &request.selected_content_path;
        let modified = fs::metadata(path).unwrap().modified().unwrap();
        fs::write(path, b"synthetic cartridgE").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(capture(path), request.content_identity);
        assert_eq!(
            preflight_mgba_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MgbaLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );

        let (_dir, roots, request, identity) = fixture("gb", "Game Boy");
        let config = request.expected_config_path.as_ref().unwrap();
        let modified = fs::metadata(config).unwrap().modified().unwrap();
        fs::write(config, b"[mgba]\n").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(config)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(capture(config), request.config_identity.unwrap());
        assert_eq!(
            preflight_mgba_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MgbaLaunchPreflightErrorKind::ConfigurationChanged
        );
    }

    #[test]
    fn configuration_and_executable_drift_refuse() {
        let (_dir, roots, request, identity) = fixture("gb", "Game Boy");
        fs::write(
            request.expected_config_path.as_ref().unwrap(),
            b"changed config",
        )
        .unwrap();
        assert_eq!(
            preflight_mgba_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MgbaLaunchPreflightErrorKind::ConfigurationChanged
        );
        let (_dir, roots, mut request, identity) = fixture("gb", "Game Boy");
        fs::remove_file(request.expected_config_path.as_ref().unwrap()).unwrap();
        request.expected_config_path = None;
        request.config_identity = None;
        request.config_sha256 = None;
        fs::write(
            roots.explicit_configuration_roots[0].join("config.ini"),
            b"new config",
        )
        .unwrap();
        assert_eq!(
            preflight_mgba_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MgbaLaunchPreflightErrorKind::ConfigurationChanged
        );
        #[cfg(unix)]
        {
            let (_dir, roots, mut request, identity) = fixture("gb", "Game Boy");
            let config = request.expected_config_path.take().unwrap();
            fs::remove_file(&config).unwrap();
            request.config_identity = None;
            request.config_sha256 = None;
            let target = roots.home.join("other.ini");
            fs::write(&target, b"[mGBA]\n").unwrap();
            std::os::unix::fs::symlink(target, config).unwrap();
            assert_eq!(
                preflight_mgba_launch(&request, &roots, &identity)
                    .unwrap_err()
                    .kind,
                MgbaLaunchPreflightErrorKind::ConfigurationChanged
            );
        }
        let (_dir, roots, request, identity) = fixture("gb", "Game Boy");
        fs::remove_file(&request.expected_executable).unwrap();
        let missing = preflight_mgba_launch(&request, &roots, &identity).unwrap_err();
        assert_eq!(
            missing.kind,
            MgbaLaunchPreflightErrorKind::ProfileIneligible
        );
        assert!(missing.detail.contains("no safe mGBA executable"));
        let (_dir, roots, request, identity) = fixture("gb", "Game Boy");
        fs::remove_file(&request.expected_executable).unwrap();
        executable(&request.expected_executable, 7);
        assert_eq!(
            preflight_mgba_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MgbaLaunchPreflightErrorKind::BindingDrift
        );
    }

    #[test]
    fn watched_nonzero_exit_is_reported() {
        let (_dir, roots, mut request, identity) = fixture("gba", "Game Boy Advance");
        executable(&request.expected_executable, 7);
        (request.executable_identity, request.executable_sha256) =
            capture_mgba_executable(&request.expected_executable).unwrap();
        let command = preflight_mgba_launch(&request, &roots, &identity).unwrap();
        let mut process = spawn_mgba(&command).unwrap();
        let report = loop {
            if let Some(report) = process.poll() {
                break report;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(report.status.as_ref().unwrap().code(), Some(7));
    }
}
