//! Snapshot history for one live save location, plus the process observation
//! that snapshot creation requires.
//!
//! Everything here is read-only: manifests are listed and re-hashed and the
//! live save is compared with [`build_restore_plan`].
//! A snapshot is associated with a save only by its recorded original path
//! *and* the caller's explicit game identity; filenames are never matched.

use super::{
    EmulatorUseStatus, SaveArtifactType, SaveLocation, SaveQuiescenceRequirement,
    SaveRestoreConflict, SaveRestorePlan, SaveSnapshot, SaveSnapshotError, SaveSnapshotManifest,
    SaveSnapshotRequest, build_restore_plan, create_snapshot, create_snapshot_checked,
    verify_snapshot,
};
use crate::memory_card_inventory::restore_guard::{
    CardLeaseAttempt, CardWriteLease, ProcScanQuiescence, observe_card_now, ps2_card_binding,
};
use crate::persistent_state_inventory::StateEmulator;
use crate::save_snapshots::tree_restore::safety::{
    DirectorySaveBinding, EmulatorQuiescence, QuiescenceProvider, ResolvedSaveGame,
};
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
/// process observer: PCSX2 and RetroArch
/// ([`ProcScanQuiescence`]: emulator executables plus open descriptors on the
/// save). Other emulators always report `Unknown`, so snapshot creation is
/// refused for them; neither names alone nor the absence of an EmuWiz-tracked
/// launch is treated as proof that an emulator is closed.
pub fn snapshot_closure_supported(emulator: StateEmulator) -> bool {
    matches!(emulator, StateEmulator::Pcsx2 | StateEmulator::RetroArch)
}

/// The observer binding for one save file. Its `emulator` selects which
/// process names the scan treats as "running".
pub fn closure_binding(emulator: StateEmulator, save_path: &Path) -> DirectorySaveBinding {
    if emulator == StateEmulator::RetroArch {
        DirectorySaveBinding {
            game: ResolvedSaveGame::Unique("save file".to_string()),
            emulator: "retroarch".to_string(),
            profile: save_path.to_string_lossy().into_owned(),
            artifact_type: SaveArtifactType::Sram,
        }
    } else {
        ps2_card_binding(save_path, "save snapshot")
    }
}

/// Current closure of the emulator for one save file, as the restore needs
/// it. Anything but a proved `Closed` is a refusal.
pub fn restore_quiescence(
    emulator: StateEmulator,
    save_path: &Path,
    observer: &dyn QuiescenceProvider,
) -> SaveQuiescenceRequirement {
    if !snapshot_closure_supported(emulator) {
        return SaveQuiescenceRequirement::Unknown;
    }
    match observe_card_now(observer, &closure_binding(emulator, save_path), save_path) {
        EmulatorQuiescence::Closed => SaveQuiescenceRequirement::ConfirmedClosed,
        EmulatorQuiescence::Running => SaveQuiescenceRequirement::Running,
        EmulatorQuiescence::CardInUse => SaveQuiescenceRequirement::InUse,
        EmulatorQuiescence::Unknown => SaveQuiescenceRequirement::Unknown,
    }
}

/// Emulator-use evidence for `save_path`, for showing before a snapshot.
/// `NotDetected` only when the reviewed observer reports `Closed`. Observed
/// under a kernel lease held for this check only (see `observe_card_now`).
/// Creating the snapshot re-checks with [`create_snapshot_observed`].
pub fn snapshot_emulator_use(
    emulator: StateEmulator,
    save_path: &Path,
    observer: &dyn QuiescenceProvider,
) -> EmulatorUseStatus {
    if !snapshot_closure_supported(emulator) {
        return EmulatorUseStatus::Unknown;
    }
    match observe_card_now(observer, &closure_binding(emulator, save_path), save_path) {
        EmulatorQuiescence::Closed => EmulatorUseStatus::NotDetected,
        EmulatorQuiescence::Running => EmulatorUseStatus::Running,
        EmulatorQuiescence::CardInUse => EmulatorUseStatus::CardInUse,
        EmulatorQuiescence::Unknown => EmulatorUseStatus::Unknown,
    }
}

fn emulator_use(state: EmulatorQuiescence) -> EmulatorUseStatus {
    match state {
        EmulatorQuiescence::Closed => EmulatorUseStatus::NotDetected,
        EmulatorQuiescence::Running => EmulatorUseStatus::Running,
        EmulatorQuiescence::CardInUse => EmulatorUseStatus::CardInUse,
        EmulatorQuiescence::Unknown => EmulatorUseStatus::Unknown,
    }
}

/// Check emulator use now and create the snapshot (`request.emulator_use` is
/// replaced). For a PCSX2 card file on a lease-capable local filesystem, a
/// kernel lease is held from the check through the copy, and the snapshot is
/// published only if no program opened the card for writing meanwhile. So a
/// torn copy is never kept, whatever the card's timestamps say. Without lease
/// support the strict process scan decides and the copy relies on
/// `create_snapshot`'s size and modification-time recheck, as before.
pub fn create_snapshot_observed(
    emulator: StateEmulator,
    mut request: SaveSnapshotRequest,
    observer: &dyn QuiescenceProvider,
) -> Result<SaveSnapshot, SaveSnapshotError> {
    if !snapshot_closure_supported(emulator) {
        request.emulator_use = EmulatorUseStatus::Unknown;
        return create_snapshot(&request);
    }
    let path = request.location.path.clone();
    let binding = closure_binding(emulator, &path);
    match CardWriteLease::acquire(&path, None) {
        CardLeaseAttempt::Held(lease) => {
            request.emulator_use =
                emulator_use(observer.observe_under_card_lease(&binding, &lease));
            create_snapshot_checked(&request, &|| lease.intact()).map_err(|error| {
                // A program opened the card for writing during the copy.
                if matches!(error, SaveSnapshotError::SourceChanged(_)) && !lease.intact() {
                    SaveSnapshotError::CardInUse
                } else {
                    error
                }
            })
        }
        CardLeaseAttempt::Writers => {
            request.emulator_use = EmulatorUseStatus::CardInUse;
            create_snapshot(&request)
        }
        CardLeaseAttempt::Changed | CardLeaseAttempt::Unavailable => {
            request.emulator_use = emulator_use(observer.observe(&binding));
            create_snapshot(&request)
        }
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
