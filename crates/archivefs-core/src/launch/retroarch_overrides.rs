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

/// What the override files for one core and one game say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverrideDiscovery {
    /// Existing override files that set a setting the cheat launch pins.
    pub findings: Vec<RetroArchOverrideFinding>,
    /// Locations that exist but could not be read safely (a link, a non-regular
    /// file, oversized, unreadable). Cheat launching must be refused while any
    /// remain: their contents are unknown.
    pub unverifiable: Vec<UnverifiableOverride>,
}

impl OverrideDiscovery {
    /// Plain-language reason cheat launching is unavailable, if it is.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
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
#[must_use]
pub fn discover_retroarch_overrides(
    real_config_path: &Path,
    core_library_name: &str,
    content_path: &Path,
) -> OverrideDiscovery {
    let mut discovery = OverrideDiscovery::default();
    let mut unverifiable = |path: &Path, reason: &str| {
        discovery.unverifiable.push(UnverifiableOverride {
            path: path.to_path_buf(),
            reason: reason.to_string(),
        });
    };
    if !is_safe_cheat_name_part(core_library_name) {
        unverifiable(real_config_path, "the core's library name is not usable");
        return discovery;
    }
    // Where RetroArch keeps override files.
    let configured = match read_setting(real_config_path, "rgui_config_directory") {
        Ok(value) => value,
        Err(reason) => {
            unverifiable(real_config_path, &reason);
            return discovery;
        }
    };
    let root = match configured.as_deref() {
        None | Some("") | Some("default") => real_config_path
            .parent()
            .map(|parent| parent.join("config")),
        Some(value) if value.starts_with(':') || value.starts_with('~') => {
            unverifiable(
                real_config_path,
                "rgui_config_directory uses a shortcut EmuWiz cannot resolve",
            );
            return discovery;
        }
        Some(value) => Some(PathBuf::from(value)),
    };
    let Some(root) = root else {
        unverifiable(real_config_path, "no configuration folder could be derived");
        return discovery;
    };
    if !root.is_absolute() {
        unverifiable(&root, "the override folder is not an absolute path");
        return discovery;
    }

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

/// The value of `key` in a `key = "value"` configuration file, if set.
fn read_setting(path: &Path, key: &str) -> Result<Option<String>, String> {
    let text = match std::fs::symlink_metadata(path) {
        Ok(_) => read_regular_text(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    // RetroArch honours the last assignment of a key.
    Ok(text
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim().trim_matches('"').to_string())
        })
        .last())
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
