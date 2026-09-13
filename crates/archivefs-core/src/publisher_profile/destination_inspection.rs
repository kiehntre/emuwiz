//! Read-only inspection of an existing destination path - task section 16.
//!
//! Every function here only ever *reads* filesystem metadata
//! (`symlink_metadata`/`read_link`/`exists`). Nothing is created, written,
//! renamed, or removed. Called only when a caller explicitly supplies a
//! destination root to inspect; with no root, every item's
//! [`super::model::DestinationState`] stays [`super::model::DestinationState::Unknown`]
//! rather than a guessed "missing".

use std::path::Path;

use super::model::DestinationState;

/// Inspects one planned destination path against what (if anything)
/// already exists there, read-only.
pub fn inspect_destination(planned_destination: &Path, planned_source: &Path) -> DestinationState {
    let metadata = match std::fs::symlink_metadata(planned_destination) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return DestinationState::Missing;
        }
        Err(_) => return DestinationState::Unknown,
    };
    if metadata.file_type().is_symlink() {
        return match std::fs::read_link(planned_destination) {
            Ok(target) => {
                if !target.exists() {
                    return DestinationState::Stale;
                }
                if paths_match(&target, planned_source) {
                    DestinationState::AlreadyCorrect
                } else {
                    DestinationState::Conflicting
                }
            }
            Err(_) => DestinationState::Unknown,
        };
    }
    // A regular file (or directory) already occupies this destination.
    // Phase 1 never hashes/compares content - that is a Phase 2 execution
    // concern - so this is honestly `Conflicting`, never assumed correct.
    DestinationState::Conflicting
}

/// Compares a symlink's stored target against the planned source path.
/// Both are compared as given (Phase 1 never canonicalizes a source path,
/// since canonicalizing can itself touch the filesystem in surprising ways
/// on some platforms) - an exact-string match after this crate's own
/// existing lossless path normalization is sufficient for the same-path
/// bind contracts this codebase already documents
/// ([`crate::playing_library::romm_projection::RommVisibility`]).
fn paths_match(a: &Path, b: &Path) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn missing_destination_is_reported_missing() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("does-not-exist.rom");
        let source = dir.path().join("source.rom");
        assert_eq!(
            inspect_destination(&destination, &source),
            DestinationState::Missing
        );
    }

    #[test]
    fn matching_symlink_is_already_correct() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.rom");
        fs::write(&source, b"data").unwrap();
        let destination = dir.path().join("dest.rom");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&source, &destination).unwrap();
        #[cfg(unix)]
        assert_eq!(
            inspect_destination(&destination, &source),
            DestinationState::AlreadyCorrect
        );
    }

    #[test]
    fn symlink_to_a_different_source_is_conflicting() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.rom");
        let other = dir.path().join("other.rom");
        fs::write(&source, b"data").unwrap();
        fs::write(&other, b"data").unwrap();
        let destination = dir.path().join("dest.rom");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&other, &destination).unwrap();
        #[cfg(unix)]
        assert_eq!(
            inspect_destination(&destination, &source),
            DestinationState::Conflicting
        );
    }

    #[test]
    fn dangling_symlink_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.rom");
        let missing_target = dir.path().join("gone.rom");
        let destination = dir.path().join("dest.rom");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&missing_target, &destination).unwrap();
        #[cfg(unix)]
        assert_eq!(
            inspect_destination(&destination, &source),
            DestinationState::Stale
        );
    }

    #[test]
    fn regular_file_at_destination_is_conflicting() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.rom");
        let destination = dir.path().join("dest.rom");
        fs::write(&destination, b"already here").unwrap();
        assert_eq!(
            inspect_destination(&destination, &source),
            DestinationState::Conflicting
        );
    }

    #[test]
    fn inspection_never_writes_anything() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("dest.rom");
        let source = dir.path().join("source.rom");
        let _ = inspect_destination(&destination, &source);
        assert!(!destination.exists(), "inspection must never create a file");
    }
}
