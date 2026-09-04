//! Read-only Azahar Phase 1 discovery and Nintendo 3DSX evidence.
//!
//! Only loose `.3dsx` homebrew is launchable here. Retail images and installed
//! titles are classified but deliberately never installed, decrypted, or
//! passed to an emulator command.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const AZAHAR_MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub const AZAHAR_MAX_3DSX_INSPECTION_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AzaharContentForm {
    ThreeDsx,
    ThreeDs,
    Cci,
    Cxi,
    Cia,
    InstalledTitle,
    Unknown,
}

impl AzaharContentForm {
    pub fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "3dsx" => Self::ThreeDsx,
            "3ds" => Self::ThreeDs,
            "cci" => Self::Cci,
            "cxi" => Self::Cxi,
            "cia" => Self::Cia,
            _ => Self::Unknown,
        }
    }
    pub fn directly_launchable(self) -> bool {
        matches!(self, Self::ThreeDsx)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzaharTitleIdentity {
    pub title: Option<String>,
    pub short_title: Option<String>,
    pub description: Option<String>,
    pub source: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AzaharEvidenceState {
    Present,
    Absent,
    Unreadable,
    Oversized,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzaharProfile {
    pub executable: PathBuf,
    pub config: Option<PathBuf>,
    pub config_state: AzaharEvidenceState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzaharDiscoveryRoots {
    pub explicit_executable: Option<PathBuf>,
    pub path: Option<std::ffi::OsString>,
    pub config_root: Option<PathBuf>,
}

pub fn discover_azahar_executable(roots: &AzaharDiscoveryRoots) -> Option<PathBuf> {
    let candidates = roots.explicit_executable.iter().cloned().chain(
        roots
            .path
            .as_ref()
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|p| p.join("azahar")),
    );
    candidates.filter(|p| safe_executable(p)).next()
}

fn safe_executable(path: &Path) -> bool {
    let Ok(m) = fs::symlink_metadata(path) else {
        return false;
    };
    if m.file_type().is_symlink() || !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn discover_azahar_profile(roots: &AzaharDiscoveryRoots, executable: PathBuf) -> AzaharProfile {
    let config = roots
        .config_root
        .as_ref()
        .map(|root| root.join("qt-config.ini"));
    let config_state = match config.as_ref().and_then(|p| fs::metadata(p).ok()) {
        None => AzaharEvidenceState::Absent,
        Some(m) if m.len() > AZAHAR_MAX_CONFIG_BYTES => AzaharEvidenceState::Oversized,
        Some(_) => config
            .as_ref()
            .and_then(|p| fs::read(p).ok())
            .map_or(AzaharEvidenceState::Unreadable, |_| {
                AzaharEvidenceState::Present
            }),
    };
    AzaharProfile {
        executable,
        config,
        config_state,
    }
}

pub fn parse_azahar_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|token| {
            token.chars().next().is_some_and(|c| c.is_ascii_digit()) && token.contains('.')
        })
        .map(|v| {
            v.trim_matches(|c: char| !c.is_ascii_digit() && c != '.')
                .to_string()
        })
        .filter(|v| v.split('.').count() >= 2)
}

pub fn inspect_azahar_3dsx(path: &Path) -> Result<Option<AzaharTitleIdentity>, std::io::Error> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(AZAHAR_MAX_3DSX_INSPECTION_BYTES);
    file.by_ref()
        .take(AZAHAR_MAX_3DSX_INSPECTION_BYTES as u64)
        .read_to_end(&mut bytes)?;
    let bounded = bytes.as_slice();
    let Some(offset) = bounded.windows(4).position(|w| w == b"SMDH") else {
        return Ok(None);
    };
    let title_offset = offset + 4 + 4 + 0x200; // language 1 (English)
    if title_offset + 0x100 > bounded.len() {
        return Ok(None);
    }
    let decode = |slice: &[u8]| -> Option<String> {
        let mut words = Vec::new();
        for pair in slice.chunks_exact(2) {
            let word = u16::from_le_bytes([pair[0], pair[1]]);
            if word == 0 {
                break;
            }
            words.push(word)
        }
        (!words.is_empty())
            .then(|| String::from_utf16_lossy(&words).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    Ok(Some(AzaharTitleIdentity {
        title: decode(&bounded[title_offset..title_offset + 0x80]),
        short_title: decode(&bounded[title_offset..title_offset + 0x80]),
        description: decode(&bounded[title_offset + 0x80..title_offset + 0x100]),
        source: "embedded SMDH",
    }))
}

pub fn inspect_azahar_content(path: &Path) -> Result<AzaharContentForm, std::io::Error> {
    let form = AzaharContentForm::from_path(path);
    if form == AzaharContentForm::ThreeDsx {
        let _ = inspect_azahar_3dsx(path)?;
    }
    Ok(form)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    #[test]
    fn content_forms_are_explicit_and_only_3dsx_launches() {
        assert!(AzaharContentForm::from_path(Path::new("x.3dsx")).directly_launchable());
        assert!(!AzaharContentForm::from_path(Path::new("x.3ds")).directly_launchable());
        assert_eq!(
            AzaharContentForm::from_path(Path::new("x.cia")),
            AzaharContentForm::Cia
        );
    }
    #[test]
    fn malformed_or_absent_smdh_is_safe_and_filename_is_not_identity() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"3DSX not metadata").unwrap();
        assert_eq!(inspect_azahar_3dsx(f.path()).unwrap(), None);
    }
    #[test]
    fn version_parser_has_unknown_fallback() {
        assert_eq!(parse_azahar_version("Azahar 2123.1"), Some("2123.1".into()));
        assert_eq!(parse_azahar_version("unknown"), None);
    }
}
