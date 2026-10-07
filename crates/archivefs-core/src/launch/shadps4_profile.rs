//! Read-only Linux shadPS4 discovery and configuration inspection. No probes:
//! upstream initializes user paths before main, including for CLI help.
use super::{
    installation::LaunchInstallation,
    installation_known::{KnownEmulator, KnownInstallRoots, discover_appimages},
    native_support::{ExecutableBinding, discover_executables, executable_identity},
    planning::StandaloneProfileInput,
    process_spawn::CapturedFileIdentity,
    readiness::FirmwareReadiness,
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf},
};

pub const ADAPTER_ID: &str = "shadps4";
pub const PLATFORM_ID: &str = "PS4";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_BINARY_BYTES: u64 = 1024 * 1024 * 1024;
const CLI_MARKER: &[u8] = b"shadPS4 Emulator CLI";
pub const SHADPS4: KnownEmulator = KnownEmulator {
    id: "shadPS4",
    appimage_stems: &["shadps4"],
    flatpak_ids: &[],
    portable_markers: &["user"],
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadPs4RefusalKind {
    ExecutableMissing,
    ExecutableNotRunnable,
    ExecutableIdentityUnconfirmed,
    UnsupportedGameLayout,
    IdentityUnverified,
    ConfigurationUnavailable,
    RequiredSysmoduleMissing,
    ChangedAfterPreview,
    UnsafePath,
    SpawnFailed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadPs4Refusal {
    pub kind: ShadPs4RefusalKind,
    pub detail: String,
}
pub(crate) fn refuse(kind: ShadPs4RefusalKind, detail: impl Into<String>) -> ShadPs4Refusal {
    ShadPs4Refusal {
        kind,
        detail: detail.into(),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadPs4ExecutableEvidence {
    /// Static CLI marker in an ELF; recognition, not publisher authentication.
    NativeCliMarker,
    /// Container cannot establish its inner emulator identity. A user selected it.
    UserSelectedAppImage,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadPs4DiscoveryCandidate {
    pub executable: PathBuf,
    /// Only explicit configured paths have this authority; names on PATH do not.
    pub user_selected: bool,
}
/// Fixed known AppImage locations plus bounded explicit/PATH discovery; never run.
pub fn discover_shadps4(
    roots: &KnownInstallRoots,
    configured: &[PathBuf],
    path: Option<&std::ffi::OsStr>,
) -> Vec<ShadPs4DiscoveryCandidate> {
    let mut explicit: Vec<_> = configured.iter().take(16).cloned().collect();
    explicit.extend(
        discover_appimages(&SHADPS4, roots)
            .into_iter()
            .map(|v| v.path),
    );
    discover_executables(&explicit, path, &["shadps4", "shadPS4"])
        .executables
        .into_iter()
        .map(|executable| ShadPs4DiscoveryCandidate {
            user_selected: configured.iter().take(16).any(|p| p == &executable),
            executable,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundFile {
    pub path: PathBuf,
    pub identity: CapturedFileIdentity,
    pub sha256: [u8; 32],
}
impl BoundFile {
    pub fn capture(path: &Path, max: u64) -> Result<Self, ShadPs4Refusal> {
        if !normal_absolute(path) {
            return Err(refuse(
                ShadPs4RefusalKind::UnsafePath,
                "file path must be absolute and normalized",
            ));
        }
        let mut file = open_file(path)?;
        let metadata = file.metadata().map_err(config_io)?;
        if !metadata.is_file() || metadata.len() > max {
            return Err(refuse(
                ShadPs4RefusalKind::UnsafePath,
                "source is not a bounded regular file",
            ));
        }
        let identity = CapturedFileIdentity::capture(&metadata);
        let mut hash = Sha256::new();
        let mut remaining = identity.size;
        let mut buffer = [0; 65536];
        while remaining > 0 {
            let size = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..size]).map_err(config_io)?;
            hash.update(&buffer[..size]);
            remaining -= size as u64;
        }
        let result = Self {
            path: path.into(),
            identity,
            sha256: hash.finalize().into(),
        };
        if !result.metadata_matches()
            || CapturedFileIdentity::capture(&file.metadata().map_err(config_io)?) != identity
        {
            return Err(refuse(
                ShadPs4RefusalKind::ChangedAfterPreview,
                "file changed during inspection",
            ));
        }
        Ok(result)
    }
    fn metadata_matches(&self) -> bool {
        fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.is_file() && CapturedFileIdentity::capture(&m) == self.identity)
    }
    pub fn unchanged(&self) -> bool {
        self.metadata_matches()
            && Self::capture(&self.path, self.identity.size).is_ok_and(|v| v == *self)
    }
    pub fn bytes(&self, max: u64) -> Result<Vec<u8>, ShadPs4Refusal> {
        if self.identity.size > max {
            return Err(refuse(
                ShadPs4RefusalKind::UnsafePath,
                "field exceeds read bound",
            ));
        }
        let mut file = open_file(&self.path)?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(config_io)?;
        if bytes.len() as u64 > max
            || Sha256::digest(&bytes).as_slice() != self.sha256
            || !self.metadata_matches()
        {
            return Err(refuse(
                ShadPs4RefusalKind::ChangedAfterPreview,
                "file changed during read",
            ));
        }
        Ok(bytes)
    }
}
pub(crate) fn normal_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
}
pub(crate) fn real_directory(path: &Path) -> bool {
    let mut prefix = PathBuf::new();
    normal_absolute(path)
        && path.components().all(|c| {
            prefix.push(c.as_os_str());
            fs::symlink_metadata(&prefix).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        })
}
pub(crate) fn open_file(path: &Path) -> Result<fs::File, ShadPs4Refusal> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(config_io)
}
fn config_io(error: std::io::Error) -> ShadPs4Refusal {
    refuse(
        ShadPs4RefusalKind::ConfigurationUnavailable,
        error.to_string(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadPs4ConfigFormat {
    Json,
    LegacyToml,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadPs4SysmoduleState {
    DirectoryMissing,
    PresentUnverified,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadPs4Settings {
    pub fullscreen: Option<bool>,
    pub sysmodules_directory: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadPs4Profile {
    pub profile_id: String,
    pub executable: PathBuf,
    pub installation: LaunchInstallation,
    pub executable_evidence: ShadPs4ExecutableEvidence,
    /// Unknown: no safe version probe, and filenames are not version evidence.
    pub version: Option<String>,
    pub working_directory: PathBuf,
    pub user_directory: PathBuf,
    pub config_root: PathBuf,
    pub config_path: PathBuf,
    pub config_format: ShadPs4ConfigFormat,
    pub settings: ShadPs4Settings,
    pub sysmodules: ShadPs4SysmoduleState,
    pub(crate) executable_binding: ExecutableBinding,
    pub(crate) executable_file: BoundFile,
    pub(crate) config_file: BoundFile,
    pub(crate) xdg_data: PathBuf,
}
impl ShadPs4Profile {
    /// Adapter projection for the existing pure LaunchTarget planner. Firmware
    /// presence is never promoted to verified or universally required.
    pub fn launch_profile_input(&self) -> StandaloneProfileInput {
        StandaloneProfileInput {
            adapter_id: ADAPTER_ID,
            profile_id: self.profile_id.clone(),
            profile_path: Some(self.config_path.clone()),
            eligible: true,
            firmware: match self.sysmodules {
                ShadPs4SysmoduleState::DirectoryMissing => FirmwareReadiness::Unknown,
                ShadPs4SysmoduleState::PresentUnverified => FirmwareReadiness::PresentUnverified,
            },
        }
    }
    pub fn executable_sha256(&self) -> [u8; 32] {
        self.executable_file.sha256
    }
    pub(crate) fn environment(&self) -> Vec<(OsString, OsString)> {
        vec![("XDG_DATA_HOME".into(), self.xdg_data.as_os_str().to_owned())]
    }
}
fn user_root(cwd: &Path, data: &Path) -> Result<PathBuf, ShadPs4Refusal> {
    let portable = cwd.join("user");
    match fs::symlink_metadata(&portable) {
        Ok(_) if real_directory(&portable) => Ok(portable),
        Ok(_) => Err(refuse(
            ShadPs4RefusalKind::UnsafePath,
            "portable user path is unsafe",
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(data.join("shadPS4")),
        Err(e) => Err(config_io(e)),
    }
}
/// The caller may choose cwd to bind an existing portable profile. Otherwise
/// use the executable's parent; XDG data is supplied explicitly, not guessed.
pub fn inspect_shadps4_profile(
    candidate: &ShadPs4DiscoveryCandidate,
    working_directory: &Path,
    user_data: &Path,
) -> Result<ShadPs4Profile, ShadPs4Refusal> {
    let executable = &candidate.executable;
    if !executable.exists() {
        return Err(refuse(
            ShadPs4RefusalKind::ExecutableMissing,
            "shadPS4 executable is missing",
        ));
    }
    executable_identity(executable).map_err(|_| {
        refuse(
            ShadPs4RefusalKind::ExecutableNotRunnable,
            "executable must be an absolute regular runnable file; leaf symlinks are refused",
        )
    })?;
    let executable_binding = ExecutableBinding::capture(executable).map_err(|_| {
        refuse(
            ShadPs4RefusalKind::ExecutableNotRunnable,
            "could not bind executable",
        )
    })?;
    let executable_file = BoundFile::capture(executable, MAX_BINARY_BYTES)?;
    let mut file = open_file(executable)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header).map_err(|_| {
        refuse(
            ShadPs4RefusalKind::ExecutableNotRunnable,
            "truncated ELF header",
        )
    })?;
    if &header[..7] != b"\x7fELF\x02\x01\x01" || u16::from_le_bytes([header[18], header[19]]) != 62
    {
        return Err(refuse(
            ShadPs4RefusalKind::ExecutableNotRunnable,
            "V1 requires a Linux x86-64 ELF executable or type-2 AppImage, not a shell wrapper",
        ));
    }
    let phoff = u64::from_le_bytes(header[32..40].try_into().unwrap());
    let phnum = u16::from_le_bytes(header[56..58].try_into().unwrap()) as u64;
    if !matches!(
        u16::from_le_bytes(header[16..18].try_into().unwrap()),
        2 | 3
    ) || u32::from_le_bytes(header[20..24].try_into().unwrap()) != 1
        || u16::from_le_bytes(header[52..54].try_into().unwrap()) != 64
        || u16::from_le_bytes(header[54..56].try_into().unwrap()) != 56
        || phnum == 0
        || phnum > 4096
        || phoff < 64
        || !phoff
            .checked_add(phnum * 56)
            .is_some_and(|end| end <= executable_file.identity.size)
    {
        return Err(refuse(
            ShadPs4RefusalKind::ExecutableNotRunnable,
            "invalid Linux ELF executable header",
        ));
    }
    let appimage = &header[8..11] == b"AI\x02";
    let evidence = if appimage && candidate.user_selected {
        ShadPs4ExecutableEvidence::UserSelectedAppImage
    } else if appimage {
        return Err(refuse(
            ShadPs4RefusalKind::ExecutableIdentityUnconfirmed,
            "AppImage identity cannot be proved from its name; explicitly select the trusted shadPS4 core AppImage",
        ));
    } else {
        file.seek(SeekFrom::Start(0)).map_err(config_io)?;
        let mut buffer = [0; 65536];
        let mut tail = Vec::new();
        let mut found = false;
        let mut remaining = executable_file.identity.size;
        while remaining > 0 {
            let n = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..n]).map_err(config_io)?;
            tail.extend_from_slice(&buffer[..n]);
            found |= tail.windows(CLI_MARKER.len()).any(|w| w == CLI_MARKER);
            tail.drain(..tail.len().saturating_sub(CLI_MARKER.len() - 1));
            remaining -= n as u64;
        }
        if !found {
            return Err(refuse(
                ShadPs4RefusalKind::ExecutableIdentityUnconfirmed,
                "no static shadPS4 core CLI marker; a filename is not emulator identity",
            ));
        }
        ShadPs4ExecutableEvidence::NativeCliMarker
    };
    if !real_directory(working_directory) || !normal_absolute(user_data) {
        return Err(refuse(
            ShadPs4RefusalKind::UnsafePath,
            "profile working/data roots must be absolute and safe",
        ));
    }
    let user_directory = user_root(working_directory, user_data)?;
    if !real_directory(&user_directory) {
        return Err(refuse(
            ShadPs4RefusalKind::ConfigurationUnavailable,
            "existing shadPS4 user directory is unavailable",
        ));
    }
    let json = user_directory.join("config.json");
    let (config_path, config_format) = match fs::symlink_metadata(&json) {
        Ok(_) => (json, ShadPs4ConfigFormat::Json),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            user_directory.join("config.toml"),
            ShadPs4ConfigFormat::LegacyToml,
        ),
        Err(e) => return Err(config_io(e)),
    };
    let config_file = BoundFile::capture(&config_path, MAX_CONFIG_BYTES)?;
    let bytes = config_file.bytes(MAX_CONFIG_BYTES)?;
    let config: serde_json::Value = match config_format {
        ShadPs4ConfigFormat::Json => serde_json::from_slice(&bytes)
            .map_err(|e| refuse(ShadPs4RefusalKind::ConfigurationUnavailable, e.to_string()))?,
        ShadPs4ConfigFormat::LegacyToml => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|e| refuse(ShadPs4RefusalKind::ConfigurationUnavailable, e.to_string()))?;
            let value: toml::Value = toml::from_str(text)
                .map_err(|e| refuse(ShadPs4RefusalKind::ConfigurationUnavailable, e.to_string()))?;
            serde_json::to_value(value)
                .map_err(|e| refuse(ShadPs4RefusalKind::ConfigurationUnavailable, e.to_string()))?
        }
    };
    if !config.is_object() {
        return Err(refuse(
            ShadPs4RefusalKind::ConfigurationUnavailable,
            "configuration must be a table/object",
        ));
    }
    for section in ["General", "GPU", "Log", "Debug", "Input", "Audio", "Vulkan"] {
        if config.get(section).is_some_and(|value| !value.is_object()) {
            return Err(refuse(
                ShadPs4RefusalKind::ConfigurationUnavailable,
                format!("{section} configuration section must be a table/object"),
            ));
        }
    }
    let (modules_key, fullscreen_key) = match config_format {
        ShadPs4ConfigFormat::Json => ("sys_modules_dir", "full_screen"),
        ShadPs4ConfigFormat::LegacyToml => ("sysModulesPath", "Fullscreen"),
    };
    let configured_modules = config.get("General").and_then(|v| v.get(modules_key));
    let sysmodules_directory = match configured_modules {
        None => user_directory.join("sys_modules"),
        Some(serde_json::Value::String(s)) if s.is_empty() => user_directory.join("sys_modules"),
        Some(serde_json::Value::String(s)) => {
            let p = PathBuf::from(s);
            if !normal_absolute(&p) {
                return Err(refuse(
                    ShadPs4RefusalKind::ConfigurationUnavailable,
                    "relative sysmodule paths are ambiguous and unsupported in V1",
                ));
            }
            p
        }
        _ => {
            return Err(refuse(
                ShadPs4RefusalKind::ConfigurationUnavailable,
                "sysmodule setting must be a string",
            ));
        }
    };
    let fullscreen = match config.get("GPU").and_then(|v| v.get(fullscreen_key)) {
        None => None,
        Some(v) => Some(v.as_bool().ok_or_else(|| {
            refuse(
                ShadPs4RefusalKind::ConfigurationUnavailable,
                "fullscreen setting must be boolean",
            )
        })?),
    };
    let sysmodules = if real_directory(&sysmodules_directory) {
        ShadPs4SysmoduleState::PresentUnverified
    } else {
        ShadPs4SysmoduleState::DirectoryMissing
    };
    if !executable_binding.is_unchanged(executable) || !executable_file.unchanged() {
        return Err(refuse(
            ShadPs4RefusalKind::ChangedAfterPreview,
            "executable changed during profile inspection",
        ));
    }
    Ok(ShadPs4Profile {
        profile_id: format!("shadps4:{}", executable.display()),
        executable: executable.clone(),
        installation: if appimage {
            LaunchInstallation::AppImage {
                extract_and_run: false,
            }
        } else {
            LaunchInstallation::Native
        },
        executable_evidence: evidence,
        version: None,
        working_directory: working_directory.into(),
        config_root: user_directory.clone(),
        user_directory,
        config_path,
        config_format,
        settings: ShadPs4Settings {
            fullscreen,
            sysmodules_directory,
        },
        sysmodules,
        executable_binding,
        executable_file,
        config_file,
        xdg_data: user_data.into(),
    })
}
pub(crate) fn profile_is_fresh(profile: &ShadPs4Profile) -> bool {
    inspect_shadps4_profile(
        &ShadPs4DiscoveryCandidate {
            executable: profile.executable.clone(),
            user_selected: profile.executable_evidence
                == ShadPs4ExecutableEvidence::UserSelectedAppImage,
        },
        &profile.working_directory,
        &profile.xdg_data,
    )
    .is_ok_and(|fresh| fresh == *profile)
}
