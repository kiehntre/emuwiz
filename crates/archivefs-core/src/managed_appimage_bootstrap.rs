//! Explicit, bounded first-run initialization for EmuWiz-managed AppImages.
//!
//! This module is deliberately separate from normal launch. It authorizes
//! only the two curated AppImage lanes that need native first-run config,
//! starts only the exact managed executable, and never supplies a game or a
//! shell command. The emulator remains responsible for creating its own
//! configuration; EmuWiz only verifies the resulting evidence.

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::emulator_download::{
    EmulatorDistribution, EmulatorDownloadSpec, managed_appimage_install,
};

pub const DEFAULT_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedAppImageBootstrapKind {
    Ppsspp,
    Pcsx2,
}

impl ManagedAppImageBootstrapKind {
    fn from_spec(spec: &EmulatorDownloadSpec) -> Option<Self> {
        if spec.distribution != EmulatorDistribution::GithubAppImage {
            return None;
        }
        match spec.id {
            "ppsspp" => Some(Self::Ppsspp),
            "pcsx2" => Some(Self::Pcsx2),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedAppImageBootstrapReceipt {
    pub kind: ManagedAppImageBootstrapKind,
    pub executable: PathBuf,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedAppImageBootstrapError {
    UnsupportedEmulator,
    ManagedInstallMissing,
    ConfigAlreadyPresent,
    EnvironmentUnavailable(&'static str),
    SpawnFailed(String),
    NonZeroExit(Option<i32>),
    TimedOut,
    ConfigStillMissing(PathBuf),
    ConfigUnsafe(PathBuf),
}

impl std::fmt::Display for ManagedAppImageBootstrapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedEmulator => formatter.write_str(
                "first-run initialization is supported only for managed PPSSPP and PCSX2 AppImages",
            ),
            Self::ManagedInstallMissing => {
                formatter.write_str("the exact EmuWiz-managed AppImage is not available")
            }
            Self::ConfigAlreadyPresent => {
                formatter.write_str("the emulator is already initialized")
            }
            Self::EnvironmentUnavailable(name) => write!(formatter, "{name} is not available"),
            Self::SpawnFailed(detail) => {
                write!(formatter, "could not start the managed emulator: {detail}")
            }
            Self::NonZeroExit(code) => {
                write!(formatter, "the emulator exited unsuccessfully ({code:?})")
            }
            Self::TimedOut => {
                formatter.write_str("the emulator did not finish initialization in time")
            }
            Self::ConfigStillMissing(path) => write!(
                formatter,
                "the emulator exited, but its required configuration was not created: {}",
                path.display()
            ),
            Self::ConfigUnsafe(path) => write!(
                formatter,
                "required configuration path is unsafe: {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ManagedAppImageBootstrapError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequiredConfig {
    root: PathBuf,
    evidence: Vec<PathBuf>,
}

fn environment_config(
    kind: ManagedAppImageBootstrapKind,
) -> Result<RequiredConfig, ManagedAppImageBootstrapError> {
    let home = env::var_os("HOME").map(PathBuf::from).ok_or(
        ManagedAppImageBootstrapError::EnvironmentUnavailable("HOME"),
    )?;
    let xdg_config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let xdg_data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    Ok(match kind {
        ManagedAppImageBootstrapKind::Ppsspp => RequiredConfig {
            root: xdg_config.join("ppsspp"),
            evidence: vec![
                xdg_config.join("ppsspp/PSP/SYSTEM/ppsspp.ini"),
                xdg_data.join("ppsspp/PSP/SYSTEM/ppsspp.ini"),
            ],
        },
        ManagedAppImageBootstrapKind::Pcsx2 => RequiredConfig {
            root: xdg_config.join("PCSX2"),
            evidence: vec![
                xdg_config.join("PCSX2/inis/PCSX2.ini"),
                xdg_config.join("PCSX2/PCSX2.ini"),
            ],
        },
    })
}

fn safe_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

#[derive(serde::Deserialize)]
struct ManagedInstallMarker {
    emulator: String,
    installed_path: String,
}

fn exact_managed_install(root: &Path, spec: &EmulatorDownloadSpec) -> Option<PathBuf> {
    let executable = managed_appimage_install(root, spec)?;
    let marker_path = root.join("emulators").join(spec.id).join("install.json");
    let marker = fs::File::open(marker_path).ok()?;
    let mut bytes = Vec::new();
    marker.take(64 * 1024).read_to_end(&mut bytes).ok()?;
    let marker: ManagedInstallMarker = serde_json::from_slice(&bytes).ok()?;
    (marker.emulator == spec.id && marker.installed_path == spec.installed_binary)
        .then_some(executable)
}

fn existing_evidence(config: &RequiredConfig) -> Option<PathBuf> {
    config
        .evidence
        .iter()
        .find(|path| safe_regular_file(path))
        .cloned()
}

/// Read-only readiness check shared by the setup card and the bootstrap
/// worker. It never creates a directory or starts a process.
pub fn managed_appimage_is_initialized(
    spec: &EmulatorDownloadSpec,
) -> Result<bool, ManagedAppImageBootstrapError> {
    let kind = ManagedAppImageBootstrapKind::from_spec(spec)
        .ok_or(ManagedAppImageBootstrapError::UnsupportedEmulator)?;
    Ok(existing_evidence(&environment_config(kind)?).is_some())
}

/// Explicitly initialize one managed PPSSPP or PCSX2 AppImage.
///
/// The function is fail-closed: it validates the managed-install marker and
/// executable before spawning, refuses to run when evidence already exists,
/// passes no game or shell string, bounds the wait, and requires the same
/// emulator-owned evidence after a clean exit. It never creates or edits the
/// configuration itself.
pub fn initialize_managed_appimage(
    root: &Path,
    spec: &EmulatorDownloadSpec,
) -> Result<ManagedAppImageBootstrapReceipt, ManagedAppImageBootstrapError> {
    initialize_managed_appimage_with_timeout(root, spec, DEFAULT_BOOTSTRAP_TIMEOUT)
}

pub fn initialize_managed_appimage_with_timeout(
    root: &Path,
    spec: &EmulatorDownloadSpec,
    timeout: Duration,
) -> Result<ManagedAppImageBootstrapReceipt, ManagedAppImageBootstrapError> {
    let kind = ManagedAppImageBootstrapKind::from_spec(spec)
        .ok_or(ManagedAppImageBootstrapError::UnsupportedEmulator)?;
    let config = environment_config(kind)?;
    // Do the managed-install/provenance check after the read-only readiness
    // check and immediately before entering the spawn helper. This prevents
    // a stale path or marker from authorizing the process.
    let executable = exact_managed_install(root, spec)
        .ok_or(ManagedAppImageBootstrapError::ManagedInstallMissing)?;
    initialize_executable(kind, executable, config, timeout)
}

fn initialize_executable(
    kind: ManagedAppImageBootstrapKind,
    executable: PathBuf,
    config: RequiredConfig,
    timeout: Duration,
) -> Result<ManagedAppImageBootstrapReceipt, ManagedAppImageBootstrapError> {
    if existing_evidence(&config).is_some() {
        return Err(ManagedAppImageBootstrapError::ConfigAlreadyPresent);
    }

    let mut child = Command::new(&executable)
        .args(std::iter::empty::<&str>())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| ManagedAppImageBootstrapError::SpawnFailed(error.to_string()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(ManagedAppImageBootstrapError::NonZeroExit(status.code()));
                }
                let path = existing_evidence(&config).ok_or_else(|| {
                    ManagedAppImageBootstrapError::ConfigStillMissing(config.root.clone())
                })?;
                return Ok(ManagedAppImageBootstrapReceipt {
                    kind,
                    executable,
                    config_path: path,
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ManagedAppImageBootstrapError::TimedOut);
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ManagedAppImageBootstrapError::SpawnFailed(
                    error.to_string(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_download::EmulatorDownloadSpec;
    use std::os::unix::fs::PermissionsExt;

    fn spec(id: &'static str, binary: &'static str) -> EmulatorDownloadSpec {
        EmulatorDownloadSpec {
            id,
            display_name: id,
            profile_name: id,
            distribution: EmulatorDistribution::GithubAppImage,
            official_project: id,
            project_url: "https://example.invalid",
            github_api_url: None,
            flatpak_id: None,
            asset_prefix: None,
            installed_binary: binary,
        }
    }

    fn script(root: &Path, body: &str) -> PathBuf {
        let path = root.join("fake.AppImage");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn install(root: &Path, id: &'static str, binary: &'static str, body: &str) -> PathBuf {
        let installed = root.join("emulators").join(id).join(binary);
        let source = script(root, body);
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        fs::copy(source, &installed).unwrap();
        // The helper writes the provenance marker and validates the AppImage
        // bytes, so use a minimal ELF-shaped fake only for the install marker
        // path in these tests by writing the marker directly below.
        let marker = installed.parent().unwrap().join("install.json");
        fs::write(
            marker,
            format!("{{\"emulator\":\"{id}\",\"installed_path\":\"{binary}\"}}\n"),
        )
        .unwrap();
        installed
    }

    #[test]
    fn unsupported_emulator_is_refused_before_spawn() {
        let root = tempfile::tempdir().unwrap();
        let result = initialize_managed_appimage(root.path(), &spec("rpcs3", "rpcs3.AppImage"));
        assert_eq!(
            result,
            Err(ManagedAppImageBootstrapError::UnsupportedEmulator)
        );
    }

    #[test]
    fn missing_managed_install_is_refused() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            initialize_managed_appimage(root.path(), &spec("ppsspp", "ppsspp.AppImage")),
            Err(ManagedAppImageBootstrapError::ManagedInstallMissing)
        );
    }

    #[test]
    fn mismatched_install_marker_cannot_authorize_initialization() {
        let root = tempfile::tempdir().unwrap();
        let installed = install(root.path(), "ppsspp", "ppsspp.AppImage", "exit 0");
        fs::write(
            installed.parent().unwrap().join("install.json"),
            "{\"emulator\":\"pcsx2\",\"installed_path\":\"pcsx2.AppImage\"}\n",
        )
        .unwrap();
        assert_eq!(
            initialize_managed_appimage(root.path(), &spec("ppsspp", "ppsspp.AppImage")),
            Err(ManagedAppImageBootstrapError::ManagedInstallMissing)
        );
    }

    #[test]
    fn clean_exit_without_config_stays_blocked() {
        let root = tempfile::tempdir().unwrap();
        let installed = install(root.path(), "ppsspp", "ppsspp.AppImage", "exit 0");
        let config_root = root.path().join("config/ppsspp");
        assert!(matches!(
            initialize_executable(
                ManagedAppImageBootstrapKind::Ppsspp,
                installed,
                RequiredConfig {
                    root: config_root.clone(),
                    evidence: vec![config_root.join("PSP/SYSTEM/ppsspp.ini")],
                },
                Duration::from_secs(1)
            ),
            Err(ManagedAppImageBootstrapError::ConfigStillMissing(_))
        ));
    }

    #[test]
    fn successful_ppsspp_first_run_proves_its_config() {
        let root = tempfile::tempdir().unwrap();
        let config_root = root.path().join("config/ppsspp");
        let evidence = config_root.join("PSP/SYSTEM/ppsspp.ini");
        let installed = install(
            root.path(),
            "ppsspp",
            "ppsspp.AppImage",
            &format!(
                "mkdir -p '{}' && touch '{}'",
                evidence.parent().unwrap().display(),
                evidence.display()
            ),
        );
        let receipt = initialize_executable(
            ManagedAppImageBootstrapKind::Ppsspp,
            installed,
            RequiredConfig {
                root: config_root,
                evidence: vec![evidence.clone()],
            },
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(receipt.config_path, evidence);
    }

    #[test]
    fn successful_pcsx2_first_run_proves_its_config() {
        let root = tempfile::tempdir().unwrap();
        let config_root = root.path().join("config/PCSX2");
        let evidence = config_root.join("inis/PCSX2.ini");
        let installed = install(
            root.path(),
            "pcsx2",
            "pcsx2.AppImage",
            &format!(
                "mkdir -p '{}' && touch '{}'",
                evidence.parent().unwrap().display(),
                evidence.display()
            ),
        );
        assert!(
            initialize_executable(
                ManagedAppImageBootstrapKind::Pcsx2,
                installed,
                RequiredConfig {
                    root: config_root,
                    evidence: vec![evidence],
                },
                Duration::from_secs(1),
            )
            .is_ok()
        );
    }

    #[test]
    fn nonzero_exit_stays_blocked() {
        let root = tempfile::tempdir().unwrap();
        let installed = install(root.path(), "pcsx2", "pcsx2.AppImage", "exit 7");
        let config_root = root.path().join("config/PCSX2");
        assert_eq!(
            initialize_executable(
                ManagedAppImageBootstrapKind::Pcsx2,
                installed,
                RequiredConfig {
                    root: config_root.clone(),
                    evidence: vec![config_root.join("inis/PCSX2.ini")],
                },
                Duration::from_secs(1),
            ),
            Err(ManagedAppImageBootstrapError::NonZeroExit(Some(7)))
        );
    }

    #[test]
    fn timeout_kills_the_managed_child() {
        let root = tempfile::tempdir().unwrap();
        let installed = install(root.path(), "ppsspp", "ppsspp.AppImage", "sleep 2");
        let config_root = root.path().join("config/ppsspp");
        assert_eq!(
            initialize_executable(
                ManagedAppImageBootstrapKind::Ppsspp,
                installed,
                RequiredConfig {
                    root: config_root.clone(),
                    evidence: vec![config_root.join("PSP/SYSTEM/ppsspp.ini")],
                },
                Duration::from_millis(10),
            ),
            Err(ManagedAppImageBootstrapError::TimedOut)
        );
    }

    #[test]
    fn existing_config_is_not_started_again() {
        let root = tempfile::tempdir().unwrap();
        let config_root = root.path().join("config/PCSX2");
        let evidence = config_root.join("PCSX2.ini");
        fs::create_dir_all(&config_root).unwrap();
        fs::write(&evidence, "already initialized").unwrap();
        let installed = install(root.path(), "pcsx2", "pcsx2.AppImage", "exit 9");
        assert_eq!(
            initialize_executable(
                ManagedAppImageBootstrapKind::Pcsx2,
                installed,
                RequiredConfig {
                    root: config_root,
                    evidence: vec![evidence],
                },
                Duration::from_secs(1),
            ),
            Err(ManagedAppImageBootstrapError::ConfigAlreadyPresent)
        );
    }
}
