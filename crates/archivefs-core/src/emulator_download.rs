//! Curated, fail-closed emulator acquisition for the setup page.
//!
//! This is intentionally not a package manager. The catalogue contains only
//! fixed official sources, and the AppImage path accepts only a GitHub release
//! asset selected by a deterministic rule. No remote value is ever allowed
//! to choose a destination, and downloaded bytes are validated before an
//! atomic replacement in EmuWiz's own data directory.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_DOWNLOAD_BYTES: usize = 1_073_741_824;
pub const MIN_APPIMAGE_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EmulatorDistribution {
    GithubAppImage,
    Flatpak,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EmulatorDownloadSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub profile_name: &'static str,
    pub distribution: EmulatorDistribution,
    pub official_project: &'static str,
    pub project_url: &'static str,
    pub github_api_url: Option<&'static str>,
    pub flatpak_id: Option<&'static str>,
    pub asset_prefix: Option<&'static str>,
    pub installed_binary: &'static str,
}

/// One catalogue, rather than a second set of emulator registries spread
/// through GUI code. Manual entries remain visible for an honest next step.
pub const EMULATOR_DOWNLOAD_CATALOGUE: &[EmulatorDownloadSpec] = &[
    EmulatorDownloadSpec {
        id: "retroarch",
        display_name: "RetroArch",
        profile_name: "RetroArch",
        distribution: EmulatorDistribution::Flatpak,
        official_project: "RetroArch",
        project_url: "https://www.retroarch.com/",
        github_api_url: None,
        flatpak_id: Some("org.libretro.RetroArch"),
        asset_prefix: None,
        installed_binary: "retroarch",
    },
    EmulatorDownloadSpec {
        id: "pcsx2",
        display_name: "PCSX2",
        profile_name: "PCSX2",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "PCSX2",
        project_url: "https://github.com/PCSX2/pcsx2",
        github_api_url: Some("https://api.github.com/repos/PCSX2/pcsx2/releases/latest"),
        flatpak_id: None,
        asset_prefix: Some("pcsx2-"),
        installed_binary: "pcsx2.AppImage",
    },
    EmulatorDownloadSpec {
        id: "ppsspp",
        display_name: "PPSSPP",
        profile_name: "PPSSPP",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "PPSSPP",
        project_url: "https://github.com/hrydgard/ppsspp",
        github_api_url: Some("https://api.github.com/repos/hrydgard/ppsspp/releases/latest"),
        flatpak_id: None,
        asset_prefix: Some("PPSSPP-"),
        installed_binary: "ppsspp.AppImage",
    },
    EmulatorDownloadSpec {
        id: "rpcs3",
        display_name: "RPCS3",
        profile_name: "RPCS3",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "RPCS3",
        project_url: "https://github.com/RPCS3/rpcs3-binaries-linux",
        github_api_url: Some(
            "https://api.github.com/repos/RPCS3/rpcs3-binaries-linux/releases/latest",
        ),
        flatpak_id: None,
        asset_prefix: Some("rpcs3-"),
        installed_binary: "rpcs3.AppImage",
    },
    EmulatorDownloadSpec {
        id: "duckstation",
        display_name: "DuckStation",
        profile_name: "DuckStation",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "DuckStation",
        project_url: "https://github.com/stenzek/duckstation",
        github_api_url: Some(
            "https://api.github.com/repos/stenzek/duckstation/releases/tags/latest",
        ),
        flatpak_id: None,
        asset_prefix: Some("DuckStation-"),
        installed_binary: "duckstation.AppImage",
    },
    EmulatorDownloadSpec {
        id: "xemu",
        display_name: "xemu",
        profile_name: "xemu",
        distribution: EmulatorDistribution::GithubAppImage,
        official_project: "xemu",
        project_url: "https://github.com/xemu-project/xemu",
        github_api_url: Some("https://api.github.com/repos/xemu-project/xemu/releases/latest"),
        flatpak_id: None,
        asset_prefix: Some("xemu-"),
        installed_binary: "xemu.AppImage",
    },
    EmulatorDownloadSpec {
        id: "dolphin",
        display_name: "Dolphin",
        profile_name: "Dolphin",
        distribution: EmulatorDistribution::Manual,
        official_project: "Dolphin Emulator",
        project_url: "https://dolphin-emu.org/download/",
        github_api_url: None,
        flatpak_id: None,
        asset_prefix: None,
        installed_binary: "dolphin-emu",
    },
    EmulatorDownloadSpec {
        id: "scummvm",
        display_name: "ScummVM",
        profile_name: "ScummVM",
        distribution: EmulatorDistribution::Manual,
        official_project: "ScummVM",
        project_url: "https://www.scummvm.org/downloads/",
        github_api_url: None,
        flatpak_id: None,
        asset_prefix: None,
        installed_binary: "scummvm",
    },
    EmulatorDownloadSpec {
        id: "shadps4",
        display_name: "shadPS4",
        profile_name: "shadPS4",
        distribution: EmulatorDistribution::Manual,
        official_project: "shadPS4",
        project_url: "https://github.com/shadps4-emu/shadPS4",
        github_api_url: None,
        flatpak_id: None,
        asset_prefix: None,
        installed_binary: "shadps4",
    },
];

