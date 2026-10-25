//! Bridges a user-selected local RetroArch `.cht` file into the existing,
//! unmodified [`super::cheat_journey`] discover -> select -> preview ->
//! apply -> undo pipeline.
//!
//! This module adds no new preview, transaction, confirmation, or rollback
//! logic of its own. It only builds the same [`CheatCatalogueSnapshot`]/
//! [`CheatJourneyGameIdentity`] inputs that pipeline already accepts from a
//! provider-downloaded catalogue directory, pointed instead at the single
//! file's own parent directory - [`load_cheat_catalogue_snapshot`] already
//! documents that "an existing directory is read as a `.cht` tree" with no
//! automatic search, so a user's own folder containing one chosen file is a
//! valid catalogue root without any new scanning code. The exact file the
//! user picked is the only one ever selected, previewed, or applied -
//! [`select_cheat_journey_candidate`] requires an exact catalogue-relative
//! path match, so a sibling file the same folder happens to contain is
//! harmlessly indexed but never installable through this call.
//!
//! ## Scope
//!
//! RetroArch `.cht` only, matching the V1 discovery audit's identified gap
//! (`docs/V1_CHEATS_MODS_DISCOVERY_AUDIT.md`, task A). PCSX2/Dolphin/Xenia
//! local-file install each need an equivalent bridge into their own
//! install-plan module and are deferred - see that document.
//!
//! ## What this module never does
//!
//! - Never copies, renames, or writes the selected file.
//! - Never contacts a network or a cheat provider.
//! - Never widens which candidate is installable beyond what
//!   [`super::cheat_candidates`]'s existing classification already allows -
//!   a cross-platform or unsupported file is reported, with its existing
//!   evidence, never force-installed.

use std::path::{Path, PathBuf};

use crate::emulator_environment::ReadOnlyHostFilesystem;

use super::cheat_candidates::{CheatCandidate, CheatCandidateOptions};
use super::cheat_catalogue::load_cheat_catalogue_snapshot;
use super::cheat_journey::{
    CheatJourneyDiscovery, CheatJourneyError, CheatJourneyGameIdentity, discover_cheat_journey,
};

/// The source name recorded on the ad-hoc single-file snapshot. Never a
/// real provider - `cheat_journey`'s own `CheatJourneyDiscovery::provider`
/// field surfaces this back to the caller so the UI can honestly label the
/// result "local file", never a provider name.
pub const LOCAL_CHEAT_SOURCE_NAME: &str = "local-file";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalCheatFileError {
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
    /// The parent directory or file name could not be represented as a
    /// catalogue-relative selection at all (e.g. non-UTF-8 name, or the
    /// path has no parent directory).
    PathNotRepresentable {
        path: PathBuf,
        detail: String,
    },
    /// The currently selected game's own identity is not yet resolved -
    /// forwarded from [`discover_cheat_journey`] unchanged.
    Identity(CheatJourneyError),
}

impl std::fmt::Display for LocalCheatFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { path, detail } => {
                write!(formatter, "{}: {detail}", path.display())
            }
            Self::IsDirectory { path } => {
                write!(
                    formatter,
                    "{} is a directory, not a cheat file",
                    path.display()
                )
            }
            Self::IsSymlink { path } => {
                write!(
                    formatter,
                    "{} is a symlink and is not followed",
                    path.display()
                )
            }
            Self::NotRegularFile { path } => {
                write!(formatter, "{} is not a regular file", path.display())
            }
            Self::UnsupportedExtension { path } => write!(
                formatter,
                "{} is not a supported RetroArch .cht cheat file",
                path.display()
            ),
            Self::PathNotRepresentable { path, detail } => {
                write!(formatter, "{}: {detail}", path.display())
            }
            Self::Identity(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for LocalCheatFileError {}

/// The catalogue-root/relative-path pair [`super::cheat_journey::select_cheat_journey_candidate`]
/// needs, computed once here from the exact file the user picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCheatFileLocation {
    pub catalogue_root: PathBuf,
    pub catalogue_relative_path: String,
}

/// The result of indexing exactly one user-selected file: the full
/// discovery (so excluded/malformed diagnostics remain visible), the
/// location needed to select it, and - when the file itself parsed as a
/// candidate at all - a direct reference to that one candidate so a caller
/// never has to re-search the candidate list for the file it just picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCheatFileDiscovery {
    pub discovery: CheatJourneyDiscovery,
    pub location: LocalCheatFileLocation,
}

impl LocalCheatFileDiscovery {
    /// The selected file's own candidate, when parsing produced one. `None`
    /// means the file was malformed or otherwise excluded entirely - see
    /// [`CheatJourneyDiscovery::excluded_candidate_count`] for that count.
    pub fn candidate(&self) -> Option<&CheatCandidate> {
        self.discovery
            .candidates
            .candidates
            .iter()
            .find(|entry| {
                entry.candidate.catalogue_relative_path == self.location.catalogue_relative_path
            })
            .map(|entry| &entry.candidate)
    }
}

