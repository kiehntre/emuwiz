//! Fresh VICE C64 preflight. No earlier discovery or file identity is trusted
//! at spawn time; this returns direct argv only and never touches VICE config.
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::vice_command::{
    VICE_ATTACH_CRT, VICE_AUTOSTART, VICE_DISABLE_SAVE_RESOURCES, VICE_SUPPORTED_PLATFORM_ID,
    ViceContentKind, vice_content_kind,
};
use crate::patch_manager::{
    ViceProfileDiscoveryRoots, discover_vice_profiles, resolve_vice_native_launch_binding,
};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViceLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViceLaunchPreflightErrorKind {
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
pub struct ViceLaunchPreflightError {
    pub kind: ViceLaunchPreflightErrorKind,
    pub detail: String,
}
fn fail(kind: ViceLaunchPreflightErrorKind, detail: impl Into<String>) -> ViceLaunchPreflightError {
    ViceLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}
fn file(path: &Path) -> Result<CapturedFileIdentity, ViceLaunchPreflightError> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| fail(ViceLaunchPreflightErrorKind::ContentNotFound, e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentIsSymlink,
            "path is a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentNotRegularFile,
            "path is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&meta))
}
pub fn preflight_vice_launch(
    request: &ViceLaunchRequest,
    roots: &ViceProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, ViceLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content must be absolute",
        ));
    }
    let Some(kind) = vice_content_kind(&request.selected_content_path) else {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentFormatUnsupported,
            "not a direct strong C64 VICE content form",
        ));
    };
    if crate::archive_kind(&request.selected_content_path).is_some_and(|k| k.is_mount_input()) {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentFormatUnsupported,
            "outer archive paths are never direct VICE content",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed since authorization",
        ));
    }
    let CanonicalIdentityStatus::Resolved(resolved) = identity else {
        return Err(fail(
            ViceLaunchPreflightErrorKind::IdentityUnresolved,
            "identity is unresolved or conflicting",
        ));
    };
    if resolved.platform_id != VICE_SUPPORTED_PLATFORM_ID
        || resolved.platform_id != request.expected_platform_id
        || resolved.game_key != request.expected_game_key
    {
        return Err(fail(
            ViceLaunchPreflightErrorKind::IdentityMismatch,
            "identity no longer matches authorized C64 content",
        ));
    }
    let profile = discover_vice_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            fail(
                ViceLaunchPreflightErrorKind::ProfileNotFound,
                "authorized VICE profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "VICE profile is ineligible".into()),
        ));
    }
    let binding = resolve_vice_native_launch_binding(&profile)
        .map_err(|e| fail(ViceLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if binding.executable != request.expected_executable {
        return Err(fail(
            ViceLaunchPreflightErrorKind::BindingDrift,
            "VICE executable binding changed since authorization",
        ));
    }
    let executable_identity = file(&binding.executable)
        .map_err(|e| fail(ViceLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&binding.executable).is_ok_and(|m| m.permissions().mode() & 0o111 == 0) {
            return Err(fail(
                ViceLaunchPreflightErrorKind::BindingUnavailable,
                "VICE executable no longer has an execute bit",
            ));
        }
    }
    if executable_identity != request.executable_identity {
        return Err(fail(
            ViceLaunchPreflightErrorKind::BindingDrift,
            "VICE executable changed since authorization",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(fail(
            ViceLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed immediately before spawn",
        ));
    }
    let arguments = match kind {
        ViceContentKind::Autostart => vec![
            VICE_DISABLE_SAVE_RESOURCES.into(),
            VICE_AUTOSTART.into(),
            request.selected_content_path.clone().into_os_string(),
        ],
        ViceContentKind::Cartridge => vec![
            VICE_DISABLE_SAVE_RESOURCES.into(),
            "+cart".into(),
            VICE_ATTACH_CRT.into(),
            request.selected_content_path.clone().into_os_string(),
        ],
    };
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments,
        working_directory: None,
    })
}
pub fn spawn_vice(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn mark_exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut m = fs::metadata(p).unwrap().permissions();
        m.set_mode(0o755);
        fs::set_permissions(p, m).unwrap();
    }

    fn identity(platform: &str, key: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform.into(),
            game_key: key.into(),
        })
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        rom: PathBuf,
        exe: PathBuf,
        roots: ViceProfileDiscoveryRoots,
        profile_id: String,
    }

    fn fixture() -> Fixture {
        let dir = tempdir().unwrap();
        let rom = dir.path().join("game.t64");
        fs::write(&rom, b"rom-bytes").unwrap();
        let exe = dir.path().join("x64sc");
        fs::write(&exe, b"exe-bytes").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = ViceProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let profile_id = discover_vice_profiles(&roots).profiles[0]
            .profile_id
            .clone();
        Fixture {
            _dir: dir,
            rom,
            exe,
            roots,
            profile_id,
        }
    }

    fn request(fixture: &Fixture) -> ViceLaunchRequest {
        ViceLaunchRequest {
            selected_content_path: fixture.rom.clone(),
            expected_platform_id: "Commodore 64".into(),
            expected_game_key: "c64sha".into(),
            profile_id: fixture.profile_id.clone(),
            expected_executable: fixture.exe.clone(),
            content_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&fixture.rom).unwrap(),
            ),
            executable_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&fixture.exe).unwrap(),
            ),
        }
    }

    #[test]
    fn ready_request_produces_the_exact_argv() {
        let fx = fixture();
        let command = preflight_vice_launch(
            &request(&fx),
            &fx.roots,
            &identity("Commodore 64", "c64sha"),
        )
        .unwrap();
        assert_eq!(command.executable, fx.exe);
        assert_eq!(
            command.arguments,
            vec![
                std::ffi::OsString::from("+saveres"),
                std::ffi::OsString::from("-autostart"),
                fx.rom.clone().into_os_string(),
            ]
        );
        assert!(command.working_directory.is_none());
    }

    #[test]
    fn crt_request_produces_the_exact_cartridge_argv() {
        let fx = fixture();
        let crt = fx.rom.with_extension("crt");
        fs::write(&crt, b"cart-bytes").unwrap();
        let mut req = request(&fx);
        req.selected_content_path = crt.clone();
        req.content_identity = CapturedFileIdentity::capture(&fs::symlink_metadata(&crt).unwrap());
        let command =
            preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha")).unwrap();
        assert_eq!(
            command.arguments,
            vec![
                std::ffi::OsString::from("+saveres"),
                std::ffi::OsString::from("+cart"),
                std::ffi::OsString::from("-cartcrt"),
                crt.into_os_string(),
            ]
        );
    }

    #[test]
    fn non_c64_platform_is_refused() {
        let fx = fixture();
        let mut req = request(&fx);
        req.expected_platform_id = "Commodore 128".into();
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 128", "c64sha"))
            .unwrap_err();
        assert_eq!(err.kind, ViceLaunchPreflightErrorKind::IdentityMismatch);
    }

    #[test]
    fn unsupported_extension_is_refused() {
        let fx = fixture();
        let bin = fx.rom.with_extension("d64");
        fs::write(&bin, b"disk").unwrap();
        let mut req = request(&fx);
        req.selected_content_path = bin.clone();
        req.content_identity = CapturedFileIdentity::capture(&fs::symlink_metadata(&bin).unwrap());
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha"))
            .unwrap_err();
        assert_eq!(
            err.kind,
            ViceLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn missing_executable_is_refused() {
        let fx = fixture();
        let req = request(&fx);
        fs::remove_file(&fx.exe).unwrap();
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha"))
            .unwrap_err();
        // VICE's own discovery filters non-existent executables out of the
        // profile list entirely (see `discover_vice_profiles`'s
        // `.filter(|(p, _)| p.exists())`), so a removed executable makes the
        // authorized profile un-rediscoverable rather than rediscoverable-
        // but-ineligible.
        assert_eq!(err.kind, ViceLaunchPreflightErrorKind::ProfileNotFound);
    }

    #[test]
    fn stale_executable_swapped_after_authorization_is_refused_at_final_check() {
        let fx = fixture();
        let mut req = request(&fx);
        req.executable_identity.size += 1;
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha"))
            .unwrap_err();
        assert_eq!(err.kind, ViceLaunchPreflightErrorKind::BindingDrift);
    }

    #[test]
    fn content_changed_before_spawn_is_refused() {
        let fx = fixture();
        let mut req = request(&fx);
        req.content_identity.size += 1;
        let err = preflight_vice_launch(&req, &fx.roots, &identity("Commodore 64", "c64sha"))
            .unwrap_err();
        assert_eq!(
            err.kind,
            ViceLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );
    }

    #[test]
    fn no_installation_candidate_is_never_substituted_with_retroarch() {
        let fx = fixture();
        let empty_roots = ViceProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let err = preflight_vice_launch(
            &request(&fx),
            &empty_roots,
            &identity("Commodore 64", "c64sha"),
        )
        .unwrap_err();
        assert_eq!(err.kind, ViceLaunchPreflightErrorKind::ProfileNotFound);
        // The error is a structured VICE-specific refusal - nothing here ever
        // falls back to building or returning a RetroArch command instead.
    }

    #[test]
    fn x64_and_x64sc_are_never_silently_substituted() {
        let dir = tempdir().unwrap();
        let x64sc = dir.path().join("x64sc");
        let x64 = dir.path().join("x64");
        fs::write(&x64sc, b"exe-bytes-sc").unwrap();
        fs::write(&x64, b"exe-bytes-fast").unwrap();
        #[cfg(unix)]
        {
            mark_exec(&x64sc);
            mark_exec(&x64);
        }
        let roots = ViceProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let discovery = discover_vice_profiles(&roots);
        let x64sc_profile = discovery
            .profiles
            .iter()
            .find(|p| p.profile_id.ends_with("x64sc"))
            .unwrap();
        let x64_profile = discovery
            .profiles
            .iter()
            .find(|p| p.profile_id.ends_with("x64") && !p.profile_id.ends_with("x64sc"))
            .unwrap();
        assert_ne!(x64sc_profile.profile_id, x64_profile.profile_id);
        let rom = dir.path().join("game.t64");
        fs::write(&rom, b"rom-bytes").unwrap();
        let req = ViceLaunchRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "Commodore 64".into(),
            expected_game_key: "c64sha".into(),
            profile_id: x64sc_profile.profile_id.clone(),
            expected_executable: x64sc.clone(),
            content_identity: CapturedFileIdentity::capture(&fs::symlink_metadata(&rom).unwrap()),
            executable_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(&x64sc).unwrap(),
            ),
        };
        let command =
            preflight_vice_launch(&req, &roots, &identity("Commodore 64", "c64sha")).unwrap();
        // The exact x64sc binary is used - never silently swapped for x64.
        assert_eq!(command.executable, x64sc);
    }
}
