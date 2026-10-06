//! Which files an audit verifies.
//!
//! Historically an audit walked the whole scan folder and hashed every regular
//! file in it. A real GBA library folder held ~27,000 files of which ~3,700
//! were games; the rest was artwork and PDF manuals, so verification read
//! tens of gigabytes of non-game files and took hours.
//!
//! Two layers fix that, neither special-cased to a platform or a file type
//! the platform "owns":
//!
//! 1. **Support files are never candidates** in the games-only walk
//!    ([`AuditTargets::FolderWalkGamesOnly`]). It skips files the
//!    ingestion layer already classifies as known non-game support material
//!    ([`crate::ingestion::discovery::is_known_non_game_extension`]: images,
//!    manuals, metadata sidecars, video/audio extras). That list is
//!    conservative by construction - it never contains an extension with game
//!    meaning (`md` is deliberately absent). A file passed *directly* as the
//!    scan target is always audited: that is an explicit choice.
//! 2. **A platform audit verifies the catalogue's game units.** When the
//!    caller supplies catalogue rows for the platform, the target set is those
//!    rows' files - not whatever else happens to sit beside them. A catalogued
//!    CUE or GDI contributes its descriptor *and* the track files it
//!    references (the DAT hashes the tracks), resolved by the existing CUE/GDI
//!    resolvers. A catalogued archive or CHD is one target; archive members
//!    are still enumerated by the archive pass. A catalogue row whose file is
//!    gone is counted as unavailable, never silently dropped.
//!
//! Unknown files are not discarded from health reporting - they are simply a
//! different concept from verification targets, and remain visible through
//! Library Files Check.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Where an audit's candidate files came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditPopulationBasis {
    /// A folder walk. Whether known support files were hashed is told by
    /// `support_files_skipped` (zero for the legacy full walk).
    #[default]
    FolderWalk,
    /// Exactly the catalogue's game units for the platform.
    Catalogue,
}

/// How the audit's candidate population was chosen, for honest reporting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AuditPopulation {
    pub basis: AuditPopulationBasis,
    /// Catalogue rows that contributed a file to verify.
    pub catalogue_rows: usize,
    /// Catalogue rows whose file was not on disk. They are not verified and are
    /// reported as unavailable, never as "no match".
    pub catalogue_rows_unavailable: usize,
    /// CUE/GDI track files added because a catalogued descriptor names them.
    pub companion_files: usize,
    /// Known support files (artwork, manuals, metadata) the games-only walk did
    /// not hash. Zero for the legacy full walk and in catalogue mode.
    pub support_files_skipped: usize,
}

/// The catalogue-derived verification targets for one platform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogueAuditTargets {
    /// Sorted, de-duplicated physical files to verify.
    pub files: Vec<PathBuf>,
    pub catalogue_rows: usize,
    pub catalogue_rows_unavailable: usize,
    pub companion_files: usize,
}

/// What an audit run verifies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AuditTargets {
    /// Every regular file under the scan folder. This is the long-standing
    /// behaviour and stays the default for callers that account for
    /// ancillary files themselves (the repair report counts artwork and
    /// manuals as "ignored ancillary" and needs to see them).
    #[default]
    FolderWalk,
    /// As [`Self::FolderWalk`], but known support files (artwork, manuals,
    /// metadata, video/audio extras) are counted and not hashed. Verification
    /// paths that only care about games choose this.
    FolderWalkGamesOnly,
    /// Verify exactly these catalogue game units.
    Catalogue(CatalogueAuditTargets),
}

/// Builds the target set from catalogue row paths. Only rows inside
/// `scan_root` count (the whole folder, or exactly the file when `scan_root`
/// is a file). Reads at most the CUE/GDI descriptors; no game content.
pub fn catalogue_audit_targets(
    row_paths: impl IntoIterator<Item = PathBuf>,
    scan_root: &Path,
) -> CatalogueAuditTargets {
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();
    let mut rows = 0usize;
    let mut unavailable = 0usize;
    let mut companions: BTreeSet<PathBuf> = BTreeSet::new();
    // Two catalogue rows for one path are one game unit on disk.
    let row_paths: BTreeSet<PathBuf> = row_paths.into_iter().collect();
    for path in row_paths {
        if !path.starts_with(scan_root) {
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                rows += 1;
                if let Some((_, referenced)) =
                    crate::catalogue_reconciliation::descriptor_companions(&path)
                {
                    for companion in referenced {
                        if std::fs::symlink_metadata(&companion).is_ok_and(|m| m.is_file()) {
                            companions.insert(companion);
                        }
                    }
                }
                files.insert(path);
            }
            // Gone, or no longer readable: not verifiable, and said so.
            Err(_) => unavailable += 1,
            // A directory or other object is not a loose verification target
            // (arcade sets and similar keep their own folder-walk audit).
            Ok(_) => {}
        }
    }
    let companion_files = companions.difference(&files).count();
    files.extend(companions);
    CatalogueAuditTargets {
        files: files.into_iter().collect(),
        catalogue_rows: rows,
        catalogue_rows_unavailable: unavailable,
        companion_files,
    }
}

/// Whether the folder walk should skip `path` as a known support file.
pub(crate) fn is_support_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            crate::ingestion::discovery::is_known_non_game_extension(
                &extension.to_ascii_lowercase(),
            )
        })
}

#[cfg(test)]
mod tests;
