//! Fresh native Stella launch preflight and process spawn.
//!
//! Only direct, loose `.a26` files are accepted. The canonical identity and
//! platform are supplied by the authoritative identity layer and are
//! checked again immediately before spawn. Stella needs no external
//! BIOS/firmware for Atari 2600 cartridge play (see
//! `crate::patch_manager::stella_local`'s own module doc comment), so
//! exactly like `rmg_execution` there is no firmware evidence parameter
//! here at all.

use std::fs;
use std::path::{Path, PathBuf};

use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::launch::stella_command::{STELLA_SUPPORTED_PLATFORM_ID, direct_atari2600_extension};
use crate::patch_manager::{
    StellaProfileDiscoveryRoots, discover_stella_profiles, resolve_stella_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StellaLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StellaLaunchPreflightErrorKind {
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
pub struct StellaLaunchPreflightError {
    pub kind: StellaLaunchPreflightErrorKind,
    pub detail: String,
}

fn error(
    kind: StellaLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> StellaLaunchPreflightError {
    StellaLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}

fn file(path: &Path) -> Result<CapturedFileIdentity, StellaLaunchPreflightError> {
    let meta = fs::symlink_metadata(path).map_err(|e| {
        error(
            StellaLaunchPreflightErrorKind::ContentNotFound,
            e.to_string(),
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(error(
            StellaLaunchPreflightErrorKind::ContentIsSymlink,
            "selected content is a symlink",
        ));
    }
    if !meta.is_file() {
        return Err(error(
            StellaLaunchPreflightErrorKind::ContentNotRegularFile,
            "selected content is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&meta))
}

/// Live-revalidates `request` from scratch and returns the exact,
/// freshly-rebuilt argv-shaped [`PreparedProcessCommand`] safe to spawn - or
/// refuses with a [`StellaLaunchPreflightError`] naming exactly why. Nothing
/// from an earlier readiness check is trusted: content, executable, and
/// profile discovery are all re-inspected fresh, exactly matching
/// `preflight_rmg_launch`'s own contract.
pub fn preflight_stella_launch(
    request: &StellaLaunchRequest,
    roots: &StellaProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, StellaLaunchPreflightError> {
    if !request.selected_content_path.is_absolute() {
        return Err(error(
            StellaLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content path must be absolute",
        ));
    }
    if request.expected_platform_id != STELLA_SUPPORTED_PLATFORM_ID
        || !direct_atari2600_extension(&request.selected_content_path)
    {
        return Err(error(
            StellaLaunchPreflightErrorKind::ContentFormatUnsupported,
            "native Stella accepts only direct .a26 Atari 2600 content",
        ));
    }
    if crate::archive_kind(&request.selected_content_path).is_some_and(|kind| kind.is_mount_input())
    {
        return Err(error(
            StellaLaunchPreflightErrorKind::ContentFormatUnsupported,
            "content path is an outer archive/mount-input path, not direct content",
        ));
    }
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            StellaLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed since authorization",
        ));
    }
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(r) => r,
        CanonicalIdentityStatus::Unknown => {
            return Err(error(
                StellaLaunchPreflightErrorKind::IdentityUnresolved,
                "fresh Stella identity is unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(error(
                StellaLaunchPreflightErrorKind::IdentityMismatch,
                "fresh identity evidence conflicts",
            ));
        }
    };
    if resolved.platform_id != STELLA_SUPPORTED_PLATFORM_ID
        || resolved.platform_id != request.expected_platform_id
        || resolved.game_key != request.expected_game_key
    {
        return Err(error(
            StellaLaunchPreflightErrorKind::IdentityMismatch,
            "fresh identity no longer matches the authorized Stella content",
        ));
    }
    let profile = discover_stella_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or_else(|| {
            error(
                StellaLaunchPreflightErrorKind::ProfileNotFound,
                "authorized Stella profile was not rediscovered",
            )
        })?;
    if !profile.eligible {
        return Err(error(
            StellaLaunchPreflightErrorKind::ProfileIneligible,
            profile
                .blocker
                .unwrap_or_else(|| "profile is not eligible".into()),
        ));
    }
    let binding = resolve_stella_native_launch_binding(&profile)
        .map_err(|e| error(StellaLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if binding.executable != request.expected_executable {
        return Err(error(
            StellaLaunchPreflightErrorKind::BindingDrift,
            "Stella executable binding changed since authorization",
        ));
    }
    let meta = fs::symlink_metadata(&binding.executable).map_err(|e| {
        error(
            StellaLaunchPreflightErrorKind::BindingUnavailable,
            e.to_string(),
        )
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(error(
            StellaLaunchPreflightErrorKind::BindingUnavailable,
            "executable is a symlink or not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(error(
                StellaLaunchPreflightErrorKind::BindingUnavailable,
                "executable has no execute bit set",
            ));
        }
    }
    if CapturedFileIdentity::capture(&meta) != request.executable_identity {
        return Err(error(
            StellaLaunchPreflightErrorKind::BindingDrift,
            "Stella executable changed since authorization",
        ));
    }
    // Final recheck immediately before spawn - the content must not have
    // been swapped out underneath this same preflight call.
    if file(&request.selected_content_path)? != request.content_identity {
        return Err(error(
            StellaLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "selected content changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: binding.executable,
        arguments: vec![request.selected_content_path.clone().into_os_string()],
        working_directory: None,
    })
}

pub fn spawn_stella(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::ResolvedIdentity;
    use crate::patch_manager::{StellaInstallationType, StellaProfile};
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
        roots: StellaProfileDiscoveryRoots,
    }

    fn fixture() -> Fixture {
        let dir = tempdir().unwrap();
        let rom = dir.path().join("Pitfall.a26");
        fs::write(&rom, b"rom-bytes").unwrap();
        let exe = dir.path().join("stella");
        fs::write(&exe, b"exe-bytes").unwrap();
        #[cfg(unix)]
        mark_exec(&exe);
        let roots = StellaProfileDiscoveryRoots {
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

    fn request(fixture: &Fixture) -> StellaLaunchRequest {
        StellaLaunchRequest {
            selected_content_path: fixture.rom.clone(),
            expected_platform_id: "Atari2600".into(),
            expected_game_key: "a26sha".into(),
            profile_id: "stella:native".into(),
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
    fn ready_request_produces_the_exact_argv_no_separator() {
        let fx = fixture();
        let command =
            preflight_stella_launch(&request(&fx), &fx.roots, &identity("Atari2600", "a26sha"))
                .unwrap();
        assert_eq!(command.executable, fx.exe);
        assert_eq!(command.arguments, vec![fx.rom.clone().into_os_string()]);
        assert!(command.working_directory.is_none());
    }

    #[test]
    fn non_atari2600_platform_is_refused() {
        let fx = fixture();
        let mut req = request(&fx);
        req.expected_platform_id = "Atari5200".into();
        let err =
            preflight_stella_launch(&req, &fx.roots, &identity("Atari5200", "a26sha")).unwrap_err();
        assert_eq!(
            err.kind,
            StellaLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn unsupported_extension_is_refused() {
        let fx = fixture();
        let bin = fx.rom.with_extension("bin");
        fs::write(&bin, b"cart").unwrap();
        let mut req = request(&fx);
        req.selected_content_path = bin.clone();
        req.content_identity = CapturedFileIdentity::capture(&fs::symlink_metadata(&bin).unwrap());
        let err =
            preflight_stella_launch(&req, &fx.roots, &identity("Atari2600", "a26sha")).unwrap_err();
        assert_eq!(
            err.kind,
            StellaLaunchPreflightErrorKind::ContentFormatUnsupported
        );
    }

    #[test]
    fn missing_executable_is_refused() {
        let fx = fixture();
        let req = request(&fx);
        fs::remove_file(&fx.exe).unwrap();
        let err =
            preflight_stella_launch(&req, &fx.roots, &identity("Atari2600", "a26sha")).unwrap_err();
        assert_eq!(err.kind, StellaLaunchPreflightErrorKind::ProfileIneligible);
    }

    #[test]
    fn stale_executable_swapped_after_authorization_is_refused_at_final_check() {
        let fx = fixture();
        let mut req = request(&fx);
        req.executable_identity.size += 1;
        let err =
            preflight_stella_launch(&req, &fx.roots, &identity("Atari2600", "a26sha")).unwrap_err();
        assert_eq!(err.kind, StellaLaunchPreflightErrorKind::BindingDrift);
    }

    #[test]
    fn content_changed_before_spawn_is_refused() {
        let fx = fixture();
        let mut req = request(&fx);
        req.content_identity.size += 1;
        let err =
            preflight_stella_launch(&req, &fx.roots, &identity("Atari2600", "a26sha")).unwrap_err();
        assert_eq!(
            err.kind,
            StellaLaunchPreflightErrorKind::ContentChangedBeforeSpawn
        );
    }

    #[test]
    fn no_installation_candidate_is_never_substituted_with_retroarch() {
        let fx = fixture();
        let empty_roots = StellaProfileDiscoveryRoots {
            explicit_executables: Vec::new(),
            path_env: None,
            known_version_outputs: std::collections::BTreeMap::new(),
        };
        let err = preflight_stella_launch(
            &request(&fx),
            &empty_roots,
            &identity("Atari2600", "a26sha"),
        )
        .unwrap_err();
        assert_eq!(err.kind, StellaLaunchPreflightErrorKind::ProfileNotFound);
        // The error is a structured Stella-specific refusal - nothing here
        // ever falls back to building or returning a RetroArch command
        // instead.
    }

    #[test]
    fn ineligible_profile_type_field_is_reachable() {
        // Sanity: StellaProfile/StellaInstallationType are constructible
        // outside this module, matching every other adapter's profile type.
        let _ = StellaProfile {
            profile_id: "stella:native".into(),
            installation_type: StellaInstallationType::Native,
            eligible: false,
            blocker: Some("no safe Stella executable was discovered".into()),
            executable_candidates: Vec::new(),
        };
    }
}
