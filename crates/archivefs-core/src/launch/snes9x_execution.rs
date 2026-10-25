//! Fresh native Snes9x launch preflight and process spawn.
//!
//! Only a direct, loose `.sfc`/`.smc` file is accepted.  The canonical
//! identity and platform are supplied by the authoritative identity layer
//! and re-checked immediately before spawn, together with the executable and
//! content file identities captured at authorization time (stale evidence
//! fails closed).  No Snes9x configuration is read or written; no RetroArch
//! fallback exists.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::snes9x_command::{SNES9X_SUPPORTED_PLATFORM_IDS, direct_snes9x_extension};
use crate::patch_manager::{
    Snes9xProfileDiscoveryRoots, discover_snes9x_profiles, resolve_snes9x_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snes9xLaunchPreflightErrorKind {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snes9xLaunchPreflightError {
    pub kind: Snes9xLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(
    kind: Snes9xLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> Snes9xLaunchPreflightError {
    Snes9xLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn file(path: &Path) -> Result<CapturedFileIdentity, Snes9xLaunchPreflightError> {
    let meta = fs::symlink_metadata(path).map_err(|e| {
        error(
            Snes9xLaunchPreflightErrorKind::ContentNotFound,
            e.to_string(),
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::ContentIsSymlink,
            "selected content is a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::ContentNotRegularFile,
            "selected content is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&meta))
}

pub fn preflight_snes9x_launch(
    request: &Snes9xLaunchRequest,
    roots: &Snes9xProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, Snes9xLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content path must be absolute",
        ));
    }
    if !SNES9X_SUPPORTED_PLATFORM_IDS.contains(&request.expected_platform_id.as_str())
        || !direct_snes9x_extension(&request.selected_content_path)
    {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::ContentFormatUnsupported,
            "native Snes9x accepts only a direct `.sfc`/`.smc` SNES content file",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed since authorization",
        ));
    }

    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => r,
        CanonicalIdentityStatus::Unknown => {
            return Err(error(
                Snes9xLaunchPreflightErrorKind::IdentityUnresolved,
                "fresh Snes9x identity is unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(error(
                Snes9xLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity evidence conflicts",
            ));
        }
    };
    if resolved.platform_id != request.expected_platform_id
        || !SNES9X_SUPPORTED_PLATFORM_IDS.contains(&resolved.platform_id.as_str())
        || resolved.game_key != request.expected_game_key
    {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity no longer matches the authorized Snes9x content",
        ));
    }

    let profile = discover_snes9x_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                Snes9xLaunchPreflightErrorKind::ProfileNotFound,
                "authorized Snes9x profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "profile is not eligible".into()),
        ));
    }

    let binding = resolve_snes9x_native_launch_binding(&profile)
        .map_err(|e| error(Snes9xLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if binding.executable != request.expected_executable {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::BindingDrift,
            "Snes9x executable binding changed since authorization",
        ));
    }
    let meta = fs::symlink_metadata(&binding.executable).map_err(|e| {
        error(
            Snes9xLaunchPreflightErrorKind::BindingUnavailable,
            e.to_string(),
        )
    })?;
    if CapturedFileIdentity::capture(&meta) != request.executable_identity {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::BindingDrift,
            "Snes9x executable changed since authorization",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            Snes9xLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed immediately before spawn",
        ));
    }

    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments: vec![request.selected_content_path.clone().into_os_string()],
        working_directory: None,
    })
}

pub fn spawn_snes9x(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn resolved(platform: &str, key: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: key.into(),
        })
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        roots: Snes9xProfileDiscoveryRoots,
        request: Snes9xLaunchRequest,
    }

    fn fixture() -> Fixture {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("snes9x-gtk");
        fs::write(&exe, b"bin").unwrap();
        let mut perms = fs::metadata(&exe).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).unwrap();
        let rom = dir.path().join("Super Mario World.sfc");
        fs::write(&rom, b"rom-bytes").unwrap();

        let roots = Snes9xProfileDiscoveryRoots {
            path_override: Some(dir.path().as_os_str().to_os_string()),
            ..Default::default()
        };
        let profile = discover_snes9x_profiles(&roots).profiles.remove(0);
        let request = Snes9xLaunchRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "SNES".into(),
            expected_game_key: "verified-key".into(),
            profile_id: profile.profile_id.clone(),
            expected_executable: exe.clone(),
            content_identity: CapturedFileIdentity::capture(&fs::symlink_metadata(&rom).unwrap()),
            executable_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&exe).unwrap(),
            ),
        };
        Fixture {
            _dir: dir,
            roots,
            request,
        }
    }

    #[test]
    fn exact_argv_preserves_the_selected_rom_path_without_a_shell() {
        let f = fixture();
        let prepared =
            preflight_snes9x_launch(&f.request, &f.roots, &resolved("SNES", "verified-key"))
                .expect("preflight");
        assert_eq!(prepared.executable, f.request.expected_executable);
        assert_eq!(
            prepared.arguments,
            vec![f.request.selected_content_path.clone().into_os_string()]
        );
        assert_eq!(prepared.arguments.len(), 1);
        assert_eq!(prepared.working_directory, None);
    }

    #[test]
    fn non_snes_identity_fails_closed() {
        let f = fixture();
        let err = preflight_snes9x_launch(&f.request, &f.roots, &resolved("NES", "verified-key"))
            .unwrap_err();
        assert_eq!(err.kind, Snes9xLaunchPreflightErrorKind::IdentityMismatch);
    }

    #[test]
    fn unsupported_extension_fails_closed() {
        let mut f = fixture();
        f.request.selected_content_path = f.request.selected_content_path.with_extension("zip");
        let err = preflight_snes9x_launch(&f.request, &f.roots, &resolved("SNES", "verified-key"))
            .unwrap_err();
        assert_eq!(
            err.kind,
            Snes9xLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn missing_executable_fails_closed() {
        let f = fixture();
        fs::remove_file(&f.request.expected_executable).unwrap();
        let err = preflight_snes9x_launch(&f.request, &f.roots, &resolved("SNES", "verified-key"))
            .unwrap_err();
        assert!(matches!(
            err.kind,
            Snes9xLaunchPreflightErrorKind::ProfileNotFound
                | Snes9xLaunchPreflightErrorKind::ProfileIneligible
                | Snes9xLaunchPreflightErrorKind::BindingUnavailable
        ));
    }

    #[test]
    fn stale_executable_identity_fails_closed() {
        let f = fixture();
        // Re-write the executable so its captured identity no longer matches.
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&f.request.expected_executable, b"different-bin").unwrap();
        let mut perms = fs::metadata(&f.request.expected_executable)
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&f.request.expected_executable, perms).unwrap();
        let err = preflight_snes9x_launch(&f.request, &f.roots, &resolved("SNES", "verified-key"))
            .unwrap_err();
        assert_eq!(err.kind, Snes9xLaunchPreflightErrorKind::BindingDrift);
    }

    #[test]
    fn stale_content_fails_closed() {
        let f = fixture();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&f.request.selected_content_path, b"tampered-rom").unwrap();
        let err = preflight_snes9x_launch(&f.request, &f.roots, &resolved("SNES", "verified-key"))
            .unwrap_err();
        assert_eq!(
            err.kind,
            Snes9xLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );
    }
}
