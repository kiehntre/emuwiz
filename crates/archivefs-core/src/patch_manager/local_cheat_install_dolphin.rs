//! Bridges a user-selected local Dolphin cheat file (a GameSettings-shaped
//! `.ini` snippet declaring `[Gecko]` and/or `[ActionReplay]`) into the
//! existing, unmodified Dolphin install-plan pipeline
//! (`dolphin_gecko_install_plan`, `gecko_document`, `shared_transaction`).
//!
//! This module adds no new write path and no new cheat-code grammar. It
//! only:
//! - validates the user-picked source file with the same source-path
//!   safety rules used across every local-file install in this codebase
//!   (reject symlink/directory/non-regular/oversized/wrong-extension
//!   before any parse),
//! - parses the file's own `[Gecko]`/`[ActionReplay]` sections with the
//!   existing, unmodified [`parse_dolphin_ini`] - the same reader Dolphin's
//!   real GameSettings files are read with, so a code's format is decided
//!   only by an explicit section header the file itself declares, never
//!   guessed from the hex shape (matching the code-format audit already
//!   documented on `GameCubeCodeFormat`),
//! - binds the file to the currently selected game via the *already
//!   resolved* [`DolphinCandidate`] the existing Dolphin Cheats & Mods
//!   workflow produces (`build_dolphin_candidate`, unmodified) - this
//!   bridge never re-derives game identity itself - and additionally
//!   checks the source file's own `<GameID>[rN]` filename convention
//!   (reusing [`super::dolphin_local::parse_game_identity`] unmodified)
//!   against that candidate, exactly as the PCSX2 bridge checks its
//!   `<SERIAL>_<CRC>.pnach` filename convention against the selected
//!   game's verified PCSX2 identity,
//! - and produces exactly the merged `GameSettings` text that
//!   [`super::dolphin_gecko_install_plan::build_dolphin_install_preview`]
//!   (unmodified) already knows how to preview, apply, and roll back.
//!
//! ## Scope
//!
//! Dolphin Gecko and Action Replay local `.ini` files only. RetroArch
//! `.cht` and PCSX2 `.pnach` local install each have their own equivalent
//! bridge already; Xenia `.patch.toml` local install is deferred.
//!
//! ## Idempotent re-apply
//!
//! [`merge_external_gecko_codes`]/[`merge_external_action_replay_codes`]
//! already treat "same name, same body" as a no-op addition (never a
//! duplicate), so re-merging the exact same file is always safe. This
//! module additionally exposes [`check_local_dolphin_install_state`] so a
//! caller can report "already installed, unchanged" up front rather than
//! staging and applying a byte-identical file again.

use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::dolphin_gecko_install_plan::{
    DolphinCandidate, DolphinInstallPlanError, DolphinInstallPlanErrorKind,
    LoadedDolphinDestination, MAX_GENERATED_INI_BYTES, StagedDolphinIni,
};
use super::dolphin_local::parse_game_identity;
use super::gecko_document::{
    DolphinIniWarningKind, GeckoCode, merge_external_action_replay_codes,
    merge_external_gecko_codes, parse_dolphin_ini,
};

