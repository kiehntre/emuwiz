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

/// The adapter-local profile evidence used by launch binding and future
/// Doctor projection. This is deliberately a projection of the already
/// discovered profile; it does not perform a second filesystem search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kProfileReadiness {
    pub profile_id: String,
    pub executable: Option<Vita3kExecutable>,
    pub version: Option<String>,
    pub config: Option<Vita3kConfigInspection>,
    pub firmware: Vita3kFirmwareState,
    pub vita_fs_path: PathBuf,
    pub blockers: Vec<Vita3kProfileBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kProfileBlocker {
    pub kind: Vita3kProfileBlockerKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vita3kProfileBlockerKind {
    ExecutableMissing,
    ConfigurationUnreadable,
    VitaFilesystemUnavailable,
}

/// Read-only selected-title evidence composed from the profile readiness,
/// installed-title inspection, and content classifier. `title_id` and
/// `content_path` are optional because a Doctor/profile scan may not have a
/// selected game yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vita3kReadinessEvidence {
    pub profile: Vita3kProfileReadiness,
    pub title: Option<Vita3kInstalledTitle>,
    pub license: Option<Vita3kLicenseState>,
    pub content: Option<Vita3kContentDisposition>,
    pub title_blocker: Option<String>,
    pub content_blocker: Option<String>,
    pub first_blocker: Option<String>,
    pub ready: bool,
}

pub fn assess_vita3k_profile(profile: &Vita3kProfile) -> Vita3kProfileReadiness {
    let executable = profile.executable_candidates.first().cloned();
    let version = executable
        .as_ref()
        .and_then(|candidate| candidate.version.clone());
    let mut blockers = Vec::new();
    if executable.is_none() {
        blockers.push(Vita3kProfileBlocker {
            kind: Vita3kProfileBlockerKind::ExecutableMissing,
            detail: "no safe Vita3K executable was discovered".into(),
        });
    }
    if profile
        .config
        .as_ref()
        .is_some_and(|config| !config.readable)
    {
        blockers.push(Vita3kProfileBlocker {
            kind: Vita3kProfileBlockerKind::ConfigurationUnreadable,
            detail: "Vita3K configuration is unreadable or oversized".into(),
        });
    }
    if !directory(&profile.vita_fs_path) {
        blockers.push(Vita3kProfileBlocker {
            kind: Vita3kProfileBlockerKind::VitaFilesystemUnavailable,
            detail: "Vita3K emulated filesystem is missing or unsafe".into(),
        });
    }
    Vita3kProfileReadiness {
        profile_id: profile.profile_id.clone(),
        executable,
        version,
        config: profile.config.clone(),
        firmware: profile.firmware,
        vita_fs_path: profile.vita_fs_path.clone(),
        blockers,
    }
}

