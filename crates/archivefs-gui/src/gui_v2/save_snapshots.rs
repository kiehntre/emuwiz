//! Game-specific save snapshots on the Blue Saves page.
//!
//! History, integrity and freshness come from
//! `archivefs_core::save_snapshots::history`, computed by the worker. Creation
//! uses the reviewed `create_snapshot` copy and is offered only when the
//! emulator's closure is proven by an existing observer (PCSX2 today).
//! Snapshots never write to a live save, and restore is not offered here.

use super::{
    App,
    guidance::{self, GuidanceCategory, GuidanceTip, MascotState},
    library::Game,
};
use crate::ui::{
    components::{StatusTone, status_badge},
    theme,
};
use archivefs_core::{
    game_identity::IdentityStatus,
    persistent_state_inventory::{
        PersistentStateRecord, PersistentStateType, StateEmulator, StatePathOrigin,
    },
    save_snapshots::{
        EmulatorUseStatus, SaveArtifactType, SaveLocation, SaveProvenance, SaveRestoreConflict,
        SaveSnapshotError, SaveSnapshotRequest, create_snapshot,
        history::{
            SnapshotFreshness, SnapshotHistoryEntry, SnapshotIntegrity, history_for_location,
            list_snapshots, nearest_existing_directory, snapshot_closure_supported,
            snapshot_emulator_use,
        },
        tree_restore::safety::{FilesystemSpace, QuiescenceProvider, RestoreSpaceProvider},
    },
};
use eframe::egui;
use std::{collections::BTreeMap, path::PathBuf};

/// Why restore is not offered from this page.
pub(super) const RESTORE_UNAVAILABLE: &str = "Restoring a snapshot is not available here yet. EmuWiz cannot yet prove the emulator is closed for a safe write, and only a few save types have a reviewed restore with backup and Undo. Your snapshots are kept unchanged.";

/// One live save this game may snapshot, with everything the snapshot and
/// comparison need. Built only from explicit verified identity evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SnapshotTarget {
    pub(super) location: SaveLocation,
    pub(super) emulator: StateEmulator,
    pub(super) game_identity: String,
    pub(super) platform: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RecordSnapshots {
    pub(super) entries: Vec<SnapshotHistoryEntry>,
    pub(super) emulator_use: Option<EmulatorUseStatus>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SnapshotReport {
    pub(super) records: BTreeMap<PathBuf, RecordSnapshots>,
    pub(super) warnings: Vec<String>,
}

#[derive(Default)]
pub(super) struct SnapshotPanelState {
    /// EmuWiz's managed snapshot root. `None` in tests unless a fixture sets it.
    pub(super) root: Option<PathBuf>,
    pub(super) root_resolved: bool,
    /// (game, inventory generation) the current report belongs to.
    pub(super) loaded_for: Option<(i64, u64)>,
    pub(super) report: Option<SnapshotReport>,
    pub(super) history_job: Option<u64>,
    pub(super) create_job: Option<u64>,
    pub(super) feedback: Option<(bool, String)>,
}

impl SnapshotPanelState {
    fn root(&mut self) -> Option<PathBuf> {
        if !self.root_resolved {
            self.root_resolved = true;
            #[cfg(not(test))]
            {
                self.root = archivefs_core::save_snapshots::default_snapshot_root().ok();
            }
        }
        self.root.clone()
    }
}

/// The snapshot target for a record already associated with `game`, or the
/// reason no snapshot can be offered.
pub(super) fn snapshot_target(
    record: &PersistentStateRecord,
    game: &Game,
) -> Result<SnapshotTarget, &'static str> {
    let artifact_type = match record.state_type {
        PersistentStateType::MemoryCard => match record.emulator {
            StateEmulator::Pcsx2 | StateEmulator::DuckStation => SaveArtifactType::PsMemoryCard,
            StateEmulator::Dolphin => SaveArtifactType::DolphinMemoryCard,
            _ => SaveArtifactType::MemoryCard,
        },
        PersistentStateType::SaveState => SaveArtifactType::SaveState,
        // Refined to a directory type by the worker; never guessed as SRAM.
        PersistentStateType::NativeSave => SaveArtifactType::Other,
        PersistentStateType::NandOrVirtualDisk
        | PersistentStateType::ConfigBoundState
        | PersistentStateType::CloudManaged
        | PersistentStateType::Unknown => {
            return Err(
                "This is shared system storage or emulator data, not one game's save, so it is not snapshotted from a game page.",
            );
        }
    };
    if !record.warnings.is_empty() {
        return Err(
            "This save location needs review first. Check the warnings in Advanced details.",
        );
    }
    let report =
        game.archive.identity_report.as_ref().ok_or(
            "This game has no verified identity yet, so EmuWiz cannot tie a snapshot to it.",
        )?;
    let identity = record
        .game_identity
        .iter()
        .filter(|identity| identity.status == IdentityStatus::Verified)
        .find_map(|identity| {
            let value = identity.value.as_deref()?;
            report
                .evidence
                .iter()
                .any(|game_identity| {
                    game_identity.status == IdentityStatus::Verified
                        && game_identity.kind == identity.kind
                        && game_identity.value.as_deref() == Some(value)
                })
                .then(|| format!("{:?}:{value}", identity.kind))
        })
        .ok_or(
            "This save does not share a verified identity with this game, so EmuWiz will not snapshot it here.",
        )?;
    Ok(SnapshotTarget {
        location: SaveLocation {
            path: record.path.clone(),
            emulator: Some(record.emulator.as_str().to_string()),
            profile: record
                .selected_installation
                .as_ref()
                .map(|installation| installation.installation_id.clone())
                .or_else(|| record.slot_profile_account.clone()),
            artifact_type,
            provenance: match record.source_path_origin {
                StatePathOrigin::Configured | StatePathOrigin::ExplicitCustom => {
                    SaveProvenance::ConfiguredPath
                }
                StatePathOrigin::Native | StatePathOrigin::Flatpak | StatePathOrigin::Portable => {
                    SaveProvenance::EmulatorProfile
                }
            },
        },
        emulator: record.emulator,
        game_identity: identity,
        platform: game.platform.clone(),
    })
}