/// Managed-file byte limit for a user-selected local Dolphin cheat file,
/// mirroring `dolphin_local::DOLPHIN_MAX_GAME_INI_BYTES`.
pub const MAX_LOCAL_DOLPHIN_INI_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalDolphinCodeKind {
    Gecko,
    ActionReplay,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalDolphinFileError {
    NotFound {
        path: PathBuf,
        detail: String,
    },
    IsDirectory {
        path: PathBuf,
    },
    IsSymlink {
        path: PathBuf,
    },
    NotRegularFile {
        path: PathBuf,
    },
    UnsupportedExtension {
        path: PathBuf,
    },
    TooLarge {
        path: PathBuf,
    },
    Malformed {
        path: PathBuf,
        detail: String,
    },
    /// The file has no valid, selectable Gecko or Action Replay code.
    NoCodesFound {
        path: PathBuf,
    },
    /// The file's own `<GameID>[rN]` filename does not match the already
    /// resolved [`DolphinCandidate`] for the selected game.
    IdentityConflict {
        detail: String,
    },
    /// Surfaced unchanged from the existing Dolphin install-plan machinery
    /// (destination unsafe, candidate unreadable, generated file too
    /// large, preview failed, ...).
    Plan(DolphinInstallPlanError),
}

impl std::fmt::Display for LocalDolphinFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::IsDirectory { path } => write!(
                formatter,
                "{} is a directory, not a cheat file",
                path.display()
            ),
            Self::IsSymlink { path } => write!(
                formatter,
                "{} is a symlink and is not followed",
                path.display()
            ),
            Self::NotRegularFile { path } => {
                write!(formatter, "{} is not a regular file", path.display())
            }
            Self::UnsupportedExtension { path } => write!(
                formatter,
                "{} is not a supported Dolphin .ini Gecko/Action Replay cheat file",
                path.display()
            ),
            Self::TooLarge { path } => write!(
                formatter,
                "{} exceeds the managed-file byte limit",
                path.display()
            ),
            Self::Malformed { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::NoCodesFound { path } => write!(
                formatter,
                "{} contains no valid Gecko or Action Replay codes",
                path.display()
            ),
            Self::IdentityConflict { detail } => formatter.write_str(detail),
            Self::Plan(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for LocalDolphinFileError {}

impl From<DolphinInstallPlanError> for LocalDolphinFileError {
    fn from(error: DolphinInstallPlanError) -> Self {
        Self::Plan(error)
    }
}

/// The codes a local file resolves to, plus the candidate it was checked
/// against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDolphinDiscovery {
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub kind: LocalDolphinCodeKind,
    /// Selectable `[Gecko]` codes found in the file, in file order. Empty
    /// unless `kind` is `Gecko` or `Both`.
    pub gecko_codes: Vec<GeckoCode>,
    /// Selectable `[ActionReplay]` codes found in the file, in file order.
    /// Empty unless `kind` is `ActionReplay` or `Both`.
    pub action_replay_codes: Vec<GeckoCode>,
}

/// Validates `source_path` (must exist, be a regular non-symlink file,
/// have a `.ini` extension, and be within the managed-file byte limit),
/// parses it with the existing, unmodified [`parse_dolphin_ini`], and
/// checks its own `<GameID>[rN].ini` filename convention (when present)
/// against `candidate` - the already-resolved Dolphin destination for the
/// currently selected game.
///
/// `candidate` must already be `installable` (an exact game-ID, and when
/// applicable exact-revision, match) - callers reach this function only
/// after `build_dolphin_candidate` (unmodified) has already refused an
/// unresolved or ambiguous identity, exactly as the provider-driven Gecko
/// install flow requires.
pub fn discover_local_dolphin_cheat_file(
    source_path: &Path,
    candidate: &DolphinCandidate,
) -> Result<LocalDolphinDiscovery, LocalDolphinFileError> {
    let metadata = std::fs::symlink_metadata(source_path).map_err(|error| {
        LocalDolphinFileError::NotFound {
            path: source_path.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(LocalDolphinFileError::IsSymlink {
            path: source_path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        return Err(LocalDolphinFileError::IsDirectory {
            path: source_path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(LocalDolphinFileError::NotRegularFile {
            path: source_path.to_path_buf(),
        });
    }
    let has_ini_extension = source_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ini"));
    if !has_ini_extension {
        return Err(LocalDolphinFileError::UnsupportedExtension {
            path: source_path.to_path_buf(),
        });
    }
    if metadata.len() > MAX_LOCAL_DOLPHIN_INI_BYTES {
        return Err(LocalDolphinFileError::TooLarge {
            path: source_path.to_path_buf(),
        });
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(source_path)
        .map_err(|error| LocalDolphinFileError::NotFound {
            path: source_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mut bytes = Vec::new();
    file.take((MAX_LOCAL_DOLPHIN_INI_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| LocalDolphinFileError::Malformed {
            path: source_path.to_path_buf(),
            detail: format!("could not be read: {error}"),
        })?;
    if bytes.len() as u64 > MAX_LOCAL_DOLPHIN_INI_BYTES {
        return Err(LocalDolphinFileError::TooLarge {
            path: source_path.to_path_buf(),
        });
    }

    if let Some(stem) = source_path.file_stem() {
        let (file_game_id, file_revision, _region) = parse_game_identity(stem);
        // A real Dolphin game ID always contains at least one digit (e.g.
        // `GALE01`, `RSBE01`, `SOUE8P`) - `parse_game_identity`'s bare
        // "3-6 alphanumeric characters" shape is otherwise indistinguishable
        // from an ordinary, purely alphabetic filename a user picked for
        // their own cheat file (e.g. "cheats.ini"), so only a digit-bearing
        // match is ever treated as the file making an identity claim at
        // all; a purely alphabetic stem is never a conflict.
        let file_game_id = file_game_id.filter(|id| id.bytes().any(|byte| byte.is_ascii_digit()));
        if let Some(file_game_id) = file_game_id {
            if file_game_id != candidate.game_id {
                return Err(LocalDolphinFileError::IdentityConflict {
                    detail: format!(
                        "{} targets Dolphin game ID {file_game_id}, which does not match the \
                         selected game's verified game ID {}",
                        source_path.display(),
                        candidate.game_id
                    ),
                });
            }
            let candidate_revision = candidate.revision.unwrap_or(0);
            let file_revision = file_revision.unwrap_or(0);
            if file_revision != candidate_revision {
                return Err(LocalDolphinFileError::IdentityConflict {
                    detail: format!(
                        "{} targets disc revision {file_revision}, which does not match the \
                         selected game's verified disc revision {candidate_revision}",
                        source_path.display()
                    ),
                });
            }
        }
    }

    let text = std::str::from_utf8(&bytes).map_err(|_| LocalDolphinFileError::Malformed {
        path: source_path.to_path_buf(),
        detail: "Dolphin cheat file is not valid UTF-8".to_string(),
    })?;
    let document = parse_dolphin_ini(text);
    if document
        .warnings
        .iter()
        .any(|warning| warning.kind == DolphinIniWarningKind::TooManyCodes)
    {
        return Err(LocalDolphinFileError::Malformed {
            path: source_path.to_path_buf(),
            detail: "file exceeds the supported Gecko/Action Replay code-count limit".to_string(),
        });
    }
    if let Some(warning) = document
        .warnings
        .iter()
        .find(|warning| warning.kind == DolphinIniWarningKind::MalformedSectionHeader)
    {
        return Err(LocalDolphinFileError::Malformed {
            path: source_path.to_path_buf(),
            detail: warning.detail.clone(),
        });
    }

    let gecko_codes = reject_unselectable(source_path, &document.gecko_codes)?;
    let action_replay_codes = reject_unselectable(source_path, &document.action_replay_codes)?;

    let kind = match (!gecko_codes.is_empty(), !action_replay_codes.is_empty()) {
        (true, true) => LocalDolphinCodeKind::Both,
        (true, false) => LocalDolphinCodeKind::Gecko,
        (false, true) => LocalDolphinCodeKind::ActionReplay,
        (false, false) => {
            return Err(LocalDolphinFileError::NoCodesFound {
                path: source_path.to_path_buf(),
            });
        }
    };

    Ok(LocalDolphinDiscovery {
        source_path: source_path.to_path_buf(),
        source_sha256: hex_sha256(&bytes),
        kind,
        gecko_codes,
        action_replay_codes,
    })
}

/// Every code in `codes` must already be selectable (non-empty body, no
/// blocking warning). Unlike the read-only provider catalogue, this is a
/// write-path parser: a single malformed code rejects the whole file
/// rather than silently dropping it, matching the PCSX2 and RetroArch
/// local-file bridges' own strictness.
fn reject_unselectable(
    source_path: &Path,
    codes: &[GeckoCode],
) -> Result<Vec<GeckoCode>, LocalDolphinFileError> {
    for code in codes {
        if !code.is_selectable() {
            return Err(LocalDolphinFileError::Malformed {
                path: source_path.to_path_buf(),
                detail: format!(
                    "code {:?} is malformed: {}",
                    code.name,
                    code.warnings
                        .first()
                        .map(|warning| warning.detail.as_str())
                        .unwrap_or("no valid code lines")
                ),
            });
        }
    }
    Ok(codes.to_vec())
}

/// Whether the discovered codes are already installed, unchanged, at
/// `destination`. Pure: never touches a filesystem itself - `destination`
/// is the caller's own already-loaded [`LoadedDolphinDestination`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalDolphinInstallState {
    New,
    AlreadyInstalled,
}

#[must_use]
pub fn check_local_dolphin_install_state(
    destination: &LoadedDolphinDestination,
    discovery: &LocalDolphinDiscovery,
) -> LocalDolphinInstallState {
    let gecko_unchanged = discovery.gecko_codes.iter().all(|code| {
        destination
            .document
            .gecko_codes
            .iter()
            .any(|existing| existing.name == code.name && existing.lines == code.lines)
            && destination
                .document
                .gecko_enabled_names
                .iter()
                .any(|name| name == &code.name)
    });
    let action_replay_unchanged = discovery.action_replay_codes.iter().all(|code| {
        destination
            .document
            .action_replay_codes
            .iter()
            .any(|existing| existing.name == code.name && existing.lines == code.lines)
            && destination
                .document
                .action_replay_enabled_names
                .iter()
                .any(|name| name == &code.name)
    });
    if gecko_unchanged && action_replay_unchanged {
        LocalDolphinInstallState::AlreadyInstalled
    } else {
        LocalDolphinInstallState::New
    }
}

fn plan_error(
    kind: DolphinInstallPlanErrorKind,
    path: Option<&Path>,
    detail: impl Into<String>,
) -> DolphinInstallPlanError {
    DolphinInstallPlanError {
        kind,
        path: path.map(Path::to_path_buf),
        detail: detail.into(),
    }
}

/// Merges `discovery`'s codes into `destination`'s document (via the
/// existing, unmodified [`merge_external_gecko_codes`]/
/// [`merge_external_action_replay_codes`] - never a new merge algorithm)
/// and stages the result atomically into `staging_root`, exactly the
/// staging discipline `stage_dolphin_provider_ini` already uses: the real
/// GameSettings file is never written to directly here.
pub fn stage_local_dolphin_codes(
    staging_root: &Path,
    destination: &LoadedDolphinDestination,
    discovery: &LocalDolphinDiscovery,
) -> Result<StagedDolphinIni, LocalDolphinFileError> {
    let mut contents: Option<String> = None;
    let mut selected_names = Vec::new();

    if !discovery.gecko_codes.is_empty() {
        let names: Vec<String> = discovery
            .gecko_codes
            .iter()
            .map(|code| code.name.clone())
            .collect();
        let merged =
            merge_external_gecko_codes(&destination.document, &discovery.gecko_codes, &names)
                .map_err(|failure| {
                    LocalDolphinFileError::Plan(plan_error(
                        DolphinInstallPlanErrorKind::SelectionInvalid,
                        Some(&destination.path),
                        failure.to_string(),
                    ))
                })?;
        selected_names.extend(names);
        contents = Some(merged);
    }

    if !discovery.action_replay_codes.is_empty() {
        let document = match &contents {
            Some(text) => parse_dolphin_ini(text),
            None => destination.document.clone(),
        };
        let names: Vec<String> = discovery
            .action_replay_codes
            .iter()
            .map(|code| code.name.clone())
            .collect();
        let merged =
            merge_external_action_replay_codes(&document, &discovery.action_replay_codes, &names)
                .map_err(|failure| {
                LocalDolphinFileError::Plan(plan_error(
                    DolphinInstallPlanErrorKind::SelectionInvalid,
                    Some(&destination.path),
                    failure.to_string(),
                ))
            })?;
        selected_names.extend(names);
        contents = Some(merged);
    }

    let contents = contents.ok_or_else(|| LocalDolphinFileError::NoCodesFound {
        path: discovery.source_path.clone(),
    })?;
    if contents.len() > MAX_GENERATED_INI_BYTES {
        return Err(LocalDolphinFileError::Plan(plan_error(
            DolphinInstallPlanErrorKind::GeneratedFileTooLarge,
            Some(&destination.path),
            format!("generated GameSettings file exceeds {MAX_GENERATED_INI_BYTES} bytes"),
        )));
    }

    std::fs::create_dir_all(staging_root).map_err(|failure| {
        LocalDolphinFileError::Plan(plan_error(
            DolphinInstallPlanErrorKind::StagingUnavailable,
            Some(staging_root),
            format!("staging directory unavailable: {failure}"),
        ))
    })?;
    let file_name = destination
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            LocalDolphinFileError::Plan(plan_error(
                DolphinInstallPlanErrorKind::DestinationUnsafe,
                Some(&destination.path),
                "Dolphin destination has no usable filename",
            ))
        })?;
    let path = staging_root.join(file_name);
    let temporary = staging_root.join(format!(".{file_name}.partial"));
    std::fs::write(&temporary, &contents).map_err(|failure| {
        LocalDolphinFileError::Plan(plan_error(
            DolphinInstallPlanErrorKind::StagingUnavailable,
            Some(&temporary),
            format!("staged file could not be written: {failure}"),
        ))
    })?;
    std::fs::rename(&temporary, &path).map_err(|failure| {
        let _ = std::fs::remove_file(&temporary);
        LocalDolphinFileError::Plan(plan_error(
            DolphinInstallPlanErrorKind::StagingUnavailable,
            Some(&path),
            format!("staged file could not be finalized: {failure}"),
        ))
    })?;

    Ok(StagedDolphinIni {
        staging_root: staging_root.to_path_buf(),
        path,
        digest: hex_sha256(contents.as_bytes()),
        contents,
        selected_code_count: selected_names.len(),
        selected_code_names: selected_names,
        destination_existed: destination.existed,
        preserved_sections: destination.document.section_names(),
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