pub fn assess_vita3k_readiness(
    profile: &Vita3kProfile,
    title_id: Option<&str>,
    content_path: Option<&Path>,
) -> Vita3kReadinessEvidence {
    let profile_readiness = assess_vita3k_profile(profile);
    let (title, title_blocker) = match title_id {
        Some(title_id) => match inspect_installed_title(profile, title_id) {
            Ok(title) => (Some(title), None),
            Err(detail) => (None, Some(detail)),
        },
        None => (None, None),
    };
    let license = title.as_ref().map(|title| title.license);
    let content = content_path.map(classify_vita3k_content).or_else(|| {
        title
            .as_ref()
            .map(|_| Vita3kContentDisposition::InstalledTitle)
    });
    let content_blocker = match content {
        Some(Vita3kContentDisposition::InstallPackage) => {
            Some("Vita3K install packages are not launchable content".into())
        }
        Some(Vita3kContentDisposition::UnsupportedDirectContent) => {
            Some("content is not an installed Vita3K title".into())
        }
        _ => None,
    };
    let license_blocker = matches!(license, Some(Vita3kLicenseState::Missing))
        .then(|| "installed Vita title license is missing".to_string());
    let first_blocker = profile_readiness
        .blockers
        .first()
        .map(|blocker| blocker.detail.clone())
        .or_else(|| title_blocker.clone())
        .or(license_blocker)
        .or_else(|| content_blocker.clone());
    Vita3kReadinessEvidence {
        profile: profile_readiness,
        title,
        license,
        content,
        title_blocker,
        content_blocker,
        ready: first_blocker.is_none(),
        first_blocker,
    }
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
    let readiness = assess_vita3k_profile(profile);
    let executable = readiness
        .executable
        .as_ref()
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
        profile_id: readiness.profile_id,
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
    use std::fs;

    fn profile_fixture(
        root: &Path,
        executable: Option<&Path>,
        firmware: Vita3kFirmwareState,
    ) -> Vita3kProfile {
        let vita_fs_path = root.join("ux0");
        fs::create_dir_all(&vita_fs_path).unwrap();
        let executable_candidates = executable
            .map(|path| {
                fs::write(path, b"vita3k").unwrap();
                Vita3kExecutable {
                    path: path.to_path_buf(),
                    installation_type: Vita3kInstallationType::Explicit,
                    version: Some("0.2.0".into()),
                }
            })
            .into_iter()
            .collect();
        Vita3kProfile {
            profile_id: "vita3k-test".into(),
            installation_type: Vita3kInstallationType::Explicit,
            configuration_path: root.join("config.yml"),
            config_path: Some(root.join("config.yml")),
            vita_fs_path: vita_fs_path.clone(),
            firmware,
            eligible: true,
            blocker: None,
            executable_candidates,
            config: Some(Vita3kConfigInspection {
                path: root.join("config.yml"),
                readable: true,
                vita_fs_path: Some(vita_fs_path),
            }),
        }
    }

    fn sfo(entries: &[(&str, &str)]) -> Vec<u8> {
        let index_table_len = entries.len() * 16;
        let key_table_start = 20 + index_table_len;
        let mut key_table = Vec::new();
        let mut key_offsets = Vec::new();
        for (key, _) in entries {
            key_offsets.push(key_table.len() as u16);
            key_table.extend_from_slice(key.as_bytes());
            key_table.push(0);
        }
        while key_table.len() % 4 != 0 {
            key_table.push(0);
        }
        let data_table_start = key_table_start + key_table.len();
        let mut data_table = Vec::new();
        let mut data_offsets = Vec::new();
        for (_, value) in entries {
            data_offsets.push(data_table.len() as u32);
            data_table.extend_from_slice(value.as_bytes());
            data_table.push(0);
        }
        let mut output = vec![0u8; 20];
        output[0..4].copy_from_slice(&[0, b'P', b'S', b'F']);
        output[4..8].copy_from_slice(&0x0101_u32.to_le_bytes());
        output[8..12].copy_from_slice(&(key_table_start as u32).to_le_bytes());
        output[12..16].copy_from_slice(&(data_table_start as u32).to_le_bytes());
        output[16..20].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        for (index_number, ((_, value), (key_offset, data_offset))) in entries
            .iter()
            .zip(key_offsets.into_iter().zip(data_offsets))
            .enumerate()
        {
            let mut index = [0u8; 16];
            index[0..2].copy_from_slice(&key_offset.to_le_bytes());
            index[2..4].copy_from_slice(&0x0204_u16.to_le_bytes());
            let value_len = (value.len() + 1) as u32;
            index[4..8].copy_from_slice(&value_len.to_le_bytes());
            index[8..12].copy_from_slice(&value_len.to_le_bytes());
            index[12..16].copy_from_slice(&data_offset.to_le_bytes());
            output.extend_from_slice(&index);
            debug_assert_eq!(index_number, output[16..].chunks_exact(16).count() - 1);
        }
        output.extend_from_slice(&key_table);
        output.extend_from_slice(&data_table);
        output
    }

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

    #[test]
    fn profile_projection_exposes_authoritative_evidence() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("vita3k");
        let profile = profile_fixture(
            root.path(),
            Some(&executable),
            Vita3kFirmwareState::PresentUnverified,
        );
        let readiness = assess_vita3k_profile(&profile);
        assert_eq!(readiness.executable.unwrap().path, executable);
        assert_eq!(readiness.version.as_deref(), Some("0.2.0"));
        assert!(readiness.config.unwrap().readable);
        assert_eq!(readiness.firmware, Vita3kFirmwareState::PresentUnverified);
        assert!(readiness.blockers.is_empty());
    }

    #[test]
    fn profile_blockers_have_deterministic_order() {
        let root = tempfile::tempdir().unwrap();
        let mut profile = profile_fixture(root.path(), None, Vita3kFirmwareState::Missing);
        profile.config = Some(Vita3kConfigInspection {
            path: root.path().join("config.yml"),
            readable: false,
            vita_fs_path: None,
        });
        profile.vita_fs_path = root.path().join("missing-ux0");
        let readiness = assess_vita3k_profile(&profile);
        assert_eq!(
            readiness
                .blockers
                .iter()
                .map(|blocker| blocker.kind)
                .collect::<Vec<_>>(),
            vec![
                Vita3kProfileBlockerKind::ExecutableMissing,
                Vita3kProfileBlockerKind::ConfigurationUnreadable,
                Vita3kProfileBlockerKind::VitaFilesystemUnavailable,
            ]
        );
        assert_eq!(readiness.firmware, Vita3kFirmwareState::Missing);
    }

    #[test]
    fn readiness_distinguishes_title_license_and_content_without_writes() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("vita3k");
        let profile = profile_fixture(
            root.path(),
            Some(&executable),
            Vita3kFirmwareState::PresentUnverified,
        );
        let title_id = "PCSA00001";
        let title_root = profile.vita_fs_path.join("app").join(title_id);
        fs::create_dir_all(title_root.join("sce_sys")).unwrap();
        fs::write(
            title_root.join("sce_sys/param.sfo"),
            sfo(&[
                ("TITLE_ID", title_id),
                ("TITLE", "Test Game"),
                ("CATEGORY", "GD"),
            ]),
        )
        .unwrap();
        fs::create_dir_all(profile.vita_fs_path.join("license/app").join(title_id)).unwrap();
        let marker = root.path().join("marker");
        fs::write(&marker, b"unchanged").unwrap();
        let before = fs::read(&marker).unwrap();

        let evidence = assess_vita3k_readiness(&profile, Some(title_id), Some(&title_root));
        assert_eq!(evidence.title.as_ref().unwrap().title_id, title_id);
        assert_eq!(
            evidence.license,
            Some(Vita3kLicenseState::PresentUnverified)
        );
        assert_eq!(
            evidence.content,
            Some(Vita3kContentDisposition::InstalledTitle)
        );
        assert!(evidence.ready);
        assert!(evidence.first_blocker.is_none());
        assert_eq!(fs::read(marker).unwrap(), before);
    }

    #[test]
    fn readiness_reports_missing_title_license_and_package_blockers() {
        let root = tempfile::tempdir().unwrap();
        let profile = profile_fixture(root.path(), None, Vita3kFirmwareState::Unknown);
        let package = root.path().join("game.vpk");
        fs::write(&package, b"package").unwrap();
        let evidence = assess_vita3k_readiness(&profile, Some("PCSA00001"), Some(&package));
        assert!(evidence.title.is_none());
        assert_eq!(
            evidence.title_blocker.as_deref(),
            Some("the exact Vita title ID is not installed")
        );
        assert_eq!(
            evidence.content,
            Some(Vita3kContentDisposition::InstallPackage)
        );
        assert!(evidence.content_blocker.is_some());
        assert!(!evidence.ready);
        assert!(evidence.first_blocker.is_some());
    }

    #[test]
    fn launch_binding_reuses_profile_projection() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("vita3k");
        let profile = profile_fixture(root.path(), Some(&executable), Vita3kFirmwareState::Unknown);
        let binding = resolve_vita3k_native_launch_binding(&profile).unwrap();
        assert_eq!(binding.executable, executable);
        assert_eq!(binding.profile_id, "vita3k-test");
    }
}
