//! Where EmuWiz keeps a game's WHDLoad trainer options: the single storage
//! contract shared by the GUI, the CLI and the future FS-UAE launcher
//! (`docs/cheats/TRAINERS_AND_POKES_BACKEND.md`, "Option file storage").
//!
//! `<EmuWiz data dir>/whdload-trainer-options/<sha256(verified identity)>.options`
//!
//! - Keyed by the verified game identity, not by a path, so it survives a
//!   restart and the game moving; two different identities never share a
//!   file (SHA-256 of the identity text, hex).
//! - The file name is the digest alone, so no title or path text reaches the
//!   file system, and the folder lives in EmuWiz's own data directory, never
//!   beside original game media.
//! - Only a `Ready` discovery with a verified identity resolves; anything
//!   else (unverified, ambiguous, no slave, ...) is refused rather than
//!   guessed.
//! - Resolving reads and writes nothing. Preview, Apply and Undo go through
//!   the existing shared transaction, which never silently replaces a file
//!   it did not preview.

use std::fmt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::whdload_trainer_discovery::{WhdloadTrainerDiscovery, WhdloadTrainerDiscoveryState};

/// The folder under EmuWiz's data directory holding per-game option files.
pub const WHDLOAD_TRAINER_OPTION_FOLDER: &str = "whdload-trainer-options";
/// The option file extension.
pub const WHDLOAD_TRAINER_OPTION_EXTENSION: &str = "options";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhdloadTrainerStorageError {
    /// The discovery is not `Ready`; options cannot be stored for it.
    NotReady(WhdloadTrainerDiscoveryState),
    /// No verified game identity (or an empty one).
    IdentityUnverified,
    /// EmuWiz's data directory could not be resolved.
    DataDirectory(String),
}

impl fmt::Display for WhdloadTrainerStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady(state) => write!(
                f,
                "trainer options cannot be stored: {}",
                state.plain_label()
            ),
            Self::IdentityUnverified => {
                f.write_str("trainer options cannot be stored without a verified game identity")
            }
            Self::DataDirectory(error) => {
                write!(f, "EmuWiz's data folder could not be resolved: {error}")
            }
        }
    }
}

impl std::error::Error for WhdloadTrainerStorageError {}

/// The option file for a `Ready` discovery, in EmuWiz's data directory.
pub fn whdload_trainer_option_file(
    discovery: &WhdloadTrainerDiscovery,
) -> Result<PathBuf, WhdloadTrainerStorageError> {
    let folder = crate::app_dirs::data_path(WHDLOAD_TRAINER_OPTION_FOLDER)
        .map_err(|error| WhdloadTrainerStorageError::DataDirectory(error.to_string()))?;
    whdload_trainer_option_file_in(&folder, discovery)
}

/// The same resolution under an explicit folder (tests, tools).
pub fn whdload_trainer_option_file_in(
    folder: &Path,
    discovery: &WhdloadTrainerDiscovery,
) -> Result<PathBuf, WhdloadTrainerStorageError> {
    if discovery.state != WhdloadTrainerDiscoveryState::Ready {
        return Err(WhdloadTrainerStorageError::NotReady(discovery.state));
    }
    let identity = discovery
        .identity
        .as_ref()
        .map(|identity| identity.verified_game_identity.trim())
        .filter(|identity| !identity.is_empty())
        .ok_or(WhdloadTrainerStorageError::IdentityUnverified)?;
    let digest: String = Sha256::digest(identity.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(folder.join(format!("{digest}.{WHDLOAD_TRAINER_OPTION_EXTENSION}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::WhdloadTrainerIdentity;

    fn discovery(
        state: WhdloadTrainerDiscoveryState,
        identity: Option<&str>,
    ) -> WhdloadTrainerDiscovery {
        WhdloadTrainerDiscovery {
            state,
            detail: String::new(),
            slave_path: Some(PathBuf::from("/games/Turrican/Turrican.slave")),
            slave_sha256: Some("a".repeat(64)),
            slave_runtime_version: Some(17),
            declarations: Vec::new(),
            options: Vec::new(),
            identity: identity.map(|identity| WhdloadTrainerIdentity {
                slave_path: PathBuf::from("/games/Turrican/Turrican.slave"),
                expected_slave_sha256: "a".repeat(64),
                verified_game_identity: identity.into(),
                package_version: None,
            }),
            candidates: Vec::new(),
        }
    }

    #[test]
    fn the_file_is_keyed_by_identity_alone_and_never_by_title_or_path() {
        let folder = Path::new("/data/whdload-trainer-options");
        let ready = discovery(WhdloadTrainerDiscoveryState::Ready, Some("amiga:turrican2"));
        let path = whdload_trainer_option_file_in(folder, &ready).unwrap();
        assert_eq!(path.parent(), Some(folder));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 64 + ".options".len());
        assert!(name.ends_with(".options"));
        assert!(!name.contains("turrican") && !name.contains("Turrican"));
        // Stable when the game moves: the slave path does not take part.
        let mut moved = ready.clone();
        moved.slave_path = Some(PathBuf::from("/elsewhere/Turrican.slave"));
        moved.identity.as_mut().unwrap().slave_path = PathBuf::from("/elsewhere/Turrican.slave");
        assert_eq!(
            whdload_trainer_option_file_in(folder, &moved).unwrap(),
            path
        );
        // Different games never collide.
        let other = discovery(WhdloadTrainerDiscoveryState::Ready, Some("amiga:turrican"));
        assert_ne!(
            whdload_trainer_option_file_in(folder, &other).unwrap(),
            path
        );
    }

    #[test]
    fn anything_but_a_ready_verified_discovery_is_refused() {
        let folder = Path::new("/data/whdload-trainer-options");
        for state in [
            WhdloadTrainerDiscoveryState::IdentityUnverified,
            WhdloadTrainerDiscoveryState::AmbiguousSlaves,
            WhdloadTrainerDiscoveryState::NoSlave,
            WhdloadTrainerDiscoveryState::SlaveInsideHdf,
            WhdloadTrainerDiscoveryState::SlaveChanged,
        ] {
            assert_eq!(
                whdload_trainer_option_file_in(folder, &discovery(state, Some("amiga:x"))),
                Err(WhdloadTrainerStorageError::NotReady(state))
            );
        }
        for identity in [None, Some(""), Some("   ")] {
            assert_eq!(
                whdload_trainer_option_file_in(
                    folder,
                    &discovery(WhdloadTrainerDiscoveryState::Ready, identity)
                ),
                Err(WhdloadTrainerStorageError::IdentityUnverified)
            );
        }
    }

    #[test]
    fn resolving_reads_and_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join(WHDLOAD_TRAINER_OPTION_FOLDER);
        let ready = discovery(WhdloadTrainerDiscoveryState::Ready, Some("amiga:x"));
        let path = whdload_trainer_option_file_in(&folder, &ready).unwrap();
        assert!(!folder.exists());
        assert!(!path.exists());
    }
}
