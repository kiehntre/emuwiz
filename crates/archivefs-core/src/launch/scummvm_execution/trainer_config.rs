//! Read-only validation of the exact target that ScummVM will load.
//!
//! This is a deliberately strict native-config gate, not a generic INI editor.
//! It leaves savepath and trainer options untouched, and refuses evidence that
//! ScummVM could discard, merge, or interpret as a different launch binding.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path};

use super::{ScummVmLaunchPreflightError, ScummVmLaunchPreflightErrorKind, error};
use crate::launch::scummvm_command::ScummVmTrainerLaunchBinding;
use crate::scummvm_detection::is_valid_scummvm_game_id;

// Same ceiling as the existing native trainer renderer; its private adapter
// constant is intentionally not exposed through the shared cheat API.
pub(super) const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 8192;

fn invalid(detail: impl Into<String>) -> ScummVmLaunchPreflightError {
    error(
        ScummVmLaunchPreflightErrorKind::TrainerConfigurationInvalid,
        detail,
    )
}

pub(super) fn validate(
    trainer: &ScummVmTrainerLaunchBinding,
    verified_id: &str,
    folder: &Path,
) -> Result<(), ScummVmLaunchPreflightError> {
    if !is_valid_scummvm_game_id(verified_id)
        || !safe_path(&trainer.configuration)
        || !safe_path(folder)
        || trainer.target_name.len() > 96
        || !identifier(&trainer.target_name)
        || matches!(
            trainer.target_name.to_ascii_lowercase().as_str(),
            "scummvm" | "keymapper" | "cloud"
        )
    {
        return Err(invalid("unsafe ScummVM trainer target, identity or path"));
    }
    let file = open_configuration(&trainer.configuration).map_err(|e| {
        invalid(format!(
            "trainer configuration is unavailable or unsafe: {e}"
        ))
    })?;
    let metadata = file.metadata().map_err(|e| invalid(e.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES as u64 {
        return Err(invalid(
            "trainer configuration is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| invalid(e.to_string()))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(invalid("trainer configuration exceeded the read bound"));
    }
    let text =
        std::str::from_utf8(&bytes).map_err(|_| invalid("trainer configuration is not UTF-8"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut sections = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut current = String::new();
    let mut target = BTreeMap::new();
    let mut application = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let malformed = || {
            invalid(format!(
                "malformed or ambiguous trainer configuration at line {}",
                index + 1
            ))
        };
        if line.len() > MAX_LINE_BYTES || line.chars().any(|c| c.is_control() && c != '\t') {
            return Err(malformed());
        }
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let section = line
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .filter(|s| identifier(s))
                .ok_or_else(malformed)?;
            if !sections.insert(section.to_ascii_lowercase()) {
                return Err(malformed());
            }
            current = section.into();
            keys.clear();
        } else {
            // ScummVM's native parser recognizes only column-zero # comments
            // and section headers. Do not accept a more permissive INI dialect.
            let (key, value) = line.trim_start().split_once('=').ok_or_else(malformed)?;
            let key = key.trim().to_ascii_lowercase();
            if current.is_empty() || !identifier(&key) || !keys.insert(key.clone()) {
                return Err(malformed());
            }
            if current == trainer.target_name {
                target.insert(key, value.trim());
            } else if current.eq_ignore_ascii_case("scummvm") {
                application.insert(key, value.trim());
            }
        }
    }
    let (engine, game) = verified_id
        .split_once(':')
        .ok_or_else(|| invalid("invalid qualified identity"))?;
    if target.get("engineid").copied() != Some(engine)
        || target.get("gameid").copied() != Some(game)
        || target.get("path").map(|s| Path::new(s)) != Some(folder)
    {
        return Err(invalid(
            "trainer target engineid, gameid or path does not match the verified game; regenerate the owned target configuration",
        ));
    }
    // An alternate config does not inherit the user's normal global savepath.
    // Require an explicit binding rather than silently choosing a new default.
    let savepath = target
        .get("savepath")
        .or_else(|| application.get("savepath"));
    if !savepath.is_some_and(|value| safe_path(Path::new(value))) {
        return Err(invalid(
            "trainer configuration requires an explicit absolute target or global savepath preserving the user's selected saves",
        ));
    }
    Ok(())
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn safe_path(path: &Path) -> bool {
    path.is_absolute()
        && path.as_os_str().len() <= 4096
        && !path.components().any(|c| c == Component::ParentDir)
}

/// Component-by-component no-follow opens prevent a parent symlink from
/// redirecting validation. Nonblocking leaf open avoids waiting on a FIFO.
#[cfg(target_os = "linux")]
fn open_configuration(path: &Path) -> io::Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let mut parent = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")?;
    let components: Vec<_> = path
        .components()
        .filter(|c| !matches!(c, Component::RootDir | Component::CurDir))
        .collect();
    for (i, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        };
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if i + 1 == components.len() {
                0
            } else {
                libc::O_DIRECTORY
            };
        // SAFETY: parent owns a live fd, name is NUL-terminated, and openat
        // returns a fresh fd or -1; ownership is taken only after checking it.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: this successful openat result is owned exactly once.
        parent = unsafe { File::from_raw_fd(fd) };
    }
    Ok(parent)
}

#[cfg(not(target_os = "linux"))]
fn open_configuration(_path: &Path) -> io::Result<File> {
    // The committed native launch contract is Linux. Extend it only after an
    // equivalent no-follow reader and runtime target binding are proven.
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "trainer config validation currently requires Linux",
    ))
}
