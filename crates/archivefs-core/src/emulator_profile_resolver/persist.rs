//! The remembered AUTO / PREFERRED / FORCED choice.
//!
//! A small TOML file beside the existing remembered-profile file, written with
//! the same atomic primitive. Remembering a choice stores *which pair* the user
//! chose; whether that pair still exists is decided afresh by the resolver at
//! every resolution, so a moved AppImage or a changed profile root makes the
//! choice stale (bypassed for PREFERRED, refused for FORCED) instead of being
//! trusted.

use std::fs;
use std::path::{Path, PathBuf};

use super::model::{CandidateIdentity, EmulatorSelectionMode};
use crate::emulator_inventory::InventoryEmulator;
use crate::patch_manager::emulator_profile_memory::{quote, unquote};
use crate::{ArchiveFsError, Result};

const HEADER: &str = "[[emulator_selection]]";

pub fn default_selection_path() -> Result<PathBuf> {
    crate::app_dirs::config_path("emulator_selection.toml")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSelection {
    pub emulator: String,
    pub mode: String,
    pub executable: Option<PathBuf>,
    pub profile_root: Option<PathBuf>,
}

/// The remembered mode for `emulator`; `Auto` when nothing is remembered.
pub fn load_selection_from(
    path: impl AsRef<Path>,
    emulator: InventoryEmulator,
) -> Result<EmulatorSelectionMode> {
    let stored = read_all(path.as_ref())?;
    let Some(entry) = stored
        .into_iter()
        .find(|entry| entry.emulator == emulator.label())
    else {
        return Ok(EmulatorSelectionMode::Auto);
    };
    let identity = || {
        entry
            .profile_root
            .clone()
            .map(|profile_root| CandidateIdentity {
                emulator,
                executable: entry.executable.clone(),
                profile_root,
            })
    };
    Ok(match (entry.mode.as_str(), identity()) {
        ("preferred", Some(identity)) => EmulatorSelectionMode::Preferred(identity),
        ("forced", Some(identity)) => EmulatorSelectionMode::Forced(identity),
        ("auto", _) => EmulatorSelectionMode::Auto,
        (other, _) => {
            return Err(ArchiveFsError::Config(format!(
                "the remembered {} selection '{other}' is incomplete or unknown",
                emulator.label()
            )));
        }
    })
}

/// Remembers `mode` for `emulator`, replacing only that emulator's entry.
pub fn remember_selection_to(
    path: impl AsRef<Path>,
    emulator: InventoryEmulator,
    mode: &EmulatorSelectionMode,
) -> Result<()> {
    let path = path.as_ref();
    let mut all = read_all(path)?;
    all.retain(|entry| entry.emulator != emulator.label());
    let (kind, identity) = match mode {
        EmulatorSelectionMode::Auto => ("auto", None),
        EmulatorSelectionMode::Preferred(identity) => ("preferred", Some(identity)),
        EmulatorSelectionMode::Forced(identity) => ("forced", Some(identity)),
    };
    for path in identity.iter().flat_map(|identity| {
        identity
            .executable
            .iter()
            .chain(Some(&identity.profile_root))
    }) {
        if path.to_str().is_none() {
            return Err(ArchiveFsError::Config(format!(
                "path cannot be stored losslessly in the UTF-8 configuration file: {}",
                path.display()
            )));
        }
    }
    all.push(StoredSelection {
        emulator: emulator.label().to_string(),
        mode: kind.to_string(),
        executable: identity.and_then(|identity| identity.executable.clone()),
        profile_root: identity.map(|identity| identity.profile_root.clone()),
    });
    crate::atomic_write_text(path, &render(&all))
}

/// Forgets the remembered choice (back to AUTO). Other emulators are untouched.
pub fn clear_selection_at(path: impl AsRef<Path>, emulator: InventoryEmulator) -> Result<()> {
    let path = path.as_ref();
    let mut all = read_all(path)?;
    let before = all.len();
    all.retain(|entry| entry.emulator != emulator.label());
    if all.len() == before {
        return Ok(());
    }
    crate::atomic_write_text(path, &render(&all))
}

fn read_all(path: &Path) -> Result<Vec<StoredSelection>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ArchiveFsError::io(path.to_path_buf(), error)),
    };
    let mut entries = Vec::new();
    let mut current: Option<StoredSelection> = None;
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == HEADER {
            entries.extend(current.take());
            current = Some(StoredSelection {
                emulator: String::new(),
                mode: String::new(),
                executable: None,
                profile_root: None,
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            return Err(ArchiveFsError::Config(format!(
                "line {line_number} is not a key/value pair"
            )));
        };
        let value = unquote(value.trim(), line_number)?;
        match key.trim() {
            "emulator" => entry.emulator = value,
            "mode" => entry.mode = value,
            "executable" => entry.executable = Some(PathBuf::from(value)),
            "profile_root" => entry.profile_root = Some(PathBuf::from(value)),
            _ => {}
        }
    }
    entries.extend(current);
    Ok(entries)
}

fn render(entries: &[StoredSelection]) -> String {
    let mut out = String::from("# EmuWiz emulator selection (AUTO / PREFERRED / FORCED)\n");
    for entry in entries {
        out.push('\n');
        out.push_str(HEADER);
        out.push('\n');
        out.push_str(&format!("emulator = {}\n", quote(&entry.emulator)));
        out.push_str(&format!("mode = {}\n", quote(&entry.mode)));
        if let Some(executable) = &entry.executable {
            out.push_str(&format!(
                "executable = {}\n",
                quote(&executable.display().to_string())
            ));
        }
        if let Some(root) = &entry.profile_root {
            out.push_str(&format!(
                "profile_root = {}\n",
                quote(&root.display().to_string())
            ));
        }
    }
    out
}
