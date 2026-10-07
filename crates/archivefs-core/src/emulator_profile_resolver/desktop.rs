//! Read-only `.desktop` launcher association: which executable does the
//! user's launcher for an emulator actually start?
//!
//! Only launchers whose file name contains the emulator's name are read, in a
//! fixed list of directories; nothing is crawled.

use std::fs;
use std::path::{Path, PathBuf};

const MAX_DESKTOP_BYTES: u64 = 64 * 1024;
const MAX_ENTRIES_PER_DIRECTORY: usize = 4_000;

/// The directories that hold user and system launchers.
pub fn default_desktop_directories(home: &Path, xdg_data_home: &Path) -> Vec<PathBuf> {
    vec![
        xdg_data_home.join("applications"),
        home.join(".local/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/usr/share/applications"),
        home.join("Desktop"),
    ]
}

/// Executable paths named by the `Exec=` lines of launchers matching `needle`.
pub fn launcher_targets(directories: &[PathBuf], needle: &str) -> Vec<PathBuf> {
    let needle = needle.to_ascii_lowercase();
    let mut targets = Vec::new();
    for directory in directories {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.take(MAX_ENTRIES_PER_DIRECTORY).flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !name.ends_with(".desktop") || !name.contains(&needle) {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > MAX_DESKTOP_BYTES {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            targets.extend(exec_target(&text));
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

/// The program of the first `Exec=` line inside `[Desktop Entry]`.
fn exec_target(text: &str) -> Option<PathBuf> {
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some(value) = line.strip_prefix("Exec=") else {
            continue;
        };
        let value = value.trim();
        let program = if let Some(rest) = value.strip_prefix('"') {
            rest.split('"').next()?
        } else {
            value.split_whitespace().next()?
        };
        let path = PathBuf::from(program);
        return path.is_absolute().then_some(path);
    }
    None
}