/// Worker: list, verify and compare snapshots and observe emulator closure.
pub(super) fn load_history(
    root: &std::path::Path,
    targets: &[SnapshotTarget],
    observer: &dyn QuiescenceProvider,
) -> SnapshotReport {
    let listing = list_snapshots(root);
    let records = targets
        .iter()
        .map(|target| {
            (
                target.location.path.clone(),
                RecordSnapshots {
                    entries: history_for_location(
                        &listing,
                        &target.location,
                        &target.game_identity,
                    ),
                    emulator_use: Some(snapshot_emulator_use(
                        target.emulator,
                        &target.location.path,
                        observer,
                    )),
                },
            )
        })
        .collect();
    SnapshotReport {
        records,
        warnings: listing.warnings,
    }
}

/// Worker: re-observe closure now, then make one snapshot with the reviewed
/// Core copy. The live save is only read.
pub(super) fn create(
    root: &std::path::Path,
    target: &SnapshotTarget,
    observer: &dyn QuiescenceProvider,
) -> Result<String, String> {
    let emulator_use = snapshot_emulator_use(target.emulator, &target.location.path, observer);
    let mut location = target.location.clone();
    if location.artifact_type == SaveArtifactType::Other
        && std::fs::symlink_metadata(&location.path).is_ok_and(|metadata| metadata.is_dir())
    {
        location.artifact_type = if target.emulator == StateEmulator::Ppsspp {
            SaveArtifactType::PspSavedata
        } else {
            SaveArtifactType::SaveDirectory
        };
    }
    let available_space_bytes =
        nearest_existing_directory(root).and_then(|dir| FilesystemSpace.available_bytes(&dir).ok());
    create_snapshot(&SaveSnapshotRequest {
        location,
        game_identity: Some(target.game_identity.clone()),
        platform: Some(target.platform.clone()),
        storage_root: root.to_path_buf(),
        available_space_bytes,
        snapshot_id: None,
        now_unix_seconds: None,
        emulator_use,
    })
    .map(|snapshot| snapshot.manifest.snapshot_id)
    .map_err(|error| match error {
        SaveSnapshotError::EmulatorRunning => {
            "The emulator is running. Close it, then try again. Nothing was copied.".to_string()
        }
        SaveSnapshotError::EmulatorUseUnknown => "EmuWiz could not confirm the emulator is closed, so nothing was copied.".to_string(),
        SaveSnapshotError::SourceChanged(_) => "The save changed while it was being copied. Nothing was kept; close the emulator and try again.".to_string(),
        SaveSnapshotError::InsufficientSpace { .. } => "There is not enough free space for this snapshot. Nothing was copied.".to_string(),
        other => format!("The snapshot was not created. Your save was not changed. ({other})"),
    })
}

