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
    pub expected_firmware_sha256: Option<String>,
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
    FirmwareNotReady,
    UnsupportedExecutable,
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
    if !p.executable_candidates.iter().any(|e| {
        e.path == b.executable && e.version.as_deref().is_some_and(|v| v.starts_with("2."))
    }) {
        return Err(err(
            MesenLaunchPreflightErrorKind::UnsupportedExecutable,
            "Mesen 2 executable evidence is unavailable; a filename and executable bit do not establish the supported launch contract",
        ));
    }
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
    crate::launch::mesen_nes::validate_mesen_profile_binding(&p, roots, &b.executable)
        .map_err(|e| err(MesenLaunchPreflightErrorKind::ConfigurationChanged, e))?;
    if r.expected_platform_id == "NES" {
        super::mesen_nes::validate_mesen_nes_media(&r.selected_content_path)
            .map_err(|e| err(MesenLaunchPreflightErrorKind::ContentFormatUnsupported, e))?;
        let report = crate::game_identity::inspect_catalogued_game_identity(
            &r.selected_content_path,
            Some("NES"),
        );
        let (fresh, _) = super::evidence_bridge::canonical_identity_from_game_report(&report);
        if &fresh != identity {
            return Err(err(
                MesenLaunchPreflightErrorKind::IdentityMismatch,
                "fresh NES/FDS content identity differs from the authorized game",
            ));
        }
        let firmware = super::mesen_nes::inspect_mesen_nes_firmware(&p, &r.selected_content_path);
        if !matches!(
            firmware.firmware,
            super::readiness::FirmwareReadiness::Verified
                | super::readiness::FirmwareReadiness::NotRequired
        ) || firmware.firmware_sha256 != r.expected_firmware_sha256
        {
            return Err(err(
                MesenLaunchPreflightErrorKind::FirmwareNotReady,
                firmware.detail,
            ));
        }
    }
    if file(&r.selected_content_path)? != r.content_identity {
        return Err(err(
            MesenLaunchPreflightErrorKind::ContentChangedBeforeSpawn,
            "content changed immediately before spawn",
        ));
    }
    Ok(PreparedProcessCommand {
        executable: b.executable,
        arguments: std::iter::once(MESEN_DO_NOT_SAVE_SETTINGS.into())
            .chain(
                super::mesen_nes::MESEN_SAFE_MEDIA_SWITCHES
                    .iter()
                    .map(std::ffi::OsString::from),
            )
            .chain(std::iter::once(
                r.selected_content_path.clone().into_os_string(),
            ))
            .collect(),
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
        let root = d.path().join("none/Mesen2");
        fs::create_dir_all(&root).unwrap();
        let cfg = root.join("settings.json");
        fs::write(&cfg, br#"{"Preferences":{"SingleInstance":false}}"#).unwrap();
        let rom = d.path().join("game.nes");
        let mut bytes = vec![0; 16 + 16384];
        bytes[..4].copy_from_slice(b"NES\x1a");
        bytes[4] = 1;
        fs::write(&rom, &bytes).unwrap();
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
            known_version_outputs: BTreeMap::from([(exe.clone(), "Mesen v2.1.1".into())]),
        };
        let r = MesenLaunchRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "NES".into(),
            expected_game_key:
                match super::super::evidence_bridge::canonical_identity_from_game_report(
                    &crate::game_identity::inspect_catalogued_game_identity(&rom, Some("NES")),
                )
                .0
                {
                    CanonicalIdentityStatus::Resolved(i) => i.game_key,
                    _ => panic!("synthetic identity"),
                },
            profile_id: format!("mesen:{}", root.display()),
            expected_executable: exe.clone(),
            content_identity: meta(&rom),
            executable_identity: meta(&exe),
            config_identity: meta(&cfg),
            expected_firmware_sha256: None,
        };
        let id = CanonicalIdentityStatus::Resolved(crate::launch::planning::ResolvedIdentity {
            platform_id: "NES".into(),
            game_key: r.expected_game_key.clone(),
        });
        assert!(preflight_mesen_launch(&r, &roots, &id).is_ok());
        fs::remove_file(&exe).unwrap();
        assert_eq!(
            preflight_mesen_launch(&r, &roots, &id).unwrap_err().kind,
            MesenLaunchPreflightErrorKind::ProfileIneligible
        );
    }
    #[test]
    fn valid_fds_cannot_bypass_firmware_or_executable_contracts() {
        let d = tempdir().unwrap();
        let root = d.path().join("profile");
        fs::create_dir(&root).unwrap();
        let cfg = root.join("settings.json");
        fs::write(&cfg, br#"{"Preferences":{"SingleInstance":false}}"#).unwrap();
        let exe = root.join("Mesen");
        fs::write(&exe, b"fixture only - never executed").unwrap();
        #[cfg(unix)]
        exec(&exe);
        let rom = d.path().join("game.fds");
        let mut side = vec![0; 65500];
        side[0] = 1;
        side[1..15].copy_from_slice(b"*NINTENDO-HVC*");
        side[0x10..0x14].copy_from_slice(b"TST ");
        side[56] = 2;
        side[57] = 1;
        side[58] = 3;
        side[60] = 1;
        side[61..69].copy_from_slice(b"TEST    ");
        side[69..71].copy_from_slice(&0x8000_u16.to_le_bytes());
        side[71..73].copy_from_slice(&4_u16.to_le_bytes());
        side[75] = 4;
        fs::write(&rom, side).unwrap();
        assert!(super::super::mesen_nes::validate_mesen_nes_media(&rom).is_ok());
        let identity = super::super::evidence_bridge::canonical_identity_from_game_report(
            &crate::game_identity::inspect_catalogued_game_identity(&rom, Some("NES")),
        )
        .0;
        let CanonicalIdentityStatus::Resolved(resolved) = &identity else {
            panic!("fixture identity")
        };
        let meta = |p: &Path| CapturedFileIdentity::capture(&fs::symlink_metadata(p).unwrap());
        let roots = MesenProfileDiscoveryRoots {
            home: d.path().into(),
            xdg_config_home: d.path().join("config"),
            explicit_configuration_roots: vec![root.clone()],
            explicit_executables: vec![exe.clone()],
            known_version_outputs: BTreeMap::from([(exe.clone(), "Mesen v2.1.1".into())]),
        };
        let request = MesenLaunchRequest {
            selected_content_path: rom.clone(),
            expected_platform_id: "NES".into(),
            expected_game_key: resolved.game_key.clone(),
            profile_id: format!("mesen:{}", root.display()),
            expected_executable: exe,
            content_identity: meta(&rom),
            executable_identity: meta(&root.join("Mesen")),
            config_identity: meta(&cfg),
            expected_firmware_sha256: None,
        };
        assert_eq!(
            preflight_mesen_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MesenLaunchPreflightErrorKind::FirmwareNotReady
        );
        fs::create_dir(root.join("Firmware")).unwrap();
        fs::write(root.join("Firmware/disksys.rom"), vec![0; 8192]).unwrap();
        assert_eq!(
            preflight_mesen_launch(&request, &roots, &identity)
                .unwrap_err()
                .kind,
            MesenLaunchPreflightErrorKind::FirmwareNotReady
        );
        let mut no_version = roots.clone();
        no_version.known_version_outputs.clear();
        assert_eq!(
            preflight_mesen_launch(&request, &no_version, &identity)
                .unwrap_err()
                .kind,
            MesenLaunchPreflightErrorKind::UnsupportedExecutable
        );
        let mut bytes = fs::read(&rom).unwrap();
        bytes[76] = 1;
        fs::write(&rom, bytes).unwrap();
        let updated = MesenLaunchRequest {
            content_identity: meta(&rom),
            ..request
        };
        assert_eq!(
            preflight_mesen_launch(&updated, &roots, &identity)
                .unwrap_err()
                .kind,
            MesenLaunchPreflightErrorKind::IdentityMismatch
        );
    }
}
