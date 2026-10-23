//! Snapshot history for one live save location, plus the process observation
//! that snapshot creation requires.
//!
//! Everything here is read-only: manifests are listed and re-hashed and the
//! live save is compared with [`build_restore_plan`].
//! A snapshot is associated with a save only by its recorded original path
//! *and* the caller's explicit game identity; filenames are never matched.

use super::{
    EmulatorUseStatus, SaveLocation, SaveRestoreConflict, SaveRestorePlan, SaveSnapshot,
    SaveSnapshotManifest, build_restore_plan, verify_snapshot,
};
use crate::memory_card_inventory::restore_guard::{ProcScanQuiescence, ps2_card_binding};
use crate::persistent_state_inventory::StateEmulator;
use crate::save_snapshots::tree_restore::safety::{EmulatorQuiescence, QuiescenceProvider};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// Upper bound on manifests read from one snapshot root.
pub const MAX_LISTED_SNAPSHOTS: usize = 2_048;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotListing {
    pub snapshots: Vec<SaveSnapshot>,
    /// Unreadable or malformed entries. They are reported, never repaired.
    pub warnings: Vec<String>,
}

/// Lists complete published snapshots under `root/<game>/<snapshot>`.
/// Hidden `.partial-*` staging directories and symlinks are skipped. A
/// missing root is an empty history, not an error.
pub fn list_snapshots(root: &Path) -> SnapshotListing {
    let mut listing = SnapshotListing::default();
    let games = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return listing,
        Err(error) => {
            listing
                .warnings
                .push(format!("{}: {error}", root.display()));
            return listing;
        }
    };
    let mut game_dirs = games
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| visible_directory(path))
        .collect::<Vec<_>>();
    game_dirs.sort();
    for game_dir in game_dirs {
        let Ok(entries) = fs::read_dir(&game_dir) else {
            listing
                .warnings
                .push(format!("{}: could not be read", game_dir.display()));
            continue;
        };
        let mut snapshot_dirs = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| visible_directory(path))
            .collect::<Vec<_>>();
        snapshot_dirs.sort();
        for storage_path in snapshot_dirs {
            if listing.snapshots.len() >= MAX_LISTED_SNAPSHOTS {
                listing.warnings.push(format!(
                    "Only the first {MAX_LISTED_SNAPSHOTS} snapshots were listed."
                ));
                return listing;
            }
            match read_manifest(&storage_path.join("manifest.json")) {
                Ok(manifest) => listing.snapshots.push(SaveSnapshot {
                    manifest,
                    storage_path,
                }),
                Err(detail) => listing
                    .warnings
                    .push(format!("{}: {detail}", storage_path.display())),
            }
        }
    }
    listing
}

