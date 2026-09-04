//! Read-only Vita3K discovery and installed-title inspection.
//!
//! Vita3K has two materially different command-line inputs: an installable
//! package and an already-installed app.  This adapter only models the latter
//! for launch.  VPK/PKG inspection is classification evidence, never an
//! installation request or a launchable path.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::param_sfo::parse_param_sfo;

pub const VITA3K_MAX_PROFILES: usize = 16;
pub const VITA3K_MAX_CONFIG_BYTES: u64 = 256 * 1024;
pub const VITA3K_MAX_SFO_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Vita3kInstallationType {
    Native,
    Portable,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vita3kFirmwareState {
    PresentUnverified,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vita3kLicenseState {
    NotRequired,
    PresentUnverified,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vita3kContentDisposition {
    InstalledTitle,
    InstallPackage,
    UnsupportedDirectContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kExecutable {
    pub path: PathBuf,
    pub installation_type: Vita3kInstallationType,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kConfigInspection {
    pub path: PathBuf,
    pub readable: bool,
    pub vita_fs_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kProfile {
    pub profile_id: String,
    pub installation_type: Vita3kInstallationType,
    pub configuration_path: PathBuf,
    pub config_path: Option<PathBuf>,
    pub vita_fs_path: PathBuf,
    pub firmware: Vita3kFirmwareState,
    pub eligible: bool,
    pub blocker: Option<String>,
    pub executable_candidates: Vec<Vita3kExecutable>,
    pub config: Option<Vita3kConfigInspection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kProfileDiscovery {
    pub profiles: Vec<Vita3kProfile>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kProfileDiscoveryRoots {
    pub home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub explicit_configuration_roots: Vec<PathBuf>,
    pub portable_configuration_roots: Vec<PathBuf>,
    pub explicit_executables: Vec<PathBuf>,
    pub known_version_outputs: BTreeMap<PathBuf, String>,
}

impl Vita3kProfileDiscoveryRoots {
    pub fn from_environment() -> Result<Self, Vita3kDiscoveryError> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(Vita3kDiscoveryError::HomeUnavailable)?;
        let xdg_data_home = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Ok(Self {
            home,
            xdg_data_home,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vita3kDiscoveryError {
    HomeUnavailable,
}

impl std::fmt::Display for Vita3kDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set")
    }
}
impl std::error::Error for Vita3kDiscoveryError {}

fn regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}

fn directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    regular(path) && fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    regular(path)
}

fn executable_candidates(roots: &Vita3kProfileDiscoveryRoots) -> Vec<Vita3kExecutable> {
    let mut paths: Vec<(PathBuf, Vita3kInstallationType)> = roots
        .explicit_executables
        .iter()
        .cloned()
        .map(|p| (p, Vita3kInstallationType::Explicit))
        .collect();
    if let Some(path_env) = env::var_os("PATH") {
        for directory in env::split_paths(&path_env) {
            paths.push((directory.join("vita3k"), Vita3kInstallationType::Native));
            paths.push((directory.join("Vita3K"), Vita3kInstallationType::Native));
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|(path, _)| executable(path))
        .map(|(path, installation_type)| Vita3kExecutable {
            version: roots
                .known_version_outputs
                .get(&path)
                .and_then(|text| parse_vita3k_version(text)),
            path,
            installation_type,
        })
        .collect()
}

fn config_path(root: &Path) -> Option<PathBuf> {
    let path = root.join("config.yml");
    regular(&path).then_some(path)
}

fn configured_path(text: &str, key: &str) -> Option<PathBuf> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| PathBuf::from(value.trim().trim_matches(['\'', '"'])))
    })
}

fn inspect_config(path: &Path, default_fs: &Path) -> Vita3kConfigInspection {
    let bytes = fs::read(path).ok();
    let readable = bytes
        .as_ref()
        .is_some_and(|bytes| bytes.len() as u64 <= VITA3K_MAX_CONFIG_BYTES);
    let text = bytes
        .filter(|bytes| bytes.len() as u64 <= VITA3K_MAX_CONFIG_BYTES)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let vita_fs_path =
        configured_path(&text, "pref-path").or_else(|| Some(default_fs.to_path_buf()));
    Vita3kConfigInspection {
        path: path.to_path_buf(),
        readable,
        vita_fs_path,
    }
}

fn profile(
    root: PathBuf,
    installation_type: Vita3kInstallationType,
    executables: &[Vita3kExecutable],
) -> Vita3kProfile {
    let default_fs = root.join("ux0");
    let config_path = config_path(&root);
    let config = config_path
        .as_ref()
        .map(|path| inspect_config(path, &default_fs));
    let vita_fs_path = config
        .as_ref()
        .and_then(|config| config.vita_fs_path.clone())
        .unwrap_or(default_fs);
    let candidates: Vec<_> = executables
        .iter()
        .filter(|executable| {
            executable.installation_type == installation_type
                || installation_type == Vita3kInstallationType::Explicit
        })
        .cloned()
        .collect();
    let config_ok = config.as_ref().is_none_or(|config| config.readable);
    let eligible = !candidates.is_empty() && config_ok && directory(&vita_fs_path);
    let blocker = (!eligible).then(|| {
        if candidates.is_empty() {
            "no safe Vita3K executable was discovered".to_string()
        } else if !config_ok {
            "Vita3K configuration is unreadable or oversized".to_string()
        } else {
            "Vita3K emulated filesystem is missing or unsafe".to_string()
        }
    });
    let firmware_root = vita_fs_path.join("vs0/sys/external");
    Vita3kProfile {
        profile_id: format!("vita3k:{}", root.display()),
        installation_type,
        configuration_path: root,
        config_path,
        vita_fs_path,
        firmware: if directory(&firmware_root) {
            Vita3kFirmwareState::PresentUnverified
        } else {
            Vita3kFirmwareState::Unknown
        },
        eligible,
        blocker,
        executable_candidates: candidates,
        config,
    }
}

pub fn discover_vita3k_profiles(roots: &Vita3kProfileDiscoveryRoots) -> Vita3kProfileDiscovery {
    let mut roots_to_scan = vec![(
        roots.xdg_data_home.join("Vita3K/Vita3K"),
        Vita3kInstallationType::Native,
    )];
    roots_to_scan.extend(
        roots
            .portable_configuration_roots
            .iter()
            .cloned()
            .map(|path| (path, Vita3kInstallationType::Portable)),
    );
    roots_to_scan.extend(
        roots
            .explicit_configuration_roots
            .iter()
            .cloned()
            .map(|path| (path, Vita3kInstallationType::Explicit)),
    );
    roots_to_scan.sort();
    roots_to_scan.dedup_by(|left, right| left.0 == right.0);
    let executables = executable_candidates(roots);
    Vita3kProfileDiscovery {
        profiles: roots_to_scan
            .into_iter()
            .filter(|(path, kind)| {
                directory(path) || !matches!(kind, Vita3kInstallationType::Native)
            })
            .take(VITA3K_MAX_PROFILES)
            .map(|(path, kind)| profile(path, kind, &executables))
            .collect(),
        complete: true,
    }
}

pub fn parse_vita3k_version(output: &str) -> Option<String> {
    output.lines().map(str::trim).find_map(|line| {
        let lower = line.to_ascii_lowercase();
        let index = lower.find("vita3k")? + "vita3k".len();
        let version = line[index..].trim().trim_start_matches(['v', 'V']).trim();
        (!version.is_empty() && version.chars().any(|c| c.is_ascii_digit()))
            .then(|| version.to_string())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kInstalledTitle {
    pub title_id: String,
    pub root: PathBuf,
    pub title: Option<String>,
    pub category: Option<String>,
    pub license: Vita3kLicenseState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kNativeLaunchBinding {
    pub executable: PathBuf,
    pub profile_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kLaunchBlocker {
    pub kind: Vita3kLaunchBlockerKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vita3kLaunchBlockerKind {
    NoExecutable,
    ProfileIneligible,
    NoInstalledTitle,
}

pub fn resolve_vita3k_native_launch_binding(
    profile: &Vita3kProfile,
) -> Result<Vita3kNativeLaunchBinding, Vita3kLaunchBlocker> {
    let executable = profile
        .executable_candidates
        .first()
        .ok_or_else(|| Vita3kLaunchBlocker {
            kind: Vita3kLaunchBlockerKind::NoExecutable,
            detail: "no safe Vita3K executable is available".into(),
        })?;
    if !profile.eligible {
        return Err(Vita3kLaunchBlocker {
            kind: Vita3kLaunchBlockerKind::ProfileIneligible,
            detail: profile
                .blocker
                .clone()
                .unwrap_or_else(|| "Vita3K profile is not eligible".into()),
        });
    }
    Ok(Vita3kNativeLaunchBinding {
        executable: executable.path.clone(),
        profile_id: profile.profile_id.clone(),
    })
}

fn valid_title_id(value: &str) -> bool {
    (8..=16).contains(&value.len())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

pub fn inspect_installed_title(
    profile: &Vita3kProfile,
    title_id: &str,
) -> Result<Vita3kInstalledTitle, String> {
    if !valid_title_id(title_id) {
        return Err("Vita title ID is not in the accepted form".into());
    }
    let root = profile.vita_fs_path.join("app").join(title_id);
    if !directory(&root) {
        return Err("the exact Vita title ID is not installed".into());
    }
    let param = root.join("sce_sys/param.sfo");
    let bytes =
        fs::read(&param).map_err(|_| "installed title metadata is unreadable".to_string())?;
    if bytes.len() as u64 > VITA3K_MAX_SFO_BYTES {
        return Err("installed title metadata is oversized".into());
    }
    let sfo = parse_param_sfo(&bytes)
        .ok_or_else(|| "installed title metadata is malformed".to_string())?;
    if sfo.get_text("TITLE_ID") != Some(title_id) {
        return Err("installed title metadata does not match the requested title ID".into());
    }
    let category = sfo.get_text("CATEGORY").map(str::to_string);
    let license = match category.as_deref() {
        Some("HB") | Some("hb") => Vita3kLicenseState::NotRequired,
        Some(_) => {
            let path = profile.vita_fs_path.join("license/app").join(title_id);
            if directory(&path) || regular(&path) {
                Vita3kLicenseState::PresentUnverified
            } else {
                Vita3kLicenseState::Missing
            }
        }
        None => Vita3kLicenseState::Unknown,
    };
    Ok(Vita3kInstalledTitle {
        title_id: title_id.to_string(),
        root,
        title: sfo.get_text("TITLE").map(str::to_string),
        category,
        license,
    })
}

pub fn classify_vita3k_content(path: &Path) -> Vita3kContentDisposition {
    if directory(path) && path.join("sce_sys/param.sfo").is_file() {
        Vita3kContentDisposition::InstalledTitle
    } else if path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("vpk") || extension.eq_ignore_ascii_case("pkg")
    }) {
        Vita3kContentDisposition::InstallPackage
    } else {
        Vita3kContentDisposition::UnsupportedDirectContent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_bounded_to_vita3k_lines() {
        assert_eq!(parse_vita3k_version("Vita3K v0.2.0"), Some("0.2.0".into()));
        assert_eq!(parse_vita3k_version("nothing useful"), None);
    }

    #[test]
    fn packages_are_not_launchable_content() {
        assert_eq!(
            classify_vita3k_content(Path::new("game.vpk")),
            Vita3kContentDisposition::InstallPackage
        );
        assert_eq!(
            classify_vita3k_content(Path::new("game.pkg")),
            Vita3kContentDisposition::InstallPackage
        );
        assert_eq!(
            classify_vita3k_content(Path::new("game.iso")),
            Vita3kContentDisposition::UnsupportedDirectContent
        );
    }
}