/// Called after the selected game's records are known. Starts a read-only
/// history check when the report is missing or belongs to another game or
/// inventory generation.
pub(super) fn ensure_history(app: &mut App, game_id: i64, targets: Vec<SnapshotTarget>) {
    let generation = app.saves_states.generation;
    let panel = &mut app.saves_states.snapshots;
    if panel.history_job.is_some()
        || panel.create_job.is_some()
        || panel.loaded_for == Some((game_id, generation))
    {
        return;
    }
    let Some(root) = panel.root() else {
        return;
    };
    panel.loaded_for = Some((game_id, generation));
    if targets.is_empty() {
        panel.report = Some(SnapshotReport::default());
        return;
    }
    app.start_snapshot_history(root, targets);
}

/// The Snapshots section shown under one record on a game page.
pub(super) fn show(
    ui: &mut egui::Ui,
    app: &mut App,
    record: &PersistentStateRecord,
    target: Result<&SnapshotTarget, &'static str>,
) {
    ui.add_space(theme::SPACE_XS);
    ui.label(egui::RichText::new(section_title(record.state_type)).strong());
    let target = match target {
        Ok(target) => target,
        Err(reason) => {
            tip(
                ui,
                "save_snapshot_unavailable",
                GuidanceCategory::Explain,
                reason,
            );
            return;
        }
    };
    let panel = &app.saves_states.snapshots;
    if panel.root.is_none() && panel.root_resolved {
        tip(
            ui,
            "save_snapshot_root_missing",
            GuidanceCategory::Warning,
            "EmuWiz could not find its local data folder, so snapshots cannot be listed or created.",
        );
        return;
    }
    let busy = panel.history_job.is_some() || panel.create_job.is_some();
    let record_snapshots = panel
        .report
        .as_ref()
        .and_then(|report| report.records.get(&record.path));
    let Some(record_snapshots) = record_snapshots else {
        ui.weak(if busy {
            "Checking snapshots…"
        } else {
            "Snapshots have not been checked yet."
        });
        return;
    };
    if record_snapshots.entries.is_empty() {
        ui.weak("No snapshots of this save yet.");
    }
    for entry in &record_snapshots.entries {
        ui.push_id(&entry.snapshot.storage_path, |ui| history_row(ui, entry));
    }
    let emulator_use = record_snapshots.emulator_use;
    let create_allowed = !busy && emulator_use == Some(EmulatorUseStatus::NotDetected);
    let mut start = false;
    ui.horizontal_wrapped(|ui| {
        start = ui
            .add_enabled(create_allowed, egui::Button::new("Create snapshot"))
            .on_hover_text("Copies this save into EmuWiz's local snapshot folder and verifies it. Your save is only read.")
            .on_disabled_hover_text(closure_message(target.emulator, emulator_use))
            .clicked();
        ui.add_enabled(false, egui::Button::new("Restore"))
            .on_disabled_hover_text(RESTORE_UNAVAILABLE);
    });
    if !create_allowed && !busy {
        tip(
            ui,
            "save_snapshot_closure",
            GuidanceCategory::WhyBlocked,
            closure_message(target.emulator, emulator_use),
        );
    }
    if start && let Some(root) = app.saves_states.snapshots.root.clone() {
        app.start_snapshot_create(root, target.clone());
    }
}

fn section_title(state_type: PersistentStateType) -> &'static str {
    match state_type {
        PersistentStateType::MemoryCard => "Memory card snapshots",
        PersistentStateType::SaveState => "Savestate snapshots",
        _ => "Save snapshots",
    }
}