fn visible_directory(path: &Path) -> bool {
    let hidden = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_none_or(|name| name.starts_with('.'));
    !hidden
        && fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

fn read_manifest(path: &Path) -> Result<SaveSnapshotManifest, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("manifest is not a regular file".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("manifest is larger than the safety limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("manifest is unreadable: {error}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotIntegrity {
    /// Every artifact still matches its recorded size and SHA-256.
    Verified,
    Failed(String),
}

/// How the live save relates to this snapshot right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotFreshness {
    MatchesCurrentSave,
    /// The live save was modified after the snapshot: the snapshot is older.
    CurrentSaveNewer,
    CurrentSaveDiffers,
    CurrentSaveMissing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotHistoryEntry {
    pub snapshot: SaveSnapshot,
    pub integrity: SnapshotIntegrity,
    pub freshness: SnapshotFreshness,
    /// Emulator/profile/completeness conflicts from the existing planner.
    pub compatibility_conflicts: Vec<SaveRestoreConflict>,
    pub plan: SaveRestorePlan,
}

impl SnapshotHistoryEntry {
    pub fn compatible(&self) -> bool {
        self.compatibility_conflicts.is_empty() && self.integrity == SnapshotIntegrity::Verified
    }
}

/// Snapshots of exactly `location.path` recorded for exactly `game_identity`,
/// newest first, each re-verified and compared with the live save.
pub fn history_for_location(
    listing: &SnapshotListing,
    location: &SaveLocation,
    game_identity: &str,
) -> Vec<SnapshotHistoryEntry> {
    let mut entries = listing
        .snapshots
        .iter()
        .filter(|snapshot| {
            snapshot.manifest.original_save_path == location.path
                && snapshot.manifest.game_identity.as_deref() == Some(game_identity)
        })
        .map(|snapshot| {
            let integrity = match verify_snapshot(snapshot) {
                Ok(()) => SnapshotIntegrity::Verified,
                Err(error) => SnapshotIntegrity::Failed(error.to_string()),
            };
            // Emulator use is irrelevant to a comparison; it is assessed
            // separately by whoever acts on the result.
            let plan = build_restore_plan(snapshot, location, EmulatorUseStatus::NotDetected);
            let freshness = if !location.path.exists() {
                SnapshotFreshness::CurrentSaveMissing
            } else if plan.current_save_newer {
                SnapshotFreshness::CurrentSaveNewer
            } else if plan.files_to_add.is_empty() && plan.files_to_replace.is_empty() {
                SnapshotFreshness::MatchesCurrentSave
            } else {
                SnapshotFreshness::CurrentSaveDiffers
            };
            let compatibility_conflicts = plan
                .conflicts
                .iter()
                .filter(|conflict| {
                    matches!(
                        conflict,
                        SaveRestoreConflict::EmulatorMismatch
                            | SaveRestoreConflict::ProfileMismatch
                            | SaveRestoreConflict::SnapshotIncomplete
                            | SaveRestoreConflict::SnapshotUnreadable
                    )
                })
                .cloned()
                .collect();
            SnapshotHistoryEntry {
                snapshot: snapshot.clone(),
                integrity,
                freshness,
                compatibility_conflicts,
                plan,
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        b.snapshot
            .manifest
            .snapshot_unix_seconds
            .cmp(&a.snapshot.manifest.snapshot_unix_seconds)
            .then_with(|| b.snapshot.storage_path.cmp(&a.snapshot.storage_path))
    });
    entries
}

/// Emulators whose closure can be established by an existing, reviewed
/// process observer. Only PCSX2 has one today
/// ([`ProcScanQuiescence`]: emulator executables plus open descriptors on the
/// save). Other emulators always report `Unknown`, so snapshot creation is
/// refused for them; neither names alone nor the absence of an EmuWiz-tracked
/// launch is treated as proof that an emulator is closed.
pub fn snapshot_closure_supported(emulator: StateEmulator) -> bool {
    emulator == StateEmulator::Pcsx2
}

/// Emulator-use evidence for snapshot creation of `save_path`.
/// `NotDetected` only when the reviewed observer reports `Closed`.
pub fn snapshot_emulator_use(
    emulator: StateEmulator,
    save_path: &Path,
    observer: &dyn QuiescenceProvider,
) -> EmulatorUseStatus {
    if !snapshot_closure_supported(emulator) {
        return EmulatorUseStatus::Unknown;
    }
    match observer.observe(&ps2_card_binding(save_path, "save snapshot")) {
        EmulatorQuiescence::Closed => EmulatorUseStatus::NotDetected,
        EmulatorQuiescence::Running => EmulatorUseStatus::Running,
        EmulatorQuiescence::Unknown => EmulatorUseStatus::Unknown,
    }
}

/// The production observer used by [`snapshot_emulator_use`].
pub fn default_closure_observer() -> ProcScanQuiescence {
    ProcScanQuiescence::new()
}

/// The location's nearest existing ancestor, for a free-space reading before
/// the snapshot root has been created.
pub fn nearest_existing_directory(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.is_dir())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests;
