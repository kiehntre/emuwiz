//! Read-only health of RetroArch's save-file and save-state folders.
//!
//! RetroArch silently ignores a configured `savefile_directory` /
//! `savestate_directory` that is not an existing folder and writes beside the
//! game file instead; a folder it cannot write to loses saves outright. This
//! module records, for `retroarch.cfg` and for each installed core's per-core
//! override (`<rgui_config_directory or config/>/<core>/<core>.cfg`), whether
//! the configured folder is usable. It only probes and reads; it never
//! creates, changes or follows anything.

use super::{
    BoundedReadResult, EncodedPath, FsProbe, MAX_CONFIG_BYTES, PathPurpose, ReadOnlyHostFilesystem,
    resolve_configured_value,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The two keys this module checks, with their purpose.
pub(super) const SAVE_DIRECTORY_KEYS: [(PathPurpose, &str); 2] = [
    (PathPurpose::Saves, "savefile_directory"),
    (PathPurpose::SaveStates, "savestate_directory"),
];
/// `retroarch.cfg` key naming where override files live.
pub(super) const OVERRIDE_DIRECTORY_KEY: &str = "rgui_config_directory";
/// Bound on per-core override files examined for one profile.
const MAX_CORE_OVERRIDES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveDirectoryStatus {
    /// An existing, real folder that was not shown to be unwritable.
    Usable,
    /// Nothing exists at the configured path.
    Missing,
    /// Something other than a folder exists at the configured path.
    NotADirectory,
    /// The configured path is a symbolic link (never followed).
    Symlink,
    /// The folder exists but this user cannot write into it.
    NotWritable,
    /// The setting or folder could not be examined (unreadable override
    /// file, unresolvable value, permission or I/O error).
    Uncheckable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveDirectoryFinding {
    pub purpose: PathPurpose,
    pub config_key: &'static str,
    /// `None` for `retroarch.cfg`; the core's `corename` for its override.
    pub core_name: Option<String>,
    /// The settings file the value came from.
    pub settings_file: EncodedPath,
    /// The resolved folder, when the value could be resolved.
    pub directory: Option<EncodedPath>,
    pub status: SaveDirectoryStatus,
}

fn classify(filesystem: &dyn ReadOnlyHostFilesystem, path: &Path) -> SaveDirectoryStatus {
    match filesystem.probe(path) {
        FsProbe::PresentDirectory => match filesystem.directory_writable(path) {
            Some(false) => SaveDirectoryStatus::NotWritable,
            _ => SaveDirectoryStatus::Usable,
        },
        FsProbe::Missing => SaveDirectoryStatus::Missing,
        FsProbe::PresentFile | FsProbe::WrongType => SaveDirectoryStatus::NotADirectory,
        FsProbe::Symlink => SaveDirectoryStatus::Symlink,
        FsProbe::Inaccessible | FsProbe::IoError => SaveDirectoryStatus::Uncheckable,
    }
}

fn finding(
    filesystem: &dyn ReadOnlyHostFilesystem,
    purpose: PathPurpose,
    config_key: &'static str,
    core_name: Option<&str>,
    settings_file: &Path,
    raw: &str,
    tilde_home: &Path,
) -> SaveDirectoryFinding {
    let resolved = resolve_configured_value(raw, tilde_home).filter(|path| path.is_absolute());
    SaveDirectoryFinding {
        purpose,
        config_key,
        core_name: core_name.map(str::to_string),
        settings_file: EncodedPath::from_path(settings_file),
        status: resolved
            .as_deref()
            .map_or(SaveDirectoryStatus::Uncheckable, |path| {
                classify(filesystem, path)
            }),
        directory: resolved.as_deref().map(EncodedPath::from_path),
    }
}

/// A `corename` that is safe to use as one path component.
fn safe_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.chars().any(char::is_control)
}

/// Findings for `retroarch.cfg` (`global`: the raw configured values) and for
/// every distinct core name's per-core override file.
pub(super) fn discover_save_directories<'a>(
    filesystem: &dyn ReadOnlyHostFilesystem,
    config_file: &Path,
    global: impl Fn(&str) -> Option<&'a str>,
    tilde_home: &Path,
    core_names: impl IntoIterator<Item = &'a str>,
) -> Vec<SaveDirectoryFinding> {
    let mut findings = Vec::new();
    for (purpose, key) in SAVE_DIRECTORY_KEYS {
        if let Some(raw) = global(key).filter(|raw| !raw.is_empty()) {
            findings.push(finding(
                filesystem,
                purpose,
                key,
                None,
                config_file,
                raw,
                tilde_home,
            ));
        }
    }
    let override_root: Option<PathBuf> = match global(OVERRIDE_DIRECTORY_KEY) {
        Some(raw) if !raw.is_empty() => {
            resolve_configured_value(raw, tilde_home).filter(|path| path.is_absolute())
        }
        _ => config_file.parent().map(|parent| parent.join("config")),
    };
    let Some(override_root) = override_root else {
        return findings;
    };
    let names: BTreeSet<&str> = core_names
        .into_iter()
        .filter(|n| safe_component(n))
        .collect();
    for name in names.into_iter().take(MAX_CORE_OVERRIDES) {
        let file = override_root.join(name).join(format!("{name}.cfg"));
        let keys: Vec<&'static str> = SAVE_DIRECTORY_KEYS.iter().map(|(_, key)| *key).collect();
        match filesystem.read_bounded(&file, MAX_CONFIG_BYTES) {
            BoundedReadResult::NotFound => {}
            BoundedReadResult::Ok(bytes) => {
                let Ok(text) = String::from_utf8(bytes) else {
                    findings.extend(uncheckable(name, &file));
                    continue;
                };
                let parsed = super::parse_config(&text, &keys);
                for (purpose, key) in SAVE_DIRECTORY_KEYS {
                    if let Some(raw) = parsed.values.get(key).filter(|raw| !raw.is_empty()) {
                        findings.push(finding(
                            filesystem,
                            purpose,
                            key,
                            Some(name),
                            &file,
                            raw,
                            tilde_home,
                        ));
                    }
                }
            }
            _ => findings.extend(uncheckable(name, &file)),
        }
    }
    findings
}

fn uncheckable(name: &str, file: &Path) -> impl Iterator<Item = SaveDirectoryFinding> {
    let name = name.to_string();
    let file = EncodedPath::from_path(file);
    SAVE_DIRECTORY_KEYS
        .into_iter()
        .map(move |(purpose, config_key)| SaveDirectoryFinding {
            purpose,
            config_key,
            core_name: Some(name.clone()),
            settings_file: file.clone(),
            directory: None,
            status: SaveDirectoryStatus::Uncheckable,
        })
}

#[cfg(test)]
mod tests;
