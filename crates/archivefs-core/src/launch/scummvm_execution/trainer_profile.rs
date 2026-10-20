//! Launch-time use of an EmuWiz-owned ScummVM trainer profile.
//!
//! The owned profile is read once, hashed, and checked against the evidence the
//! user saw. ScummVM is then started on a private per-launch copy of exactly
//! those bytes plus the savepath of the user's own configuration, so ScummVM's
//! habit of rewriting its configuration never touches the owned profile or the
//! user's `scummvm.ini`, and saves keep going where the user already keeps
//! them. No trainer engine lives here: option names come from the existing
//! documented projection in `patch_manager::scummvm_trainer`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::trainer_config::{MAX_CONFIG_BYTES, open_configuration};
use super::{
    LaunchedScummVmProcess, ScummVmLaunchExecutionError, ScummVmLaunchPreflightError,
    ScummVmLaunchRequest, preflight_scummvm_launch, spawn_scummvm,
};
use crate::launch::scummvm_command::ScummVmTrainerLaunchBinding;
use crate::patch_manager::scummvm_trainer_options;

/// What the owned profile can honestly be launched as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainerProfileState {
    Missing,
    Unreadable(String),
    /// Settings or structure EmuWiz did not write and cannot vouch for.
    Unsupported(Vec<String>),
    /// The profile is bound to a different game or folder than the selection.
    Mismatch(String),
    Ready(TrainerProfileSummary),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainerProfileSummary {
    /// Digest of the exact bytes the user is shown; launch refuses on change.
    pub sha256: String,
    /// Documented options the profile turns on or off (sorted by key).
    pub options: Vec<(String, bool)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainerLaunchRequest {
    pub selected_game_folder: PathBuf,
    pub expected_game_key: String,
    pub expected_executable: PathBuf,
    pub profile: PathBuf,
    pub target_name: String,
    pub expected_profile_sha256: String,
    /// The user's own ScummVM configuration, read only to find the savepath.
    pub original_configuration: Option<PathBuf>,
    /// Used only when the original configuration names no savepath.
    pub default_savepath: Option<PathBuf>,
    pub scratch_root: PathBuf,
}

#[derive(Debug)]
pub enum TrainerLaunchError {
    ProfileMissing,
    ProfileUnsupported(Vec<String>),
    ProfileMismatch(String),
    ProfileUnreadable(String),
    /// The profile bytes differ from the evidence the user approved.
    ProfileChanged {
        expected: String,
        actual: String,
    },
    SaveLocationUnavailable(String),
    Scratch(String),
    Preflight(ScummVmLaunchPreflightError),
    Spawn(std::io::Error),
}

impl From<ScummVmLaunchExecutionError> for TrainerLaunchError {
    fn from(value: ScummVmLaunchExecutionError) -> Self {
        match value {
            ScummVmLaunchExecutionError::Preflight(e) => Self::Preflight(e),
            ScummVmLaunchExecutionError::Spawn(super::ScummVmLaunchSpawnError::Spawn(e)) => {
                Self::Spawn(e)
            }
        }
    }
}

impl std::fmt::Display for TrainerLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProfileMissing => f.write_str(
                "No saved trainer profile exists for this game. Save one first; ScummVM was not started.",
            ),
            Self::ProfileUnsupported(items) => write!(
                f,
                "The saved profile contains unsupported settings ({}); ScummVM was not started.",
                items.join("; ")
            ),
            Self::ProfileMismatch(detail) => write!(f, "{detail}; ScummVM was not started."),
            Self::ProfileUnreadable(detail) => {
                write!(f, "The saved profile could not be read safely: {detail}")
            }
            Self::ProfileChanged { .. } => f.write_str(
                "The saved trainer profile changed since you reviewed it. Check it again; ScummVM was not started.",
            ),
            Self::SaveLocationUnavailable(detail) => write!(
                f,
                "Your ScummVM save location could not be established: {detail}. ScummVM was not started so your saves stay where they are."
            ),
            Self::Scratch(detail) => write!(
                f,
                "The private launch configuration could not be prepared: {detail}"
            ),
            Self::Preflight(e) => write!(f, "Launch checks refused: {:?}: {}", e.kind, e.detail),
            Self::Spawn(e) => write!(f, "ScummVM could not be started: {e}"),
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Reads the owned profile with the same no-follow, bounded reader as launch
/// validation. `Ok(None)` means the profile does not exist.
fn read_profile(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match open_configuration(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES as u64 {
        return Err("not a bounded regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("exceeded the read bound".into());
    }
    Ok(Some(bytes))
}

/// Classifies already-read profile bytes. Pure, so launch classifies the exact
/// bytes it will copy.
fn classify(bytes: &[u8], target: &str, game_id: &str, folder: &Path) -> TrainerProfileState {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return TrainerProfileState::Unsupported(vec!["the file is not UTF-8 text".into()]);
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Ok(declared) = scummvm_trainer_options(game_id, b"") else {
        return TrainerProfileState::Unsupported(vec![format!(
            "{game_id} has no documented trainer options"
        )]);
    };
    let known: BTreeSet<String> = declared
        .iter()
        .map(|o| o.key.to_ascii_lowercase())
        .collect();
    let mut unsupported = Vec::new();
    let mut sections = BTreeSet::new();
    let mut target_keys: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<String> = None;
    for (index, line) in text.lines().enumerate() {
        let n = index + 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if !sections.insert(name.to_ascii_lowercase()) {
                unsupported.push(format!("line {n}: duplicate section [{name}]"));
            }
            if !name.eq_ignore_ascii_case(target) {
                unsupported.push(format!(
                    "section [{name}] is not part of this trainer profile"
                ));
            }
            current = Some(name.to_string());
        } else if let Some((key, value)) = line.split_once('=') {
            let (key, value) = (key.trim(), value.trim());
            if !current
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(target))
            {
                continue; // already reported as an unsupported section
            }
            let lower = key.to_ascii_lowercase();
            if target_keys
                .insert(lower.clone(), value.to_string())
                .is_some()
            {
                unsupported.push(format!("line {n}: duplicate setting {key}"));
            } else if !matches!(lower.as_str(), "engineid" | "gameid" | "path")
                && !known.contains(&lower)
            {
                unsupported.push(format!("setting {key}={value}"));
            } else if known.contains(&lower) && !matches!(value, "true" | "false") {
                unsupported.push(format!("setting {key}={value} is not true or false"));
            }
        } else {
            unsupported.push(format!("line {n} is not a setting or section"));
        }
    }
    if !sections.contains(&target.to_ascii_lowercase()) {
        unsupported.push(format!("target section [{target}] is missing"));
    }
    if !unsupported.is_empty() {
        unsupported.sort();
        unsupported.dedup();
        return TrainerProfileState::Unsupported(unsupported);
    }
    let Some((engine, game)) = game_id.split_once(':') else {
        return TrainerProfileState::Mismatch("invalid game identity".into());
    };
    let bound = |key: &str| target_keys.get(key).map(String::as_str);
    if bound("engineid") != Some(engine)
        || bound("gameid") != Some(game)
        || bound("path").map(Path::new) != Some(folder)
    {
        return TrainerProfileState::Mismatch(
            "The saved profile is bound to a different game or folder than the one selected".into(),
        );
    }
    let mut options: Vec<(String, bool)> = declared
        .iter()
        .filter_map(|o| {
            target_keys
                .get(&o.key.to_ascii_lowercase())
                .map(|v| (o.key.clone(), v == "true"))
        })
        .collect();
    options.sort();
    TrainerProfileState::Ready(TrainerProfileSummary {
        sha256: sha256_hex(bytes),
        options,
    })
}

/// Read-only inspection of the owned profile for the selected game.
pub fn inspect_trainer_profile(
    profile: &Path,
    target: &str,
    game_id: &str,
    folder: &Path,
) -> TrainerProfileState {
    match read_profile(profile) {
        Ok(None) => TrainerProfileState::Missing,
        Err(e) => TrainerProfileState::Unreadable(e),
        Ok(Some(bytes)) => classify(&bytes, target, game_id, folder),
    }
}

/// The savepath ScummVM would use for this game in the user's own
/// configuration: a game section bound to this folder, else the global one.
/// The file is only read.
fn original_savepath(
    original: Option<&Path>,
    folder: &Path,
    default: Option<&Path>,
) -> Result<PathBuf, String> {
    let mut found: Option<String> = None;
    if let Some(path) = original {
        let bytes = match read_profile(path) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => Vec::new(),
            Err(e) => return Err(format!("your ScummVM configuration is unreadable ({e})")),
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut global: Option<String> = None;
        let mut sections: Vec<(String, BTreeMap<String, String>)> = Vec::new();
        for line in text.lines() {
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                sections.push((name.to_string(), BTreeMap::new()));
            } else if let Some((k, v)) = line.split_once('=')
                && let Some((_, map)) = sections.last_mut()
            {
                map.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        for (name, map) in &sections {
            if name == "scummvm" {
                global = map.get("savepath").cloned();
            } else if map.get("path").map(Path::new) == Some(folder)
                && let Some(value) = map.get("savepath")
            {
                found = Some(value.clone());
            }
        }
        found = found.or(global);
    }
    let chosen = match found {
        Some(value) => PathBuf::from(value),
        None => default.map(Path::to_path_buf).ok_or_else(|| {
            "no savepath is set in your ScummVM configuration and no default location is known"
                .to_string()
        })?,
    };
    let text = chosen.to_string_lossy();
    if !chosen.is_absolute()
        || chosen
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        || text.chars().any(char::is_control)
    {
        return Err("the save location is not a plain absolute path".into());
    }
    if !fs::metadata(&chosen).is_ok_and(|m| m.is_dir()) {
        return Err(format!(
            "the save location {} is not an existing directory",
            chosen.display()
        ));
    }
    Ok(chosen)
}

/// A running ScummVM started on a private configuration copy. The copy is
/// removed when the process is observed to exit or this value is dropped.
pub struct LaunchedTrainerScummVm {
    pub process: LaunchedScummVmProcess,
    pub profile_sha256: String,
    pub savepath: PathBuf,
    scratch: Option<tempfile::TempDir>,
    cleanup: Option<Result<(), String>>,
}

impl LaunchedTrainerScummVm {
    /// Polls the process; once it has exited the private copy is removed.
    pub fn poll(&mut self) -> Option<&super::ScummVmLaunchExitReport> {
        if self.process.poll().is_some()
            && let Some(dir) = self.scratch.take()
        {
            self.cleanup = Some(dir.close().map_err(|e| e.to_string()));
        }
        self.process.poll()
    }

    /// `None` until the process has exited and the copy was removed.
    pub fn cleanup_result(&self) -> Option<&Result<(), String>> {
        self.cleanup.as_ref()
    }
}

/// Revalidates everything, then starts ScummVM on a private configuration.
/// Nothing is written outside `scratch_root`; the owned profile, the user's
/// configuration, saves and game folder are only read.
pub fn preflight_and_launch_with_trainer_profile(
    request: &TrainerLaunchRequest,
) -> Result<LaunchedTrainerScummVm, TrainerLaunchError> {
    let bytes = read_profile(&request.profile)
        .map_err(TrainerLaunchError::ProfileUnreadable)?
        .ok_or(TrainerLaunchError::ProfileMissing)?;
    match classify(
        &bytes,
        &request.target_name,
        &request.expected_game_key,
        &request.selected_game_folder,
    ) {
        TrainerProfileState::Ready(summary) => {
            if summary.sha256 != request.expected_profile_sha256 {
                return Err(TrainerLaunchError::ProfileChanged {
                    expected: request.expected_profile_sha256.clone(),
                    actual: summary.sha256,
                });
            }
        }
        TrainerProfileState::Unsupported(items) => {
            return Err(TrainerLaunchError::ProfileUnsupported(items));
        }
        TrainerProfileState::Mismatch(detail) => {
            return Err(TrainerLaunchError::ProfileMismatch(detail));
        }
        TrainerProfileState::Missing => return Err(TrainerLaunchError::ProfileMissing),
        TrainerProfileState::Unreadable(e) => return Err(TrainerLaunchError::ProfileUnreadable(e)),
    }
    let savepath = original_savepath(
        request.original_configuration.as_deref(),
        &request.selected_game_folder,
        request.default_savepath.as_deref(),
    )
    .map_err(TrainerLaunchError::SaveLocationUnavailable)?;

    fs::create_dir_all(&request.scratch_root)
        .map_err(|e| TrainerLaunchError::Scratch(e.to_string()))?;
    let scratch = tempfile::Builder::new()
        .prefix("scummvm-trainer-")
        .tempdir_in(&request.scratch_root)
        .map_err(|e| TrainerLaunchError::Scratch(e.to_string()))?;
    let configuration = scratch.path().join("scummvm.ini");
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&configuration)
            .map_err(|e| TrainerLaunchError::Scratch(e.to_string()))?;
        file.write_all(&bytes)
            .and_then(|()| {
                if bytes.last().is_some_and(|b| *b != b'\n') {
                    file.write_all(b"\n")?;
                }
                writeln!(file, "[scummvm]\nsavepath={}", savepath.display())
            })
            .and_then(|()| file.sync_all())
            .map_err(|e| TrainerLaunchError::Scratch(e.to_string()))?;
    }
    let process = spawn_scummvm(
        preflight_scummvm_launch(&ScummVmLaunchRequest {
            selected_game_folder: request.selected_game_folder.clone(),
            expected_game_key: request.expected_game_key.clone(),
            expected_executable: request.expected_executable.clone(),
            trainer: Some(ScummVmTrainerLaunchBinding {
                configuration,
                target_name: request.target_name.clone(),
            }),
        })
        .map_err(TrainerLaunchError::Preflight)?,
    )
    .map_err(|super::ScummVmLaunchSpawnError::Spawn(e)| TrainerLaunchError::Spawn(e))?;
    Ok(LaunchedTrainerScummVm {
        process,
        profile_sha256: request.expected_profile_sha256.clone(),
        savepath,
        scratch: Some(scratch),
        cleanup: None,
    })
}

#[cfg(test)]
mod tests;
