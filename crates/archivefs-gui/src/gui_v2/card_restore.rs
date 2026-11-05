//! Restore one save folder from a PCSX2 memory-card snapshot (Blue Saves page).
//!
//! A memory card holds saves for many games, so a card snapshot is never
//! written back whole (Core's generic restore refuses shared containers, and
//! this module does not change that). Instead the person picks one save folder
//! that the snapshot actually contains, and the existing Core pieces do the
//! rest, unchanged:
//!
//! ```text
//! verify snapshot -> inspect snapshot card -> plan_ps2_psu_export /
//!   apply_ps2_psu_export (one folder -> new PSU; the snapshot is only read)
//!   -> plan_ps2_psu_restore (live card preflight, read-only)
//!   -> apply_ps2_psu_restore_guarded (closure, backup, journal, exchange)
//!   -> undo_ps2_psu_restore_guarded
//! ```
//!
//! Card snapshots made here carry no game identity: card folder names are
//! only candidate evidence, so nothing is assigned to a game. Interrupted
//! restores share Gold's journal directory and are recovered by the Save
//! Vault's existing "Check interrupted restores".

use super::save_snapshots::save_location;
use crate::pcsx2_page::psu_restore_error_message;
use archivefs_core::{
    memory_card_inventory::{
        MemoryCardFormat, MemoryCardHealth, MemoryCardInventory, Ps2PsuRestorePlan,
        Ps2SaveDirectory, apply_ps2_psu_export, inspect_memory_card, plan_ps2_psu_export,
        plan_ps2_psu_restore,
        restore_guard::{
            Ps2GuardedRestore, Ps2RestoreGuardOptions, apply_ps2_psu_restore_guarded,
            undo_ps2_psu_restore_guarded,
        },
    },
    persistent_state_inventory::{PersistentStateRecord, PersistentStateType, StateEmulator},
    save_snapshots::{
        EmulatorUseStatus, SaveArtifactType, SaveLocation, SaveSnapshot, SaveSnapshotCompleteness,
        SaveSnapshotError, SaveSnapshotRequest, create_snapshot,
        history::{list_snapshots, nearest_existing_directory, snapshot_emulator_use},
        tree_restore::safety::{FilesystemSpace, QuiescenceProvider, RestoreSpaceProvider},
        verify_snapshot,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) mod panel;
pub(super) mod recovery;
pub(super) mod storage;
#[cfg(test)]
mod tests;

/// Why savestates are never restored, here or on game pages.
pub(super) const SAVESTATE_RESTORE_UNSUPPORTED: &str = "Savestates cannot be restored. A savestate only works with the exact emulator version and settings that made it, and EmuWiz has no reviewed way to check that. Your savestate snapshots are kept unchanged.";

/// Where a game page sends people who want a memory-card save back.
pub(super) const CARD_RESTORE_WHERE: &str = "Memory-card saves are restored one save folder at a time from a card snapshot. Open Saves & States (all saves) and use Restore a save folder on the PCSX2 memory card.";

static NEXT_WORK_FILE: AtomicU64 = AtomicU64::new(0);

/// The live PCSX2 card a record names, or why it cannot be offered.
pub(super) fn card_location(
    record: &PersistentStateRecord,
    all: &[PersistentStateRecord],
) -> Result<SaveLocation, &'static str> {
    if record.state_type != PersistentStateType::MemoryCard {
        return Err("Only memory cards can have a save folder restored.");
    }
    if record.emulator != StateEmulator::Pcsx2 {
        return Err(
            "Restoring a save folder from a snapshot is only available for PCSX2 memory cards.",
        );
    }
    if record.container_path.is_some() {
        return Err("This is an entry inside a card, not the card itself.");
    }
    if !record.warnings.is_empty() {
        return Err("This memory card needs review first. Check the warnings in Advanced details.");
    }
    let same_card = all
        .iter()
        .filter(|other| {
            other.state_type == PersistentStateType::MemoryCard
                && other.container_path.is_none()
                && other.path == record.path
        })
        .count();
    if same_card != 1 {
        return Err(
            "This memory card is listed more than once, so EmuWiz cannot tell which emulator setup it belongs to. Nothing will be restored to it.",
        );
    }
    Ok(save_location(record, SaveArtifactType::PsMemoryCard))
}