pub(super) fn closure_message(
    emulator: StateEmulator,
    observed: Option<EmulatorUseStatus>,
) -> &'static str {
    if !snapshot_closure_supported(emulator) {
        return "EmuWiz cannot yet confirm this emulator is closed, so snapshots of its saves are not offered. You can still review existing snapshots and your save here.";
    }
    match observed {
        Some(EmulatorUseStatus::NotDetected) => {
            "The emulator is closed. A snapshot only reads your save."
        }
        Some(EmulatorUseStatus::Running) => {
            "The emulator is running or has this save open. Close it, then refresh save locations."
        }
        Some(EmulatorUseStatus::Unknown) | None => {
            "EmuWiz could not check every running program, so it cannot confirm the emulator is closed. Nothing will be copied until it can."
        }
    }
}

fn history_row(ui: &mut egui::Ui, entry: &SnapshotHistoryEntry) {
    let manifest = &entry.snapshot.manifest;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format_unix_seconds(manifest.snapshot_unix_seconds));
            let (label, tone) = match (&entry.integrity, entry.compatible()) {
                (SnapshotIntegrity::Failed(_), _) => ("Damaged", StatusTone::Warning),
                (_, false) => ("Different emulator setup", StatusTone::Warning),
                _ => ("Verified", StatusTone::Success),
            };
            status_badge(ui, label, tone);
        });
        ui.label(freshness_label(entry.freshness));
        for conflict in &entry.compatibility_conflicts {
            ui.label(egui::RichText::new(conflict_label(conflict)).color(theme::warning(ui)));
        }
        if let SnapshotIntegrity::Failed(detail) = &entry.integrity {
            ui.label(
                egui::RichText::new(format!(
                    "This snapshot no longer matches its recorded checksums: {detail}"
                ))
                .color(theme::warning(ui)),
            );
        }
        ui.collapsing("Snapshot details", |ui| {
            ui.label(format!("Source: {}", manifest.original_save_path.display()));
            ui.label(format!(
                "Emulator: {}{}",
                manifest.emulator.as_deref().unwrap_or("not recorded"),
                manifest
                    .emulator_profile
                    .as_deref()
                    .map(|profile| format!(" · profile {profile}"))
                    .unwrap_or_default()
            ));
            ui.label(format!(
                "Provenance: {}",
                match manifest.provenance {
                    SaveProvenance::EmulatorProfile => "emulator profile location",
                    SaveProvenance::ConfiguredPath => "configured save location",
                    SaveProvenance::UserSpecified => "location you chose",
                }
            ));
            ui.label(format!(
                "{} file(s), {} bytes · stored in {}",
                manifest.artifacts.len(),
                manifest.source_size_bytes,
                entry.snapshot.storage_path.display()
            ));
        });
    });
}

pub(super) fn freshness_label(freshness: SnapshotFreshness) -> &'static str {
    match freshness {
        SnapshotFreshness::MatchesCurrentSave => "Same as your current save.",
        SnapshotFreshness::CurrentSaveNewer => {
            "Older than your current save: you have played since this snapshot."
        }
        SnapshotFreshness::CurrentSaveDiffers => "Different from your current save.",
        SnapshotFreshness::CurrentSaveMissing => {
            "Your current save is missing; this snapshot still holds a copy."
        }
    }
}

fn conflict_label(conflict: &SaveRestoreConflict) -> &'static str {
    match conflict {
        SaveRestoreConflict::EmulatorMismatch => "Made with a different emulator.",
        SaveRestoreConflict::ProfileMismatch => "Made with a different emulator profile.",
        SaveRestoreConflict::SnapshotIncomplete => "This snapshot is incomplete.",
        SaveRestoreConflict::SnapshotUnreadable => "Part of this snapshot could not be read.",
        _ => "Needs review before any use.",
    }
}

fn tip(ui: &mut egui::Ui, key: &'static str, category: GuidanceCategory, message: &str) {
    guidance::show_tip(
        ui,
        &GuidanceTip {
            key,
            category,
            mascot: MascotState::Helpful,
            message: message.to_string(),
        },
    );
}

fn format_unix_seconds(seconds: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(seconds as i64)
        .map(|at| {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02} UTC",
                at.year(),
                u8::from(at.month()),
                at.day(),
                at.hour(),
                at.minute()
            )
        })
        .unwrap_or_else(|_| "Unknown date".into())
}
