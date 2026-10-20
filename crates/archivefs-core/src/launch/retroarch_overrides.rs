//! Read-only discovery of the RetroArch override files a cheat launch could
//! be affected by.
//!
//! RetroArch applies, in order, a per-core override
//! (`<config>/<core library name>/<core library name>.cfg`), a per-content-
//! directory override (`.../<content folder name>.cfg`) and a per-game override
//! (`.../<content file stem>.cfg`) on top of `retroarch.cfg`. A cheat launch
//! pins a few settings (see [`RETROARCH_MANDATORY_KEYS`]) in its own generated
//! profile and turns automatic overrides off, but the planner also refuses
//! when an override that exists would set one of those keys, because then the
//! launch's behaviour no longer matches what was planned.
//!
//! This module only looks. It never follows a link, opens nothing but regular
//! files (no-follow, non-blocking, size-bounded), and treats anything it cannot
//! read safely as a reason to refuse cheat launching rather than guessing.

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::cheat_launch_plan::{RETROARCH_MANDATORY_KEYS, RetroArchOverrideFinding};
use super::process_spawn::open_regular_file_nonblocking;
use crate::patch_manager::is_safe_cheat_name_part;

const MAX_OVERRIDE_BYTES: u64 = 256 * 1024;

/// An override location that could not be examined safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiableOverride {
    pub path: PathBuf,
    pub reason: String,
}

/// The settings file that was read, bound so it can be re-checked before launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigBinding {
    /// RetroArch's settings file (always a regular file; a link is refused).
    pub path: PathBuf,
    identity: FileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl FileIdentity {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

impl ConfigBinding {
    /// The settings file must still be the very regular file that was read.
    /// A path swapped for a link, a pipe or another file is refused. Call this
    /// immediately before launching.
    pub fn ensure_unchanged(&self) -> Result<(), String> {
        match open_regular_file_nonblocking(&self.path).and_then(|f| f.metadata()) {
            Ok(metadata) if FileIdentity::of(&metadata) == self.identity => Ok(()),
            _ => Err(format!(
                "RetroArch's settings file {} changed since it was checked, so the cheat launch was not started. Try again.",
                self.path.display()
            )),
        }
    }
}

/// The one explanation used wherever a symlinked settings file stops a cheat
/// launch (override checking, the launch worker and profile construction all
/// refuse links the same way).
#[must_use]
pub fn symlinked_settings_message(path: &Path) -> String {
    format!(
        "RetroArch's settings file {} is a symbolic link, and EmuWiz does not follow symbolic links for RetroArch's settings (so it cannot launch RetroArch from here, with or without cheats). Replace the link with a regular copy of the file, or launch RetroArch yourself.",
        path.display()
    )
}

/// What the override files for one core and one game say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverrideDiscovery {
    /// Existing override files that set a setting the cheat launch pins.
    pub findings: Vec<RetroArchOverrideFinding>,
    /// Locations that exist but could not be read safely (a link, a non-regular
    /// file, oversized, unreadable). Cheat launching must be refused while any
    /// remain: their contents are unknown.
    pub unverifiable: Vec<UnverifiableOverride>,
    /// RetroArch's own settings file could not be used safely (for example a
    /// link that is broken, loops, or leaves the home folder). Reported
    /// separately so the explanation never blames an override file.
    pub configuration_problem: Option<String>,
    /// The settings file that was read, for the pre-launch recheck.
    pub configuration: Option<ConfigBinding>,
}

impl OverrideDiscovery {
    /// Plain-language reason cheat launching is unavailable, if it is.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        if let Some(problem) = &self.configuration_problem {
            return Some(problem.clone());
        }
        if let Some(bad) = self.unverifiable.first() {
            return Some(format!(
                "RetroArch override {} could not be checked safely ({}). Fix or remove it, or launch without cheats.",
                bad.path.display(),
                bad.reason
            ));
        }
        None
    }
}