/// One snapshot of a card, with its integrity re-checked now.
#[derive(Clone, Debug)]
pub(super) struct CardSnapshot {
    pub(super) snapshot: SaveSnapshot,
    pub(super) integrity: Result<(), String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CardHistory {
    pub(super) snapshots: Vec<CardSnapshot>,
    pub(super) emulator_use: Option<EmulatorUseStatus>,
    pub(super) warnings: Vec<String>,
}

fn is_card_snapshot_of(snapshot: &SaveSnapshot, location: &SaveLocation) -> bool {
    let manifest = &snapshot.manifest;
    manifest.original_save_path == location.path
        && manifest.game_identity.is_none()
        && manifest.artifact_type == SaveArtifactType::PsMemoryCard
        && manifest.emulator == location.emulator
        && manifest.emulator_profile == location.profile
}

/// Worker: this card's snapshots (newest first) and PCSX2 closure now.
pub(super) fn load_card_history(
    root: &Path,
    location: &SaveLocation,
    observer: &dyn QuiescenceProvider,
) -> CardHistory {
    let listing = list_snapshots(root);
    let mut snapshots = listing
        .snapshots
        .into_iter()
        .filter(|snapshot| is_card_snapshot_of(snapshot, location))
        .map(|snapshot| CardSnapshot {
            integrity: verify_snapshot(&snapshot).map_err(|error| error.to_string()),
            snapshot,
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|a, b| {
        b.snapshot
            .manifest
            .snapshot_unix_seconds
            .cmp(&a.snapshot.manifest.snapshot_unix_seconds)
            .then_with(|| b.snapshot.storage_path.cmp(&a.snapshot.storage_path))
    });
    CardHistory {
        snapshots,
        emulator_use: Some(snapshot_emulator_use(
            StateEmulator::Pcsx2,
            &location.path,
            observer,
        )),
        warnings: listing.warnings,
    }
}

/// Worker: snapshot the whole card (read-only), tied to no game.
pub(super) fn create_card_snapshot(
    root: &Path,
    location: &SaveLocation,
    observer: &dyn QuiescenceProvider,
) -> Result<String, String> {
    let emulator_use = snapshot_emulator_use(StateEmulator::Pcsx2, &location.path, observer);
    let available_space_bytes =
        nearest_existing_directory(root).and_then(|dir| FilesystemSpace.available_bytes(&dir).ok());
    create_snapshot(&SaveSnapshotRequest {
        location: location.clone(),
        game_identity: None,
        platform: Some("PS2".into()),
        storage_root: root.to_path_buf(),
        available_space_bytes,
        snapshot_id: None,
        now_unix_seconds: None,
        emulator_use,
    })
    .map(|snapshot| snapshot.manifest.snapshot_id)
    .map_err(|error| match error {
        SaveSnapshotError::EmulatorRunning => {
            "PCSX2 is running. Close it, then try again. Nothing was copied.".to_string()
        }
        SaveSnapshotError::EmulatorUseUnknown => {
            "EmuWiz could not confirm PCSX2 is closed, so nothing was copied.".to_string()
        }
        SaveSnapshotError::SourceChanged(_) => {
            "The card changed while it was being copied. Nothing was kept; close PCSX2 and try again.".to_string()
        }
        SaveSnapshotError::InsufficientSpace { .. } => {
            "There is not enough free space for this snapshot. Nothing was copied.".to_string()
        }
        other => format!("The snapshot was not created. Your card was not changed. ({other})"),
    })
}

/// The verified snapshot card file, after re-checking that the snapshot is
/// complete, unchanged and was made from exactly this live card.
fn snapshot_card_file(snapshot: &SaveSnapshot, location: &SaveLocation) -> Result<PathBuf, String> {
    let manifest = &snapshot.manifest;
    if !is_card_snapshot_of(snapshot, location) {
        return Err("This snapshot was not made from this memory card and emulator setup, so it will not be restored here.".into());
    }
    if manifest.completeness != SaveSnapshotCompleteness::Complete || manifest.artifacts.len() != 1
    {
        return Err("This snapshot is incomplete, so nothing can be restored from it.".into());
    }
    verify_snapshot(snapshot).map_err(|error| {
        format!("This snapshot no longer matches its recorded checksums, so nothing can be restored from it. ({error})")
    })?;
    let relative = &manifest.artifacts[0].relative_path;
    if relative.components().count() != 1 {
        return Err("This snapshot's file layout is not a single card image.".into());
    }
    let file = snapshot.storage_path.join("files").join(relative);
    match fs::symlink_metadata(&file) {
        Ok(metadata) if metadata.is_file() => Ok(file),
        _ => Err("The snapshot's card image is missing or is not a regular file.".into()),
    }
}

fn supported_card(card: &MemoryCardInventory, what: &str) -> Result<(), String> {
    if card.format != MemoryCardFormat::Ps2
        || !card.shared_container
        || card.ps2_inventory.is_none()
        || card.ps2_geometry.is_none()
    {
        return Err(format!(
            "The {what} is not a PS2 memory card format EmuWiz can read safely."
        ));
    }
    if card.health != MemoryCardHealth::Healthy {
        return Err(format!(
            "The {what} has problems EmuWiz will not work around. Nothing was changed."
        ));
    }
    Ok(())
}

/// One save folder found inside a snapshot card. The name is the card's own
/// folder name; it is never treated as proof of which game it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SnapshotFolder {
    pub(super) raw_name: Vec<u8>,
    pub(super) display_name: String,
    pub(super) files: Vec<(String, u64)>,
    pub(super) problem: Option<String>,
}

fn folder_problem(save: &Ps2SaveDirectory) -> Option<String> {
    (!save.warnings.is_empty()
        || !save.entry.warnings.is_empty()
        || !save.chain_health.complete
        || !save.chain_health.warnings.is_empty())
    .then(|| "This folder is damaged or incomplete in the snapshot and cannot be restored.".into())
}

/// Worker: the save folders the snapshot card actually contains. Read-only.
pub(super) fn snapshot_folders(
    snapshot: &SaveSnapshot,
    location: &SaveLocation,
) -> Result<Vec<SnapshotFolder>, String> {
    let file = snapshot_card_file(snapshot, location)?;
    let card = inspect_memory_card(&file)
        .map_err(|error| format!("The snapshot card could not be read: {error}"))?;
    supported_card(&card, "snapshot card")?;
    let inventory = card.ps2_inventory.as_ref().expect("checked above");
    Ok(inventory
        .save_directories
        .iter()
        .map(|save| SnapshotFolder {
            raw_name: save.entry.raw_name.clone(),
            display_name: save.entry.display_name.clone(),
            files: save
                .files
                .iter()
                .map(|file| (file.entry.display_name.clone(), file.declared_size_bytes))
                .collect(),
            problem: folder_problem(save),
        })
        .collect())
}

/// Managed working folders: exported PSUs and pre-restore card backups.
#[derive(Clone, Debug)]
pub(super) struct WorkDirs {
    pub(super) psu_dir: PathBuf,
    pub(super) backup_dir: PathBuf,
}

impl WorkDirs {
    #[cfg(not(test))]
    pub(super) fn default_dirs() -> Option<Self> {
        let base = archivefs_core::app_dirs::data_path("save-snapshot-restore").ok()?;
        Some(Self {
            psu_dir: base.join("exported-folders"),
            backup_dir: base.join("card-backups"),
        })
    }
}

/// Everything the confirmation step shows, plus the reviewed Core plan.
#[derive(Clone, Debug)]
pub(super) struct RestorePreview {
    pub(super) plan: Ps2PsuRestorePlan,
    pub(super) snapshot_id: String,
    pub(super) snapshot_unix_seconds: u64,
    pub(super) folder: String,
    pub(super) files: Vec<(String, u64)>,
}

/// Worker: export the chosen folder from the snapshot into a new PSU and
/// build the guarded restore plan for the live card. The live card is only
/// read. On refusal the PSU this call created is removed.
pub(super) fn prepare_restore(
    snapshot: &SaveSnapshot,
    location: &SaveLocation,
    folder_raw_name: &[u8],
    dirs: &WorkDirs,
    now_unix_seconds: u64,
) -> Result<RestorePreview, String> {
    let file = snapshot_card_file(snapshot, location)?;
    for dir in [&dirs.psu_dir, &dirs.backup_dir] {
        fs::create_dir_all(dir)
            .map_err(|error| format!("EmuWiz could not prepare its restore folder: {error}"))?;
    }
    let token = format!(
        "{}-{now_unix_seconds}-{}-{}",
        sanitize(&snapshot.manifest.snapshot_id),
        std::process::id(),
        NEXT_WORK_FILE.fetch_add(1, Ordering::Relaxed)
    );
    // Folder listing and exported bytes both come from one private copy
    // whose hash matches the manifest, so a snapshot file changed between
    // two reads cannot mix layouts and bytes.
    let copy = dirs.psu_dir.join(format!("{token}.snapshot-card"));
    let result = private_copy(&file, &copy, &snapshot.manifest.artifacts[0].sha256)
        .and_then(|()| prepare_from_copy(snapshot, location, folder_raw_name, dirs, &copy, &token));
    let _ = fs::remove_file(&copy);
    result
}

/// Copy the snapshot card into a new file EmuWiz owns and check its hash.
fn private_copy(source: &Path, copy: &Path, expected_sha256: &str) -> Result<(), String> {
    use std::io::Write;
    let bytes = fs::read(source)
        .map_err(|error| format!("The snapshot card could not be read: {error}"))?;
    if sha256_hex(&bytes) != expected_sha256 {
        return Err(
            "The snapshot no longer matches its recorded checksum. Nothing was changed.".into(),
        );
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(copy)
        .map_err(|error| format!("EmuWiz could not prepare its restore folder: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("EmuWiz could not prepare its restore folder: {error}"))
}

fn prepare_from_copy(
    snapshot: &SaveSnapshot,
    location: &SaveLocation,
    folder_raw_name: &[u8],
    dirs: &WorkDirs,
    copy: &Path,
    token: &str,
) -> Result<RestorePreview, String> {
    let card = inspect_memory_card(copy)
        .map_err(|error| format!("The snapshot card could not be read: {error}"))?;
    supported_card(&card, "snapshot card")?;
    let matches = card
        .ps2_inventory
        .as_ref()
        .expect("checked above")
        .save_directories
        .iter()
        .filter(|save| save.entry.raw_name == folder_raw_name)
        .collect::<Vec<_>>();
    let save = match matches.as_slice() {
        [save] => *save,
        [] => return Err("That save folder is no longer in this snapshot. Choose again.".into()),
        _ => {
            return Err(
                "This snapshot holds more than one folder with that name, so EmuWiz cannot tell which to restore.".into(),
            );
        }
    };
    if let Some(problem) = folder_problem(save) {
        return Err(problem);
    }
    let psu = dirs.psu_dir.join(format!("{token}.psu"));
    let export = plan_ps2_psu_export(&card, save, &psu)
        .and_then(|plan| apply_ps2_psu_export(&plan))
        .map_err(|error| format!("This folder cannot be taken out of the snapshot: {error}"))?;
    let finish = || -> Result<RestorePreview, String> {
        // The exported bytes came from the exact card the manifest recorded.
        if Some(export.source_card_sha256.as_str())
            != snapshot
                .manifest
                .artifacts
                .first()
                .map(|a| a.sha256.as_str())
        {
            return Err(
                "The snapshot changed while the folder was being read. Nothing was changed.".into(),
            );
        }
        let live = match fs::symlink_metadata(&location.path) {
            Ok(metadata) if metadata.is_file() => inspect_memory_card(&location.path)
                .map_err(|error| format!("The memory card could not be read: {error}"))?,
            Ok(_) => {
                return Err(
                    "The memory card is not a regular file, so EmuWiz will not write to it.".into(),
                );
            }
            Err(_) => return Err("The memory card is missing. Nothing was changed.".into()),
        };
        supported_card(&live, "memory card")?;
        let card_stem = location
            .path
            .file_name()
            .map(|name| sanitize(&name.to_string_lossy()))
            .unwrap_or_else(|| "card".into());
        let backup = dirs
            .backup_dir
            .join(format!("{card_stem}.{token}.before-restore.ps2"));
        let plan = plan_ps2_psu_restore(&live, &psu, &backup, true)
            .map_err(|error| psu_restore_error_message(&error))?;
        Ok(RestorePreview {
            plan,
            snapshot_id: snapshot.manifest.snapshot_id.clone(),
            snapshot_unix_seconds: snapshot.manifest.snapshot_unix_seconds,
            folder: save.entry.display_name.clone(),
            files: save
                .files
                .iter()
                .map(|file| (file.entry.display_name.clone(), file.declared_size_bytes))
                .collect(),
        })
    };
    finish().inspect_err(|_| {
        let _ = fs::remove_file(&psu);
    })
}

/// Cancel: remove the PSU a preview exported, only if it is still exactly
/// the reviewed file in EmuWiz's own folder. The live card is untouched.
pub(super) fn discard_preview(preview: &RestorePreview, dirs: &WorkDirs) {
    let psu = &preview.plan.source_psu_path;
    if psu.parent() != Some(dirs.psu_dir.as_path()) {
        return;
    }
    if fs::read(psu).is_ok_and(|bytes| sha256_hex(&bytes) == preview.plan.source_psu_sha256) {
        let _ = fs::remove_file(psu);
    }
}

/// Worker: the guarded Core restore. Refuses while any earlier restore is
/// unfinished, so the Save Vault's recovery runs first.
pub(super) fn apply_restore(
    plan: &Ps2PsuRestorePlan,
    observer: &dyn QuiescenceProvider,
    options: &Ps2RestoreGuardOptions,
) -> Result<Ps2GuardedRestore, String> {
    records_gate(&options.journal_dir, &plan.source_card_path)?;
    apply_ps2_psu_restore_guarded(plan, observer, options)
        .map_err(|error| psu_restore_error_message(&error))
}

/// Worker: Core's guarded Undo, bound to the restore journal.
/// Every card write from Blue (Apply and Undo) passes this first: the record
/// list must be complete; interrupted and unreadable records block every
/// card; a parked record blocks its own card unless its review still holds.
fn records_gate(journal_dir: &Path, card: &Path) -> Result<(), String> {
    let records = recovery::load_records(journal_dir);
    if !records.complete() {
        return Err("EmuWiz could not read every earlier restore record, so it will not change the card. Refresh the restore records below. Nothing was changed.".into());
    }
    if records.rows.iter().any(|row| row.blocks(card)) {
        return Err("An earlier memory card restore was interrupted or needs review. Use Restore records below (Check interrupted restores, or Review). Nothing was changed.".into());
    }
    Ok(())
}

pub(super) fn undo_restore(
    journal: &Path,
    observer: &dyn QuiescenceProvider,
    now_unix_seconds: u64,
) -> Result<(), String> {
    // Undo writes the card too, so it passes the same records gate.
    let card =
        archivefs_core::memory_card_inventory::restore_guard::load_ps2_restore_journal(journal)
            .map_err(|error| psu_restore_error_message(&error))?
            .card_path;
    let journal_dir = journal
        .parent()
        .ok_or("The restore record has no folder. Nothing was changed.")?;
    records_gate(journal_dir, &card)?;
    undo_ps2_psu_restore_guarded(journal, observer, now_unix_seconds)
        .map_err(|error| psu_restore_error_message(&error))
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

// ---------------------------------------------------------------------------
// Blue state, background jobs and UI
// ---------------------------------------------------------------------------

/// EmuWiz-owned folders this flow uses. `None` in tests unless set.
#[derive(Clone, Debug)]
pub(super) struct Paths {
    pub(super) snapshot_root: PathBuf,
    pub(super) dirs: WorkDirs,
    pub(super) journal_dir: PathBuf,
}

/// Where the person is in Preview -> Choose -> Confirm -> Restore -> Undo.
#[derive(Clone, Debug, Default)]
pub(super) enum Flow {
    #[default]
    Idle,
    Choose {
        snapshot: Box<SaveSnapshot>,
        location: SaveLocation,
        folders: Vec<SnapshotFolder>,
    },
    Confirm {
        preview: Box<RestorePreview>,
        location: SaveLocation,
    },
    Done {
        restore: Box<Ps2GuardedRestore>,
        location: SaveLocation,
        undone: bool,
    },
}

impl Flow {
    fn card(&self) -> Option<&Path> {
        match self {
            Self::Idle => None,
            Self::Choose { location, .. }
            | Self::Confirm { location, .. }
            | Self::Done { location, .. } => Some(&location.path),
        }
    }
}

#[derive(Default)]
pub(super) struct CardRestoreState {
    pub(super) paths: Option<Paths>,
    pub(super) paths_resolved: bool,
    /// Inventory generation the histories belong to.
    pub(super) loaded_for: Option<u64>,
    pub(super) histories: std::collections::BTreeMap<PathBuf, CardHistory>,
    pub(super) job: Option<u64>,
    pub(super) flow: Flow,
    /// (card, success, message) for the last action on a card.
    pub(super) feedback: Option<(PathBuf, bool, String)>,
    /// Restore records and retained-storage panels.
    pub(super) panel: panel::PanelState,
}

impl CardRestoreState {
    fn paths(&mut self) -> Option<Paths> {
        if !self.paths_resolved {
            self.paths_resolved = true;
            #[cfg(not(test))]
            {
                self.paths = (|| {
                    Some(Paths {
                        snapshot_root: archivefs_core::save_snapshots::default_snapshot_root()
                            .ok()?,
                        dirs: WorkDirs::default_dirs()?,
                        journal_dir:
                            archivefs_core::memory_card_inventory::restore_guard::default_ps2_restore_journal_dir()
                                .ok()?,
                    })
                })();
            }
        }
        self.paths.clone()
    }
}

pub(super) enum Job {
    Panel(panel::PanelJob),
    History {
        root: PathBuf,
        locations: Vec<SaveLocation>,
        generation: u64,
    },
    Create {
        root: PathBuf,
        location: SaveLocation,
    },
    Inspect {
        snapshot: Box<SaveSnapshot>,
        location: SaveLocation,
    },
    Prepare {
        snapshot: Box<SaveSnapshot>,
        location: SaveLocation,
        raw_name: Vec<u8>,
        dirs: WorkDirs,
    },
    Apply {
        preview: Box<RestorePreview>,
        location: SaveLocation,
        journal_dir: PathBuf,
    },
    Undo {
        restore: Box<Ps2GuardedRestore>,
        location: SaveLocation,
    },
}

pub(super) enum Outcome {
    Panel(panel::PanelOutcome),
    History {
        histories: std::collections::BTreeMap<PathBuf, CardHistory>,
        generation: u64,
    },
    Created {
        card: PathBuf,
        result: Result<String, String>,
    },
    Folders {
        snapshot: Box<SaveSnapshot>,
        location: SaveLocation,
        result: Result<Vec<SnapshotFolder>, String>,
    },
    Prepared {
        location: SaveLocation,
        result: Result<Box<RestorePreview>, String>,
    },
    Applied {
        location: SaveLocation,
        result: Result<Box<Ps2GuardedRestore>, String>,
    },
    Undone {
        restore: Box<Ps2GuardedRestore>,
        location: SaveLocation,
        result: Result<(), String>,
    },
}

fn now() -> u64 {
    archivefs_core::memory_card_inventory::restore_guard::unix_now()
}

/// Backend worker entry point. Uses the production PCSX2 closure observer.
pub(super) fn run(job: Job) -> Outcome {
    let observer = archivefs_core::save_snapshots::history::default_closure_observer();
    match job {
        Job::Panel(job) => Outcome::Panel(panel::run(job)),
        Job::History {
            root,
            locations,
            generation,
        } => Outcome::History {
            histories: locations
                .iter()
                .map(|location| {
                    (
                        location.path.clone(),
                        load_card_history(&root, location, &observer),
                    )
                })
                .collect(),
            generation,
        },
        Job::Create { root, location } => Outcome::Created {
            result: create_card_snapshot(&root, &location, &observer),
            card: location.path,
        },
        Job::Inspect { snapshot, location } => Outcome::Folders {
            result: snapshot_folders(&snapshot, &location),
            snapshot,
            location,
        },
        Job::Prepare {
            snapshot,
            location,
            raw_name,
            dirs,
        } => Outcome::Prepared {
            result: prepare_restore(&snapshot, &location, &raw_name, &dirs, now()).map(Box::new),
            location,
        },
        Job::Apply {
            preview,
            location,
            journal_dir,
        } => {
            let result = fs::create_dir_all(&journal_dir)
                .map_err(|error| format!("EmuWiz could not prepare its restore records: {error}"))
                .and_then(|()| {
                    apply_restore(
                        &preview.plan,
                        &observer,
                        &Ps2RestoreGuardOptions {
                            journal_dir,
                            unix_seconds: now(),
                        },
                    )
                })
                .map(Box::new);
            Outcome::Applied { location, result }
        }
        Job::Undo { restore, location } => Outcome::Undone {
            result: undo_restore(&restore.journal_path, &observer, now()),
            restore,
            location,
        },
    }
}

/// Apply a finished job to the state; returns the activity message.
pub(super) fn receive(app: &mut super::App, id: u64, outcome: Outcome) -> String {
    let generation = app.saves_states.generation;
    let state = &mut app.saves_states.card_restore;
    if state.job == Some(id) {
        state.job = None;
    }
    match outcome {
        Outcome::Panel(outcome) => panel::receive(&mut state.panel, outcome),
        Outcome::History {
            histories,
            generation: loaded,
        } => {
            if loaded == generation {
                state.histories = histories;
            } else {
                state.loaded_for = None;
            }
            "Memory card snapshots checked. No files were changed.".into()
        }
        Outcome::Created { card, result } => {
            state.loaded_for = None;
            let message = match &result {
                Ok(id) => {
                    format!("Card snapshot {id} created and verified. Your card was only read.")
                }
                Err(error) => error.clone(),
            };
            state.feedback = Some((card, result.is_ok(), message.clone()));
            message
        }
        Outcome::Folders {
            snapshot,
            location,
            result,
        } => match result {
            Ok(folders) => {
                state.feedback = None;
                state.flow = Flow::Choose {
                    snapshot,
                    location,
                    folders,
                };
                "Snapshot card read. Nothing was changed.".into()
            }
            Err(error) => {
                state.feedback = Some((location.path, false, error.clone()));
                error
            }
        },
        Outcome::Prepared { location, result } => match result {
            Ok(preview) => {
                state.feedback = None;
                state.flow = Flow::Confirm { preview, location };
                "Restore preview ready. Your card has not been changed.".into()
            }
            Err(error) => {
                state.feedback = Some((
                    location.path,
                    false,
                    format!("{error} Your card was not changed."),
                ));
                error
            }
        },
        Outcome::Applied { location, result } => {
            state.loaded_for = None;
            state.panel.records_requested = false;
            match result {
                Ok(restore) => {
                    let message = format!(
                        "Restored the save folder {} and verified the card. A backup of the card was kept first.",
                        restore.result.save_display_name
                    );
                    state.feedback = Some((location.path.clone(), true, message.clone()));
                    state.flow = Flow::Done {
                        restore,
                        location,
                        undone: false,
                    };
                    message
                }
                Err(error) => {
                    state.flow = Flow::Idle;
                    state.feedback = Some((location.path, false, error.clone()));
                    error
                }
            }
        }
        Outcome::Undone {
            restore,
            location,
            result,
        } => {
            state.loaded_for = None;
            state.panel.records_requested = false;
            match result {
                Ok(()) => {
                    let message =
                        "Undo finished: your memory card is back to how it was before the restore."
                            .to_string();
                    state.feedback = Some((location.path.clone(), true, message.clone()));
                    state.flow = Flow::Done {
                        restore,
                        location,
                        undone: true,
                    };
                    message
                }
                Err(error) => {
                    let message = format!("Undo was not done. {error}");
                    state.feedback = Some((location.path.clone(), false, message.clone()));
                    state.flow = Flow::Done {
                        restore,
                        location,
                        undone: false,
                    };
                    message
                }
            }
        }
    }
}

/// A job ended without an outcome (worker error or cancellation).
pub(super) fn job_failed(app: &mut super::App, error: &str) {
    let state = &mut app.saves_states.card_restore;
    state.job = None;
    state.loaded_for = None;
    if let Some(card) = state.flow.card().map(Path::to_path_buf) {
        // Choose/Confirm are dropped so a half-finished step is never
        // retried from a stale preview; Done keeps Undo reachable.
        if !matches!(state.flow, Flow::Done { .. }) {
            state.flow = Flow::Idle;
        }
        state.feedback = Some((
            card,
            false,
            format!(
                "The step did not finish. Check this card again before restoring; an interrupted restore is recovered from the Save Vault. {error}"
            ),
        ));
    }
}

/// Start a read-only history check for the listed cards when needed.
pub(super) fn ensure_history(app: &mut super::App, locations: Vec<SaveLocation>) {
    let generation = app.saves_states.generation;
    let state = &mut app.saves_states.card_restore;
    if state.job.is_some() || state.loaded_for == Some(generation) {
        return;
    }
    let Some(paths) = state.paths() else {
        return;
    };
    state.loaded_for = Some(generation);
    if locations.is_empty() {
        state.histories.clear();
        return;
    }
    app.start_card_restore(
        "Checking memory card snapshots",
        Job::History {
            root: paths.snapshot_root,
            locations,
            generation,
        },
    );
}

use super::{
    guidance::GuidanceCategory,
    save_snapshots::{closure_message, format_unix_seconds, tip},
};
use crate::ui::{
    components::{StatusTone, status_badge},
    theme,
};
use eframe::egui;

/// The memory-card snapshot and folder-restore section under one card.
pub(super) fn show(
    ui: &mut egui::Ui,
    app: &mut super::App,
    record: &PersistentStateRecord,
    location: Result<&SaveLocation, &'static str>,
) {
    if record.state_type == PersistentStateType::SaveState {
        ui.add_space(theme::SPACE_XS);
        tip(
            ui,
            "card_restore_savestate",
            GuidanceCategory::Explain,
            SAVESTATE_RESTORE_UNSUPPORTED,
        );
        return;
    }
    if record.state_type != PersistentStateType::MemoryCard || record.container_path.is_some() {
        return;
    }
    ui.add_space(theme::SPACE_XS);
    ui.label(egui::RichText::new("Card snapshots and save folder restore").strong());
    let location = match location {
        Ok(location) => location.clone(),
        Err(reason) => {
            tip(
                ui,
                "card_restore_unavailable",
                GuidanceCategory::Explain,
                reason,
            );
            return;
        }
    };
    let state = &app.saves_states.card_restore;
    if state.paths.is_none() && state.paths_resolved {
        tip(
            ui,
            "card_restore_root_missing",
            GuidanceCategory::Warning,
            "EmuWiz could not find its local data folder, so card snapshots cannot be listed or restored.",
        );
        return;
    }
    let busy = state.job.is_some();
    if let Some((card, ok, message)) = &state.feedback
        && card == &location.path
    {
        let colour = if *ok {
            theme::success(ui)
        } else {
            theme::warning(ui)
        };
        ui.label(egui::RichText::new(message).color(colour));
    }
    let flow_here = state.flow.card() == Some(location.path.as_path());
    if flow_here {
        show_flow(ui, app, &location);
        return;
    }
    let Some(history) = state.histories.get(&location.path).cloned() else {
        ui.weak(if busy {
            "Checking card snapshots…"
        } else {
            "Card snapshots have not been checked yet."
        });
        return;
    };
    let idle = !busy && matches!(state.flow, Flow::Idle);
    if history.snapshots.is_empty() {
        ui.weak("No snapshots of this memory card yet.");
    }
    if !history.warnings.is_empty() {
        ui.collapsing("Some snapshots could not be read", |ui| {
            for warning in &history.warnings {
                ui.label(warning);
            }
        });
    }
    let mut inspect = None;
    for entry in &history.snapshots {
        ui.push_id(&entry.snapshot.storage_path, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format_unix_seconds(entry.snapshot.manifest.snapshot_unix_seconds));
                match &entry.integrity {
                    Ok(()) => {
                        status_badge(ui, "Verified", StatusTone::Success);
                        if ui
                            .add_enabled(idle, egui::Button::new("Restore a save folder…"))
                            .on_hover_text("Shows the save folders in this snapshot. You choose one and review the change before anything is written.")
                            .clicked()
                        {
                            inspect = Some(entry.snapshot.clone());
                        }
                    }
                    Err(detail) => {
                        status_badge(ui, "Damaged", StatusTone::Warning);
                        ui.label(
                            egui::RichText::new(format!(
                                "This snapshot no longer matches its checksums and cannot be used: {detail}"
                            ))
                            .color(theme::warning(ui)),
                        );
                    }
                }
            });
        });
    }
    let create_allowed = idle && history.emulator_use == Some(EmulatorUseStatus::NotDetected);
    let create = ui
        .add_enabled(create_allowed, egui::Button::new("Create card snapshot"))
        .on_hover_text("Copies the whole card into EmuWiz's local snapshot folder and verifies it. Your card is only read.")
        .on_disabled_hover_text(closure_message(StateEmulator::Pcsx2, history.emulator_use))
        .clicked();
    if !create_allowed && !busy {
        tip(
            ui,
            "card_restore_closure",
            GuidanceCategory::WhyBlocked,
            closure_message(StateEmulator::Pcsx2, history.emulator_use),
        );
    }
    let Some(paths) = app.saves_states.card_restore.paths.clone() else {
        return;
    };
    if create {
        app.saves_states.card_restore.feedback = None;
        app.start_card_restore(
            "Creating a memory card snapshot",
            Job::Create {
                root: paths.snapshot_root,
                location,
            },
        );
    } else if let Some(snapshot) = inspect {
        app.saves_states.card_restore.feedback = None;
        app.start_card_restore(
            "Reading save folders in a card snapshot",
            Job::Inspect {
                snapshot: Box::new(snapshot),
                location,
            },
        );
    }
}

fn show_flow(ui: &mut egui::Ui, app: &mut super::App, location: &SaveLocation) {
    let busy = app.saves_states.card_restore.job.is_some();
    let paths = app.saves_states.card_restore.paths.clone();
    let flow = app.saves_states.card_restore.flow.clone();
    let mut next: Option<Flow> = None;
    let mut job: Option<(&'static str, Job)> = None;
    egui::Frame::group(ui.style()).show(ui, |ui| match &flow {
        Flow::Idle => {}
        Flow::Choose {
            snapshot,
            folders,
            ..
        } => {
            ui.heading("Choose one save folder to restore");
            ui.label(format!(
                "From the card snapshot made {}. Only the folder you choose is changed on your card; every other save on the card stays as it is. The whole card is never replaced.",
                format_unix_seconds(snapshot.manifest.snapshot_unix_seconds)
            ));
            ui.weak("Folder names come from the memory card itself. EmuWiz does not assume which game a folder belongs to.");
            if folders.is_empty() {
                ui.label("This snapshot has no save folders to restore.");
            }
            for folder in folders {
                ui.push_id(&folder.raw_name, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&folder.display_name);
                        ui.weak(format!(
                            "{} file(s), {} bytes",
                            folder.files.len(),
                            folder.files.iter().map(|(_, size)| size).sum::<u64>()
                        ));
                        let usable = folder.problem.is_none() && !busy;
                        if ui
                            .add_enabled(usable, egui::Button::new("Preview restore"))
                            .on_disabled_hover_text(
                                folder.problem.as_deref().unwrap_or("Please wait."),
                            )
                            .clicked()
                            && let Some(paths) = &paths
                        {
                            job = Some((
                                "Preparing a save folder restore preview",
                                Job::Prepare {
                                    snapshot: snapshot.clone(),
                                    location: location.clone(),
                                    raw_name: folder.raw_name.clone(),
                                    dirs: paths.dirs.clone(),
                                },
                            ));
                        }
                    });
                    if let Some(problem) = &folder.problem {
                        ui.label(egui::RichText::new(problem).color(theme::warning(ui)));
                    }
                });
            }
            if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                next = Some(Flow::Idle);
            }
        }
        Flow::Confirm { preview, .. } => {
            let plan = &preview.plan;
            ui.heading("Preview restore");
            ui.label("Nothing has been changed yet. Review this, then confirm.");
            ui.label("Emulator: PCSX2. It must be closed; EmuWiz checks again just before writing.");
            ui.label(format!("Save folder: {}", preview.folder));
            for (name, size) in &preview.files {
                ui.label(format!("  • {name} ({size} bytes)"));
            }
            ui.label(format!(
                "From snapshot: {} ({})",
                format_unix_seconds(preview.snapshot_unix_seconds),
                preview.snapshot_id
            ));
            ui.label(format!("Memory card to change: {}", plan.source_card_path.display()));
            ui.label(if plan.existing_save {
                "Action: replace the copy of this folder that is on the card now."
            } else {
                "Action: add this folder to the card."
            });
            ui.label(format!(
                "Backup of the whole card, made and verified first: {}",
                plan.backup_path.display()
            ));
            ui.weak(format!(
                "Card space: needs {} blocks, {} free. Other saves on the card are not changed.",
                plan.required_clusters, plan.free_clusters
            ));
            let can_apply = !busy && paths.is_some();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(can_apply, egui::Button::new("Back up card and restore this folder"))
                    .clicked()
                    && let Some(paths) = &paths
                {
                    job = Some((
                        "Backing up the memory card and restoring a save folder",
                        Job::Apply {
                            preview: preview.clone(),
                            location: location.clone(),
                            journal_dir: paths.journal_dir.clone(),
                        },
                    ));
                }
                if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                    if let Some(paths) = &paths {
                        discard_preview(preview, &paths.dirs);
                    }
                    next = Some(Flow::Idle);
                }
            });
        }
        Flow::Done {
            restore, undone, ..
        } => {
            if *undone {
                ui.label("The restore was undone.");
            } else {
                ui.heading("Save folder restored");
                ui.label(format!("Save folder: {}", restore.result.save_display_name));
                ui.label(format!(
                    "Backup of the card before the restore: {}",
                    restore.result.backup_path.display()
                ));
                ui.weak("Undo puts the whole card back exactly as it was before this restore. It is refused if the card has changed since, for example after playing.");
            }
            ui.horizontal_wrapped(|ui| {
                if !*undone
                    && ui
                        .add_enabled(!busy, egui::Button::new("Undo restore"))
                        .clicked()
                {
                    job = Some((
                        "Undoing a save folder restore",
                        Job::Undo {
                            restore: restore.clone(),
                            location: location.clone(),
                        },
                    ));
                }
                if ui.add_enabled(!busy, egui::Button::new("Close")).clicked() {
                    next = Some(Flow::Idle);
                }
            });
        }
    });
    if busy {
        ui.weak("Working… please keep PCSX2 closed.");
    }
    if let Some(flow) = next {
        let state = &mut app.saves_states.card_restore;
        state.flow = flow;
        state.feedback = None;
        state.loaded_for = None;
    }
    if let Some((label, job)) = job {
        app.saves_states.card_restore.feedback = None;
        app.start_card_restore(label, job);
    }
}