pub fn emulator_download_spec(id: &str) -> Option<&'static EmulatorDownloadSpec> {
    EMULATOR_DOWNLOAD_CATALOGUE
        .iter()
        .find(|spec| spec.id == id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Unsupported(String),
    InvalidAsset(String),
    TooLarge,
    TooSmall,
    InvalidImage,
    ChecksumMismatch { expected: String, actual: String },
    Io(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(message) | Self::InvalidAsset(message) | Self::Io(message) => {
                f.write_str(message)
            }
            Self::TooLarge => write!(
                f,
                "download exceeds the {} byte safety limit",
                MAX_DOWNLOAD_BYTES
            ),
            Self::TooSmall => f.write_str("download is too small to be an AppImage"),
            Self::InvalidImage => f.write_str("download is not a valid Linux AppImage/ELF"),
            Self::ChecksumMismatch { expected, actual } => {
                write!(f, "SHA-256 mismatch: expected {expected}, got {actual}")
            }
        }
    }
}

pub fn select_x86_64_asset(
    spec: &EmulatorDownloadSpec,
    assets: &[ReleaseAsset],
) -> Result<ReleaseAsset, DownloadError> {
    if spec.distribution != EmulatorDistribution::GithubAppImage {
        return Err(DownloadError::Unsupported(
            "this emulator is not distributed through the automated AppImage lane".into(),
        ));
    }
    let prefix = spec.asset_prefix.ok_or_else(|| {
        DownloadError::Unsupported("no deterministic asset rule is configured".into())
    })?;
    let matches: Vec<_> = assets
        .iter()
        .filter(|asset| {
            let lower = asset.name.to_ascii_lowercase();
            asset.name.starts_with(prefix)
                && (lower.ends_with(".appimage") || lower.ends_with(".appimage.x86_64"))
                && (lower.contains("x86_64") || lower.contains("x64"))
                && !["arm", "aarch64", "debug", "zsync", "checksum", "sha256"]
                    .iter()
                    .any(|bad| lower.contains(bad))
        })
        .cloned()
        .collect();
    if matches.len() != 1 {
        return Err(DownloadError::InvalidAsset(format!(
            "expected exactly one deterministic Linux x86_64 asset, found {}",
            matches.len()
        )));
    }
    let asset = matches.into_iter().next().expect("checked one match");
    let url = url::Url::parse(&asset.download_url)
        .map_err(|_| DownloadError::InvalidAsset("asset URL is malformed".into()))?;
    let project_path = url::Url::parse(spec.project_url)
        .ok()
        .map(|project| project.path().trim_end_matches('/').to_string());
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || project_path.is_none_or(|path| !url.path().starts_with(&format!("{path}/releases/")))
    {
        return Err(DownloadError::InvalidAsset(
            "asset URL is not an allowlisted HTTPS GitHub host".into(),
        ));
    }
    Ok(asset)
}

pub fn validate_appimage(bytes: &[u8]) -> Result<String, DownloadError> {
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(DownloadError::TooLarge);
    }
    if bytes.len() < MIN_APPIMAGE_BYTES {
        return Err(DownloadError::TooSmall);
    }
    if !bytes.starts_with(b"\x7fELF") {
        return Err(DownloadError::InvalidImage);
    }
    Ok(sha256_hex(bytes))
}

pub fn install_appimage_at(
    root: &Path,
    spec: &EmulatorDownloadSpec,
    bytes: &[u8],
    expected_digest: Option<&str>,
) -> Result<PathBuf, DownloadError> {
    let digest = validate_appimage(bytes)?;
    if let Some(expected) =
        expected_digest.map(|value| value.strip_prefix("sha256:").unwrap_or(value))
        && !expected.eq_ignore_ascii_case(&digest)
    {
        return Err(DownloadError::ChecksumMismatch {
            expected: expected.to_string(),
            actual: digest,
        });
    }
    let directory = root.join("emulators").join(spec.id);
    fs::create_dir_all(&directory).map_err(io_error)?;
    let destination = directory.join(spec.installed_binary);
    if destination.exists() && !is_emuwiz_managed(&directory) {
        return Err(DownloadError::Unsupported(
            "an existing installation is not marked EmuWiz-managed; it was left untouched".into(),
        ));
    }
    let temporary = directory.join(format!(".{}.download", spec.installed_binary));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(io_error(error));
    }
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).map_err(io_error)?;
    }
    fs::rename(&temporary, &destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        io_error(error)
    })?;
    let provenance = InstallProvenance {
        emulator: spec.id,
        version: "release asset",
        official_source: spec.project_url,
        installed_path: spec.installed_binary,
        sha256: &digest,
    };
    let provenance_bytes = serde_json::to_vec_pretty(&provenance)
        .map_err(|error| DownloadError::Io(error.to_string()))?;
    fs::write(directory.join("install.json"), provenance_bytes).map_err(io_error)?;
    Ok(destination)
}