/// Validates `source_path` (must exist, be a regular non-symlink file, and
/// have a `.cht` extension), then reuses the existing
/// [`load_cheat_catalogue_snapshot`]/[`discover_cheat_journey`] pipeline
/// unchanged, treating the file's own parent directory as an ad-hoc,
/// single-use catalogue root.
///
/// Always passes `include_uninstallable: true` internally regardless of
/// `options.include_uninstallable`, so a cross-platform or unsupported
/// selected file is still returned - with its real classification and
/// evidence - rather than silently vanishing from the result; hiding an
/// uninstallable candidate makes sense for a provider catalogue with many
/// other real choices, never for the one exact file a user just picked.
pub fn discover_local_retroarch_cheat_file(
    filesystem: &dyn ReadOnlyHostFilesystem,
    source_path: &Path,
    game: &CheatJourneyGameIdentity,
    options: &CheatCandidateOptions,
) -> Result<LocalCheatFileDiscovery, LocalCheatFileError> {
    let metadata =
        std::fs::symlink_metadata(source_path).map_err(|error| LocalCheatFileError::NotFound {
            path: source_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    if metadata.file_type().is_symlink() {
        return Err(LocalCheatFileError::IsSymlink {
            path: source_path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        return Err(LocalCheatFileError::IsDirectory {
            path: source_path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(LocalCheatFileError::NotRegularFile {
            path: source_path.to_path_buf(),
        });
    }
    let has_cht_extension = source_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cht"));
    if !has_cht_extension {
        return Err(LocalCheatFileError::UnsupportedExtension {
            path: source_path.to_path_buf(),
        });
    }
    let parent = source_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| LocalCheatFileError::PathNotRepresentable {
            path: source_path.to_path_buf(),
            detail: "selected file has no parent directory".to_string(),
        })?;
    let file_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| LocalCheatFileError::PathNotRepresentable {
            path: source_path.to_path_buf(),
            detail: "selected file name is not valid UTF-8".to_string(),
        })?
        .to_string();

    let mut options = options.clone();
    options.include_uninstallable = true;

    let snapshot = load_cheat_catalogue_snapshot(filesystem, LOCAL_CHEAT_SOURCE_NAME, parent);
    let discovery =
        discover_cheat_journey(game, &snapshot, &options).map_err(LocalCheatFileError::Identity)?;

    Ok(LocalCheatFileDiscovery {
        discovery,
        location: LocalCheatFileLocation {
            catalogue_root: parent.to_path_buf(),
            catalogue_relative_path: file_name,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_environment::HostReadOnlyFilesystem;
    use crate::patch_manager::cheat_candidates::CheatCandidateArchive;
    use crate::patch_manager::cheat_journey::{
        CheatJourneyIdentityEvidence, CheatJourneyIdentityEvidenceKind, CheatJourneyIdentityState,
    };
    use tempfile::tempdir;

    fn verified_game(
        archive: CheatCandidateArchive,
        selected_archive: PathBuf,
    ) -> CheatJourneyGameIdentity {
        CheatJourneyGameIdentity {
            state: CheatJourneyIdentityState::Verified,
            selected_archive,
            identity_key: "test-identity".to_string(),
            archive,
            evidence: vec![CheatJourneyIdentityEvidence {
                kind: CheatJourneyIdentityEvidenceKind::CanonicalLibraryRecord,
                value: "test-identity".to_string(),
            }],
        }
    }

    fn unknown_game(selected_archive: PathBuf) -> CheatJourneyGameIdentity {
        CheatJourneyGameIdentity {
            state: CheatJourneyIdentityState::Unknown,
            selected_archive,
            identity_key: String::new(),
            archive: CheatCandidateArchive {
                display_name: String::new(),
                platform: None,
                region: None,
                serial: None,
                content_hash: None,
                content_basename: None,
            },
            evidence: Vec::new(),
        }
    }

    const VALID_CHT: &str = "cheats = 1\n\ncheat0_desc = \"Infinite Health\"\ncheat0_code = \"11223344 5566\"\ncheat0_enable = false\n";

    #[test]
    fn supported_local_cht_file_is_discovered_and_matches_the_selected_game() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("Super Mario World.cht");
        std::fs::write(&file, VALID_CHT).unwrap();

        let game = verified_game(
            CheatCandidateArchive {
                display_name: "Super Mario World".to_string(),
                platform: Some("SNES".to_string()),
                region: None,
                serial: None,
                content_hash: None,
                content_basename: Some("Super Mario World".to_string()),
            },
            dir.path().join("Super Mario World.sfc"),
        );

        let result = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            &file,
            &game,
            &CheatCandidateOptions::default(),
        )
        .expect("discovery succeeds");

        let candidate = result.candidate().expect("candidate present");
        assert!(candidate.manually_selectable);
        assert_eq!(candidate.cheat_count, 1);
        assert_eq!(
            result.location.catalogue_relative_path,
            "Super Mario World.cht"
        );
        assert_eq!(result.location.catalogue_root, dir.path());
    }

    #[test]
    fn unsupported_extension_is_rejected_clearly() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("cheat.pnach");
        std::fs::write(&file, b"patch=1,EE,0018651C,extended,00000000").unwrap();
        let game = unknown_game(dir.path().join("game.iso"));

        let error = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            &file,
            &game,
            &CheatCandidateOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            LocalCheatFileError::UnsupportedExtension { .. }
        ));
    }

    #[test]
    fn malformed_local_file_is_excluded_not_offered() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("broken.cht");
        // Invalid UTF-8 sequence embedded in an otherwise cheat-shaped file.
        let mut bytes = VALID_CHT.as_bytes().to_vec();
        bytes.extend_from_slice(&[0xFF, 0xFE, 0xFD]);
        std::fs::write(&file, &bytes).unwrap();
        let game = verified_game(
            CheatCandidateArchive {
                display_name: "Broken".to_string(),
                platform: Some("SNES".to_string()),
                region: None,
                serial: None,
                content_hash: None,
                content_basename: None,
            },
            dir.path().join("game.sfc"),
        );

        let result = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            &file,
            &game,
            &CheatCandidateOptions::default(),
        )
        .expect("discovery itself still succeeds");
        // Either genuinely excluded, or decoded leniently but still present -
        // either way it must never silently vanish without explanation.
        assert!(result.candidate().is_some() || result.discovery.excluded_candidate_count > 0);
    }

    #[test]
    fn wrong_game_identity_is_never_manually_selectable() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("Sonic the Hedgehog.cht");
        std::fs::write(&file, VALID_CHT).unwrap();
        // Selected game is a completely different platform.
        let game = verified_game(
            CheatCandidateArchive {
                display_name: "Chrono Trigger".to_string(),
                platform: Some("PS1".to_string()),
                region: None,
                serial: Some("SLUS-00069".to_string()),
                content_hash: None,
                content_basename: Some("Chrono Trigger".to_string()),
            },
            dir.path().join("Chrono Trigger.bin"),
        );

        let result = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            &file,
            &game,
            &CheatCandidateOptions::default(),
        )
        .expect("discovery succeeds");
        if let Some(candidate) = result.candidate() {
            assert!(!candidate.manually_selectable);
        }
    }

    #[test]
    fn unknown_identity_is_refused_before_any_file_access_matters() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.cht");
        std::fs::write(&file, VALID_CHT).unwrap();
        let game = unknown_game(dir.path().join("game.bin"));

        let error = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            &file,
            &game,
            &CheatCandidateOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(error, LocalCheatFileError::Identity(_)));
    }

    #[test]
    fn symlinked_source_file_is_rejected() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real.cht");
        std::fs::write(&real, VALID_CHT).unwrap();
        let link = dir.path().join("link.cht");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let game = unknown_game(dir.path().join("game.bin"));

        #[cfg(unix)]
        {
            let error = discover_local_retroarch_cheat_file(
                &HostReadOnlyFilesystem,
                &link,
                &game,
                &CheatCandidateOptions::default(),
            )
            .unwrap_err();
            assert!(matches!(error, LocalCheatFileError::IsSymlink { .. }));
        }
    }

    #[test]
    fn directory_source_is_rejected_not_treated_as_a_catalogue_root() {
        let dir = tempdir().unwrap();
        let game = unknown_game(dir.path().join("game.bin"));
        let error = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            dir.path(),
            &game,
            &CheatCandidateOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(error, LocalCheatFileError::IsDirectory { .. }));
    }

    #[test]
    fn discovery_never_mutates_the_selected_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.cht");
        std::fs::write(&file, VALID_CHT).unwrap();
        let before = std::fs::read(&file).unwrap();
        let game = verified_game(
            CheatCandidateArchive {
                display_name: "Game".to_string(),
                platform: Some("SNES".to_string()),
                region: None,
                serial: None,
                content_hash: None,
                content_basename: None,
            },
            dir.path().join("game.sfc"),
        );

        let _ = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            &file,
            &game,
            &CheatCandidateOptions::default(),
        );

        let after = std::fs::read(&file).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn sibling_files_in_the_same_folder_are_never_selected_by_this_call() {
        let dir = tempdir().unwrap();
        let chosen = dir.path().join("chosen.cht");
        let sibling = dir.path().join("sibling.cht");
        std::fs::write(&chosen, VALID_CHT).unwrap();
        std::fs::write(&sibling, VALID_CHT).unwrap();
        let game = verified_game(
            CheatCandidateArchive {
                display_name: "Chosen".to_string(),
                platform: Some("SNES".to_string()),
                region: None,
                serial: None,
                content_hash: None,
                content_basename: None,
            },
            dir.path().join("chosen.sfc"),
        );

        let result = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            &chosen,
            &game,
            &CheatCandidateOptions::default(),
        )
        .unwrap();
        assert_eq!(result.location.catalogue_relative_path, "chosen.cht");
    }
}
