//! Optional, separately installed card editors.
//!
//! myMCpp (PS2, from the PCSX2 project) and PSMemoryManager (PS1 and PS2)
//! are GPL-3.0 programs. EmuWiz does not contain, bundle or download them;
//! if the person has installed one, EmuWiz can start it as its own program.
//!
//! Neither is ever handed a card: myMCpp treats a path on its command line
//! as a command-line job rather than opening its window, and PSMemoryManager
//! opens in the browser. They are started with no arguments, and the person
//! opens a file in them, which should be a backup copy.

use super::CardError;
use serde::Serialize;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalTool {
    pub name: &'static str,
    pub executable: PathBuf,
    pub handles: &'static str,
    pub licence: &'static str,
}

struct Known {
    name: &'static str,
    handles: &'static str,
    /// Lower-case file names, or AppImage name prefixes, that are this tool.
    file_names: &'static [&'static str],
}

const KNOWN: &[Known] = &[
    Known {
        name: "myMCpp",
        handles: "PS2 cards",
        file_names: &["mymcpp"],
    },
    Known {
        name: "PSMemoryManager",
        handles: "PS1 and PS2 cards",
        file_names: &["psmemorymanager"],
    },
];

fn matches(known: &Known, file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    known
        .file_names
        .iter()
        .any(|name| lower == *name || (lower.starts_with(name) && lower.ends_with(".appimage")))
}

/// The folders a person usually installs such a program in: `PATH`, then
/// `~/Applications` and `~/.local/bin`.
pub fn default_tool_folders() -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        folders.push(home.join("Applications"));
        folders.push(home.join(".local/bin"));
    }
    folders
}

/// Installed editors found in `folders`, at most one per program (the first
/// folder wins). Only looks at file names and the executable bit.
pub fn find_external_tools_in(folders: &[PathBuf]) -> Vec<ExternalTool> {
    let mut tools: Vec<ExternalTool> = Vec::new();
    for folder in folders {
        let Ok(entries) = fs::read_dir(folder) else {
            continue;
        };
        let mut names: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        names.sort();
        for path in names {
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            for known in KNOWN {
                if tools.iter().any(|tool| tool.name == known.name) || !matches(known, file_name) {
                    continue;
                }
                if is_executable_file(&path) {
                    tools.push(ExternalTool {
                        name: known.name,
                        executable: path.clone(),
                        handles: known.handles,
                        licence: "GPL-3.0",
                    });
                }
            }
        }
    }
    tools
}

pub fn find_external_tools() -> Vec<ExternalTool> {
    find_external_tools_in(&default_tool_folders())
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Starts the program with no arguments and returns its process id. EmuWiz
/// does not wait for it and passes it no card.
pub fn launch(tool: &ExternalTool) -> Result<u32, CardError> {
    if !is_executable_file(&tool.executable) {
        return Err(CardError::Refused(format!(
            "{} is no longer an executable file",
            tool.executable.display()
        )));
    }
    let mut child = Command::new(&tool.executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| CardError::Io(format!("cannot start {}: {error}", tool.name)))?;
    let pid = child.id();
    // Reap it when it exits so it never lingers as a defunct process.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}