/// Finds the override files for `core_library_name` and `content_path` under
/// the configuration folder implied by `real_config_path` (RetroArch's
/// `retroarch.cfg`), or by its `rgui_config_directory` when it sets one.
///
/// `home` is the user's home folder; it expands a leading `~` in
/// `rgui_config_directory` the way RetroArch does. A symlinked settings file is
/// refused (the shared RetroArch discovery does not follow one either).
#[must_use]
pub fn discover_retroarch_overrides(
    real_config_path: &Path,
    core_library_name: &str,
    content_path: &Path,
    home: Option<&Path>,
) -> OverrideDiscovery {
    let mut discovery = OverrideDiscovery::default();
    if !is_safe_cheat_name_part(core_library_name) {
        discovery.configuration_problem =
            Some("The core's library name is not usable, so cheats are unavailable for it.".into());
        return discovery;
    }
    // RetroArch's settings file, resolved safely if it is a managed link.
    let (text, binding) = match read_configuration(real_config_path) {
        Ok(Some(found)) => (found.0, Some(found.1)),
        Ok(None) => (String::new(), None), // no settings file: RetroArch defaults
        Err(problem) => {
            discovery.configuration_problem = Some(problem);
            return discovery;
        }
    };
    discovery.configuration = binding;
    // Where RetroArch keeps override files (the last assignment wins).
    let configured = text
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == "rgui_config_directory")
                .then(|| value.trim().trim_matches('"').to_string())
        })
        .last();
    let root = match configured.as_deref() {
        None | Some("") | Some("default") => real_config_path
            .parent()
            .map(|parent| parent.join("config")),
        Some(value) => match expand_directory(value, home) {
            Ok(path) => Some(path),
            Err(reason) => {
                discovery.configuration_problem = Some(format!(
                    "RetroArch's settings set rgui_config_directory to {value:?}, which EmuWiz cannot use ({reason}), so cheats are unavailable. The game still launches normally."
                ));
                return discovery;
            }
        },
    };
    let Some(root) = root else {
        discovery.configuration_problem =
            Some("No RetroArch configuration folder could be derived.".into());
        return discovery;
    };
    let mut unverifiable = |path: &Path, reason: &str| {
        discovery.unverifiable.push(UnverifiableOverride {
            path: path.to_path_buf(),
            reason: reason.to_string(),
        });
    };

    // The override folders must be real directories if they exist at all.
    match real_directory(&root) {
        Ok(false) => return discovery, // no overrides folder: nothing applies
        Ok(true) => {}
        Err(reason) => {
            unverifiable(&root, &reason);
            return discovery;
        }
    }
    let core_dir = root.join(core_library_name);
    match real_directory(&core_dir) {
        Ok(false) => return discovery,
        Ok(true) => {}
        Err(reason) => {
            unverifiable(&core_dir, &reason);
            return discovery;
        }
    }

    // Per-core, per-content-folder and per-game override names.
    let content_folder = content_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    let content_stem = content_path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.rsplit_once('.').map(|(stem, _)| stem).or(Some(n)));
    let mut names = vec![core_library_name.to_string()];
    names.extend(content_folder.map(str::to_string));
    names.extend(content_stem.map(str::to_string));
    names.sort();
    names.dedup();
    for name in names {
        if !is_safe_cheat_name_part(&name) {
            continue; // cannot name a RetroArch override file; nothing to read
        }
        let path = core_dir.join(format!("{name}.cfg"));
        match examine(&path) {
            Ok(None) => {}
            Ok(Some(text)) => {
                let keys: Vec<String> = RETROARCH_MANDATORY_KEYS
                    .iter()
                    .filter(|key| sets_key(&text, key))
                    .map(|key| (*key).to_string())
                    .collect();
                if !keys.is_empty() {
                    discovery
                        .findings
                        .push(RetroArchOverrideFinding { path, keys });
                }
            }
            Err(reason) => discovery
                .unverifiable
                .push(UnverifiableOverride { path, reason }),
        }
    }
    discovery
}

/// Expands RetroArch's `~` for a directory value. Other shortcuts (`:` is the
/// application folder) and `~user` forms are not resolvable and are refused.
fn expand_directory(value: &str, home: Option<&Path>) -> Result<PathBuf, &'static str> {
    if let Some(rest) = value.strip_prefix('~') {
        let home = home
            .filter(|h| h.is_absolute())
            .ok_or("the home folder is unknown")?;
        return match rest {
            "" => Ok(home.to_path_buf()),
            _ => match rest.strip_prefix('/') {
                Some(tail) => Ok(home.join(tail)),
                None => Err("a ~user shortcut"),
            },
        };
    }
    if value.starts_with(':') {
        return Err("a shortcut for RetroArch's own folder");
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err("a relative path")
    }
}

/// `Ok(true)` a real directory, `Ok(false)` absent, `Err` anything else.
fn real_directory(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(metadata) if metadata.file_type().is_symlink() => Err("it is a link".into()),
        Ok(_) => Err("it is not a folder".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// Reads a regular override file: `Ok(None)` if absent, the text otherwise.
fn examine(path: &Path) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    }
    read_regular_text(path).map(Some)
}

fn read_regular_text(path: &Path) -> Result<String, String> {
    let file = open_regular_file_nonblocking(path)
        .map_err(|e| format!("not a readable regular file: {e}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_OVERRIDE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_OVERRIDE_BYTES {
        return Err("larger than an override file can be".into());
    }
    String::from_utf8(bytes).map_err(|_| "not text".to_string())
}

/// Reads RetroArch's settings file. Only a regular file is read, opened without
/// following links and without blocking, and checked to be the file that was
/// inspected. A link is refused (see [`symlinked_settings_message`]): following
/// one safely needs the shared RetroArch discovery to do so too, and a cheat
/// launch must never run with a different policy from the launch it builds on.
/// `Ok(None)` means there is no file.
fn read_configuration(path: &Path) -> Result<Option<(String, ConfigBinding)>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "RetroArch's settings file {} could not be inspected ({e}), so cheats are unavailable.",
                path.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(symlinked_settings_message(path));
    }
    if !metadata.is_file() {
        return Err(format!(
            "RetroArch's settings file {} is not a regular file, so cheats are unavailable.",
            path.display()
        ));
    }
    let file = open_regular_file_nonblocking(path).map_err(|e| {
        format!(
            "RetroArch's settings file {} could not be opened safely ({e}), so cheats are unavailable.",
            path.display()
        )
    })?;
    let opened = file.metadata().map_err(|e| {
        format!(
            "RetroArch's settings file {} could not be read ({e}).",
            path.display()
        )
    })?;
    if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
        return Err(format!(
            "RetroArch's settings file {} changed while it was being opened, so cheats are unavailable.",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_OVERRIDE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            format!(
                "RetroArch's settings file {} could not be read ({e}).",
                path.display()
            )
        })?;
    if bytes.len() as u64 > MAX_OVERRIDE_BYTES {
        return Err(format!(
            "RetroArch's settings file {} is larger than EmuWiz reads, so cheats are unavailable.",
            path.display()
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        format!(
            "RetroArch's settings file {} is not text, so cheats are unavailable.",
            path.display()
        )
    })?;
    Ok(Some((
        text,
        ConfigBinding {
            path: path.to_path_buf(),
            identity: FileIdentity::of(&opened),
        },
    )))
}

fn sets_key(text: &str, key: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with('#')
            && line
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
    })
}

#[cfg(test)]
mod tests;