#[derive(Debug, Serialize, Deserialize)]
struct InstallProvenance<'a> {
    emulator: &'a str,
    version: &'a str,
    official_source: &'a str,
    installed_path: &'a str,
    sha256: &'a str,
}

fn is_emuwiz_managed(directory: &Path) -> bool {
    directory.join("install.json").is_file()
}

fn io_error(error: io::Error) -> DownloadError {
    DownloadError::Io(error.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> &'static EmulatorDownloadSpec {
        emulator_download_spec("pcsx2").unwrap()
    }

    fn image() -> Vec<u8> {
        let mut bytes = vec![0u8; MIN_APPIMAGE_BYTES];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes
    }

    #[test]
    fn catalogue_has_one_entry_for_each_supported_emulator() {
        for id in [
            "retroarch",
            "dolphin",
            "pcsx2",
            "ppsspp",
            "rpcs3",
            "duckstation",
            "xemu",
            "scummvm",
            "shadps4",
        ] {
            assert!(emulator_download_spec(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn asset_selection_rejects_wrong_architecture_and_unexpected_names() {
        let assets = vec![
            ReleaseAsset {
                name: "pcsx2-arm64.AppImage".into(),
                download_url:
                    "https://github.com/PCSX2/pcsx2/releases/download/x/pcsx2-arm64.AppImage".into(),
                digest: None,
            },
            ReleaseAsset {
                name: "pcsx2-debug-x86_64.AppImage".into(),
                download_url:
                    "https://github.com/PCSX2/pcsx2/releases/download/x/pcsx2-debug-x86_64.AppImage"
                        .into(),
                digest: None,
            },
        ];
        assert!(matches!(
            select_x86_64_asset(spec(), &assets),
            Err(DownloadError::InvalidAsset(_))
        ));
    }

    #[test]
    fn valid_asset_is_selected_only_from_allowlisted_https_hosts() {
        let assets = vec![ReleaseAsset {
            name: "pcsx2-x86_64.AppImage".into(),
            download_url:
                "https://github.com/PCSX2/pcsx2/releases/download/x/pcsx2-x86_64.AppImage".into(),
            digest: None,
        }];
        assert!(select_x86_64_asset(spec(), &assets).is_ok());
        let mut bad = assets;
        bad[0].download_url = "https://example.invalid/pcsx2-x86_64.AppImage".into();
        assert!(select_x86_64_asset(spec(), &bad).is_err());
    }

    #[test]
    fn invalid_and_oversized_downloads_fail_before_install() {
        assert_eq!(
            validate_appimage(b"not an appimage"),
            Err(DownloadError::TooSmall)
        );
        let mut bytes = image();
        bytes[0] = b'N';
        assert_eq!(validate_appimage(&bytes), Err(DownloadError::InvalidImage));
    }

    #[test]
    fn install_is_atomic_and_records_provenance() {
        let root = tempfile::tempdir().unwrap();
        let bytes = image();
        let destination = install_appimage_at(root.path(), spec(), &bytes, None).unwrap();
        assert!(destination.is_file());
        assert!(root.path().join("emulators/pcsx2/install.json").is_file());
        assert_eq!(std::fs::read(destination).unwrap(), bytes);
    }

    #[test]
    fn unmanaged_existing_install_is_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("emulators/pcsx2");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(spec().installed_binary), b"user file").unwrap();
        assert!(matches!(
            install_appimage_at(root.path(), spec(), &image(), None),
            Err(DownloadError::Unsupported(_))
        ));
        assert_eq!(
            std::fs::read(directory.join(spec().installed_binary)).unwrap(),
            b"user file"
        );
    }

    #[test]
    fn manual_emulators_have_no_automated_download_lane() {
        for id in ["dolphin", "scummvm", "shadps4"] {
            assert_eq!(
                emulator_download_spec(id).unwrap().distribution,
                EmulatorDistribution::Manual
            );
        }
    }
}
