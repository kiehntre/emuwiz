//! Fresh Mesen preflight: re-discovers the profile and revalidates both exact
//! files immediately before `Command::new`, without a shell or env changes.
use crate::launch::mesen_command::{
    MESEN_DO_NOT_SAVE_SETTINGS, MESEN_SUPPORTED_PLATFORM_IDS, direct_mesen_extension,
};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{
    self, CapturedFileIdentity, PreparedProcessCommand, WatchedProcess,
};
use crate::patch_manager::{
    MesenProfileDiscoveryRoots, discover_mesen_profiles, resolve_mesen_native_launch_binding,
};
use std::fs;
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesenLaunchRequest {
    pub selected_content_path: PathBuf,
    pub expected_platform_id: String,
    pub expected_game_key: String,
    pub profile_id: String,
    pub expected_executable: PathBuf,
    pub content_identity: CapturedFileIdentity,
    pub executable_identity: CapturedFileIdentity,
    pub config_identity: CapturedFileIdentity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MesenLaunchPreflightErrorKind {
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
pub struct MesenLaunchPreflightError {
    pub kind: MesenLaunchPreflightErrorKind,
    pub detail: String,
}
fn err(
    kind: MesenLaunchPreflightErrorKind,
    detail: impl Into<String>,
) -> MesenLaunchPreflightError {
    MesenLaunchPreflightError {
        kind,
        detail: detail.into(),
    }
}
fn file(path: &Path) -> Result<CapturedFileIdentity, MesenLaunchPreflightError> {
    let m = fs::symlink_metadata(path).map_err(|e| {
        err(
            MesenLaunchPreflightErrorKind::ContentNotFound,
            e.to_string(),
        )
    })?;
    if m.file_type().is_symlink() {
        return Err(err(
            MesenLaunchPreflightErrorKind::ContentIsSymlink,
            "path is a symlink",
        ));
    }
    if !m.is_file() {
        return Err(err(
            MesenLaunchPreflightErrorKind::ContentNotRegularFile,
            "path is not a regular file",
        ));
    }
    Ok(CapturedFileIdentity::capture(&m))
}
pub fn preflight_mesen_launch(
    r: &MesenLaunchRequest,
    roots: &MesenProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
) -> Result<PreparedProcessCommand, MesenLaunchPreflightError> {
    if !r.selected_content_path.is_absolute() {
        return Err(err(
            MesenLaunchPreflightErrorKind::ContentPathNotAbsolute,
            "selected content must be absolute",
        ));
    }
    if !MESEN_SUPPORTED_PLATFORM_IDS.contains(&r.expected_platform_id.as_str())
        || !direct_mesen_extension(&r.selected_content_path, &r.expected_platform_id)
    {
        return Err(err(
            MesenLaunchPreflightErrorKind::ContentFormatUnsupported,
            "content is not an authorized direct Mesen form",
        ));
    }
    if file(&r.selected_content_path)? != r.content_identity {
        return Err(err(
            MesenLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed since authorization",
        ));
    }
    let x = match identity {
        CanonicalIdentityStatus::Resolved(x) => x,
        CanonicalIdentityStatus::Unknown => {
            return Err(err(
                MesenLaunchPreflightErrorKind::IdentityUnresolved,
                "identity unresolved",
            ));
        }
        CanonicalIdentityStatus::Conflicting => {
            return Err(err(
                MesenLaunchPreflightErrorKind::IdentityMismatch,
                "identity conflicts",
            ));
        }
    };
    if x.platform_id != r.expected_platform_id || x.game_key != r.expected_game_key {
        return Err(err(
            MesenLaunchPreflightErrorKind::IdentityMismatch,
            "identity changed since authorization",
        ));
    }
    let p = discover_mesen_profiles(roots)
        .profiles
        .into_iter()
        .find(|p| p.profile_id == r.profile_id)
        .ok_or_else(|| {
            err(
                MesenLaunchPreflightErrorKind::ProfileNotFound,
                "Mesen profile was not rediscovered",
            )
        })?;
    if !p.eligible {
        return Err(err(
            MesenLaunchPreflightErrorKind::ProfileIneligible,
            p.blocker.unwrap_or_else(|| "profile is ineligible".into()),
        ));
    }
    if file(&p.config_path).map_err(|e| {
        err(
            MesenLaunchPreflightErrorKind::ConfigurationChanged,
            e.detail,
        )
    })? != r.config_identity
    {
        return Err(err(
            MesenLaunchPreflightErrorKind::ConfigurationChanged,
            "settings changed since authorization",
        ));
    }
    let b = resolve_mesen_native_launch_binding(&p)
        .map_err(|e| err(MesenLaunchPreflightErrorKind::BindingUnavailable, e.detail))?;
    if b.executable != r.expected_executable {
        return Err(err(
            MesenLaunchPreflightErrorKind::BindingDrift,
            "executable binding drifted",
        ));
    }
    if file(&b.executable)
        .map_err(|e| err(MesenLaunchPreflightErrorKind::BindingDrift, e.detail))?
        != r.executable_identity
    {
        return Err(err(
            MesenLaunchPreflightErrorKind::BindingDrift,
            "executable changed since authorization",
        ));
    }
    if file(&r.selected_content_path)? != r.content_identity {
        return Err(err(
            MesenLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: b.executable,
        arguments: vec![
            MESEN_DO_NOT_SAVE_SETTINGS.into(),
            r.selected_content_path.clone().into_os_string(),
        ],
        working_directory: None,
    })
}
pub fn spawn_mesen(command: &PreparedProcessCommand) -> std::io::Result<WatchedProcess> {
    process_spawn::spawn_watched_process(command)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;
    #[cfg(unix)]
    fn exec(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut x = fs::metadata(p).unwrap().permissions();
        x.set_mode(0o755);
        fs::set_permissions(p, x).unwrap()
    }
    #[test]
    fn missing_or_stale_executable_is_refused() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir(&root).unwrap();
        let cfg = root.join("settings.json");
        fs::write(&cfg, b"{}").unwrap();
        let rom = d.path().join("game.nes");
        fs::write(&rom, b"x").unwrap();
        let exe = d.path().join("Mesen");
        fs::write(&exe, b"x").unwrap();
        #[cfg(unix)]
        exec(&exe);
        let meta = |p: &Path| CapturedFileIdentity::capture(&fs::symlink_metadata(p).unwrap());
        let roots = MesenProfileDiscoveryRoots {
            home: d.path().into(),
            xdg_config_home: d.path().join("none"),
            explicit_configuration_roots: vec![root.clone()],
            explicit_executables: vec![exe.clone()],
            known_version_outputs: BTreeMap::new(),
        };
        let r = MesenLaunchRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "NES".into(),
            expected_game_key: "k".into(),
            profile_id: format!("mesen:{}", root.display()),
            expected_executable: exe.clone(),
            content_identity: meta(&rom),
            executable_identity: meta(&exe),
            config_identity: meta(&cfg),
        };
        let id = CanonicalIdentityStatus::Resolved(crate::launch::planning::ResolvedIdentity {
            platform_id: "NES".into(),
            game_key: "k".into(),
        });
        assert!(preflight_mesen_launch(&r, &roots, &id).is_ok());
        fs::remove_file(&exe).unwrap();
        assert_eq!(
            preflight_mesen_launch(&r, &roots, &id).unwrap_err().kind,
            MesenLaunchPreflightErrorKind::ProfileIneligible
        );
    }
}
