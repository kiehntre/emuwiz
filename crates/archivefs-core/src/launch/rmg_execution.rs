//! Fresh native RMG launch preflight and process spawn.
//!
//! Only direct, loose `.z64`, `.n64`, and `.v64` files are accepted. The
//! canonical identity and platform are supplied by the authoritative identity
//! layer and are checked again immediately before spawn. RMG needs no
//! external BIOS/PIF ROM for N64 cartridge play (see
//! `crate::patch_manager::rmg_local`'s own module doc comment), so unlike
//! `duckstation_execution`/`flycast_execution` there is no firmware evidence
//! parameter here at all.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::rmg_command::{RMG_SUPPORTED_PLATFORM_ID, direct_n64_extension};
use crate::patch_manager::{
    RmgProfileDiscoveryRoots, discover_rmg_profiles, resolve_rmg_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmgLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RmgLaunchPreflightErrorKind {
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
pub struct RmgLaunchPreflightError {
    pub kind: RmgLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(kind: RmgLaunchPreflightErrorKind, detail: impl Into<String>) -> RmgLaunchPreflightError {
    RmgLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn file(path: &Path) -> Result<CapturedFileIdentity, RmgLaunchPreflightError> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| error(RmgLaunchPreflightErrorKind::ContentNotFound, e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(error(
            RmgLaunchPreflightErrorKind::ContentIsSymlink,
            "selected content is a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(error(
            RmgLaunchPreflightErrorKind::ContentNotRegularFile,
            "selected content is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&meta))
}

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt argv-shaped [`PreparedProcessCommand`] safe to spawn - or
/// refuses with an [`RmgLaunchPreflightError`] naming exactly why. Nothing
/// from an earlier readiness check is trusted: content, executable, and
/// profile discovery are all re-inspected fresh, exactly matching
/// `preflight_mgba_launch`'s own contract.
pub fn preflight_rmg_launch(
    request: &RmgLaunchRequest,
    roots: &RmgProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, RmgLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            RmgLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content path must be absolute",
        ));
    }
    if request.expected_platform_id != RMG_SUPPORTED_PLATFORM_ID
        || !direct_n64_extension(&request.selected_content_path)
    {
        return Err(error(
            RmgLaunchPreflightErrorKind::ContentFormatUnsupported,
            "native RMG accepts only direct .z64, .n64, or .v64 Nintendo 64 content",
        ));
    }
    if crate::archive_kind(&request.selected_content_path).is_some_and(|kind| kind.is_mount_input())
    {
        return Err(error(
            RmgLaunchPreflightErrorKind::ContentFormatUnsupported,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            RmgLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed since authorization",
        ));
    }
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => r,
        CanonicalIdentityStatus::Unknown => {
            return Err(error(
                RmgLaunchPreflightErrorKind::IdentityUnresolved,
                "fresh RMG identity is unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(error(
                RmgLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity evidence conflicts",
            ));
        }
    };
    if resolved.platform_id != RMG_SUPPORTED_PLATFORM_ID
        || resolved.platform_id != request.expected_platform_id
        || resolved.game_key != request.expected_game_key
    {
        return Err(error(
            RmgLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity no longer matches the authorized RMG content",
        ));
    }
    let profile = discover_rmg_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                RmgLaunchPreflightErrorKind::ProfileNotFound,
                "authorized RMG profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            RmgLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "profile is not eligible".into()),
        ));
    }
    let binding = resolve_rmg_native_launch_binding(&profile)
        .map_err(|e| error(RmgLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if binding.executable != request.expected_executable {
        return Err(error(
            RmgLaunchPreflightErrorKind::BindingDrift,
            "RMG executable binding changed since authorization",
        ));
    }
    let meta = fs::symlink_metadata(&binding.executable).map_err(|e| {
        error(
            RmgLaunchPreflightErrorKind::BindingUnavailable,
            e.to_string(),
        )
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(error(
            RmgLaunchPreflightErrorKind::BindingUnavailable,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(error(
                RmgLaunchPreflightErrorKind::BindingUnavailable,
                "executable has no execute bit set",
            ));
        }
    }
    if CapturedFileIdentity::capture(&meta) != request.executable_identity {
        return Err(error(
            RmgLaunchPreflightErrorKind::BindingDrift,
            "RMG executable changed since authorization",
        ));
    }
    // Final recheck immediately before spawn - the content must not have
    // been swapped out underneath this same preflight call.
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            RmgLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments: vec![
            std::ffi::OsString::from("--quit-after-emulation"),
            std::ffi::OsString::from("--"),
            request.selected_content_path.clone().into_os_string(),
        ],
        working_directory: None,
    })
}

pub fn spawn_rmg(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;
    use crate::patch_manager::{RmgInstallationType, RmgProfile};
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
        roots: RmgProfileDiscoveryRoots,
    }

    fn fixture() -> Fixture {
        let dir = tempdir().unwrap();
        let rom = dir.path().join("Mario64.z64");
        fs::write(&rom, b"rom-bytes").unwrap();
        let exe = dir.path().join("RMG");
        fs::write(&exe, b"exe-bytes").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = RmgProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: Some(dir.path().as_os_str().to_owned()),
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        Fixture {
            _dir: dir,
            rom,
            exe,
            roots,
        }
    }

    fn request(fixture: &Fixture) -> RmgLaunchRequest {
        RmgLaunchRequest {
            selected_content_path: fixture.rom.clone(),
            expected_platform_id: "N64".into(),
            expected_game_key: "z64sha".into(),
            profile_id: "rmg:native".into(),
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
        let command =
            preflight_rmg_launch(&request(&fx), &fx.roots, &identity("N64", "z64sha")).unwrap();
        assert_eq!(command.executable, fx.exe);
        assert_eq!(
            command.arguments,
            vec![
                std::ffi::OsString::from("--quit-after-emulation"),
                std::ffi::OsString::from("--"),
                fx.rom.clone().into_os_string(),
            ]
        );
        assert!(command.working_directory.is_none());
    }

    #[test]
    fn non_n64_platform_is_refused() {
        let fx = fixture();
        let mut req = request(&fx);
        req.expected_platform_id = "PSX".into();
        let err = preflight_rmg_launch(&req, &fx.roots, &identity("PSX", "z64sha")).unwrap_err();
        assert_eq!(
            err.kind,
            RmgLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn unsupported_extension_is_refused() {
        let fx = fixture();
        let ndd = fx.rom.with_extension("ndd");
        fs::write(&ndd, b"disk").unwrap();
        let mut req = request(&fx);
        req.selected_content_path = ndd.clone();
        req.content_identity = CapturedFileIdentity::capture(&fs::symlink_metadata(&ndd).unwrap());
        let err = preflight_rmg_launch(&req, &fx.roots, &identity("N64", "z64sha")).unwrap_err();
        assert_eq!(
            err.kind,
            RmgLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn missing_executable_is_refused() {
        let fx = fixture();
        let req = request(&fx);
        fs::remove_file(&fx.exe).unwrap();
        let err = preflight_rmg_launch(&req, &fx.roots, &identity("N64", "z64sha")).unwrap_err();
        assert_eq!(err.kind, RmgLaunchPreflightErrorKind::ProfileIneligible);
    }

    #[test]
    fn stale_executable_swapped_after_authorization_is_refused_at_final_check() {
        let fx = fixture();
        let mut req = request(&fx);
        // Authorization captured a different identity than what's on disk
        // now - simulating the executable having been replaced.
        req.executable_identity.size += 1;
        let err = preflight_rmg_launch(&req, &fx.roots, &identity("N64", "z64sha")).unwrap_err();
        assert_eq!(err.kind, RmgLaunchPreflightErrorKind::BindingDrift);
    }

    #[test]
    fn content_changed_before_spawn_is_refused() {
        let fx = fixture();
        let mut req = request(&fx);
        req.content_identity.size += 1;
        let err = preflight_rmg_launch(&req, &fx.roots, &identity("N64", "z64sha")).unwrap_err();
        assert_eq!(
            err.kind,
            RmgLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );
    }

    #[test]
    fn no_installation_candidate_is_never_substituted_with_retroarch() {
        let fx = fixture();
        let empty_roots = RmgProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let err = preflight_rmg_launch(&request(&fx), &empty_roots, &identity("N64", "z64sha"))
            .unwrap_err();
        assert_eq!(err.kind, RmgLaunchPreflightErrorKind::ProfileNotFound);
        // The error is a structured RMG-specific refusal - nothing here ever
        // falls back to building or returning a RetroArch command instead.
    }

    #[test]
    fn ineligible_profile_type_field_is_reachable() {
        // Sanity: RmgProfile/RmgInstallationType are constructible outside
        // this module, matching every other adapter's profile type.
        let _ = RmgProfile {
            profile_id: "rmg:native".into(),
            installation_type: RmgInstallationType::Native,
            eligible: false,
            blocker: Some("no safe RMG executable was discovered".into()),
            executable_candidates: Vec::new(),
        };
    }
}
