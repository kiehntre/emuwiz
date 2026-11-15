//! Game-specific save snapshots on the Blue Saves page.
//!
//! History, integrity and freshness come from
//! `archivefs_core::save_snapshots::history`, computed by the worker. Creation
//! uses the reviewed `create_snapshot` copy and is offered only when the
//! emulator's closure is proven by a reviewed observer (PCSX2 and RetroArch).
//! Snapshots never write to a live save. Restore is offered only for
//! single-file SRAM with verified cartridge association (see
//! `single_file_restore`); other save types keep their own routes.

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
        SaveSnapshotError, SaveSnapshotRequest,
        history::{
            SnapshotFreshness, SnapshotHistoryEntry, SnapshotIntegrity, create_snapshot_observed,
            history_for_location, list_snapshots, nearest_existing_directory,
            snapshot_closure_supported, snapshot_emulator_use,
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
    /// The newest single-file restore that can still be undone.
    pub(super) undoable: Option<Box<archivefs_core::save_snapshots::SaveRestoreReceipt>>,
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
    if let Some(target) = sram_target(record, game) {
        return target;
    }
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
        location: save_location(record, artifact_type),
        emulator: record.emulator,
        game_identity: identity,
        platform: game.platform.clone(),
    })
}

/// A RetroArch SRAM file whose origin was captured with a verified cartridge
/// that is this game's own verified identity. Anything weaker (no capture,
/// needs-review, a different cartridge, a directory) returns `None` and falls
/// through to the generic path, which never treats a file as SRAM.
fn sram_target(
    record: &PersistentStateRecord,
    game: &Game,
) -> Option<Result<SnapshotTarget, &'static str>> {
    use archivefs_core::{game_identity::IdentityKind, sram_provenance::OriginState};
    let origin = record.sram_origin.as_ref()?;
    if record.state_type != PersistentStateType::NativeSave
        || record.emulator != StateEmulator::RetroArch
        || origin.state != OriginState::Associated
        || origin.context.cartridge_identity.kind != IdentityKind::LooseRomSha256
        || origin.context.cartridge_identity.status != IdentityStatus::Verified
        || origin.context.cartridge_identity.value.as_ref()
            != Some(&origin.context.cartridge.sha256)
    {
        return None;
    }
    if !record.warnings.is_empty() {
        return Some(Err(
            "This save location needs review first. Check the warnings in Advanced details.",
        ));
    }
    let sha = &origin.context.cartridge.sha256;
    // The same join that puts this record on the game's page: sealed capture
    // evidence bound to this game's exact file, refusing any disagreeing
    // cached digest or incomplete inspection. No filename or title joins.
    let own = super::saves_states::record_belongs_to_game(record, game);
    if !own {
        return Some(Err(
            "This save was made with a cartridge that is not this game's verified file, so EmuWiz will not snapshot or restore it here.",
        ));
    }
    Some(Ok(SnapshotTarget {
        location: save_location(record, SaveArtifactType::Sram),
        emulator: record.emulator,
        game_identity: format!("LooseRomSha256:{sha}"),
        platform: game.platform.clone(),
    }))
}

/// The snapshot location for a live record, keyed by its emulator profile.
pub(super) fn save_location(
    record: &PersistentStateRecord,
    artifact_type: SaveArtifactType,
) -> SaveLocation {
    SaveLocation {
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
    }
}

/// Worker: list, verify and compare snapshots and observe emulator closure.
pub(super) fn load_history(
    root: &std::path::Path,
    targets: &[SnapshotTarget],
    observer: &dyn QuiescenceProvider,
    receipt_roots: Option<(&std::path::Path, &std::path::Path)>,
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
                    undoable: receipt_roots
                        .filter(|_| super::single_file_restore::restorable(target))
                        .and_then(|(receipts, backups)| {
                            super::single_file_restore::undoable_receipt(
                                receipts,
                                backups,
                                &target.location.path,
                            )
                        }),
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
    create_snapshot_observed(
        target.emulator,
        SaveSnapshotRequest {
        location,
        game_identity: Some(target.game_identity.clone()),
        platform: Some(target.platform.clone()),
        storage_root: root.to_path_buf(),
        available_space_bytes,
        snapshot_id: None,
        now_unix_seconds: None,
            // Decided (under a card lease where possible) by Core.
            emulator_use: EmulatorUseStatus::Unknown,
        },
        observer,
    )
    .map(|snapshot| snapshot.manifest.snapshot_id)
    .map_err(|error| match error {
        SaveSnapshotError::EmulatorRunning => {
            "The emulator is running. Close it, then try again. Nothing was copied.".to_string()
        }
        SaveSnapshotError::CardInUse => "This save is in use. Another application may have it open. Close applications accessing it and try again. Nothing was copied.".to_string(),
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
    let restorable = super::single_file_restore::restorable(target);
    let restore_busy = busy || app.saves_states.single_file.job.is_some();
    let mut preview = None;
    for entry in &record_snapshots.entries {
        let restore = restorable.then_some(!restore_busy);
        if ui
            .push_id(&entry.snapshot.storage_path, |ui| {
                history_row(ui, entry, restore)
            })
            .inner
        {
            preview = Some(entry.snapshot.clone());
        }
    }
    let undoable = record_snapshots.undoable.clone();
    let emulator_use = record_snapshots.emulator_use;
    let create_allowed = !busy && emulator_use == Some(EmulatorUseStatus::NotDetected);
    let mut start = false;
    ui.horizontal_wrapped(|ui| {
        start = ui
            .add_enabled(create_allowed, egui::Button::new("Create snapshot"))
            .on_hover_text("Copies this save into EmuWiz's local snapshot folder and verifies it. Your save is only read.")
            .on_disabled_hover_text(closure_message(target.emulator, emulator_use))
            .clicked();
        if !restorable {
            ui.add_enabled(false, egui::Button::new("Restore"))
                .on_disabled_hover_text(restore_unavailable(record.state_type));
        }
    });
    if restorable {
        let idle = matches!(
            app.saves_states.single_file.flow,
            super::single_file_restore::Flow::Idle
        );
        if idle
            && let Some(receipt) = undoable
            && ui
                .add_enabled(!restore_busy, egui::Button::new("Undo last restore"))
                .on_hover_text("Puts back the save you had before the last EmuWiz restore, if the file has not changed since.")
                .clicked()
        {
            app.start_single_file_restore(
                "Undoing a save restore",
                super::single_file_restore::Job::Undo {
                    receipt,
                    target: Box::new(target.clone()),
                },
            );
        }
        super::single_file_restore::show_flow(ui, app, &target.location.path);
    }
    if let Some(snapshot) = preview {
        super::single_file_restore::begin_preview(app, snapshot, target.clone());
    }
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

/// Why Restore is disabled on a game page, by save type.
pub(super) fn restore_unavailable(state_type: PersistentStateType) -> &'static str {
    match state_type {
        PersistentStateType::SaveState => super::card_restore::SAVESTATE_RESTORE_UNSUPPORTED,
        PersistentStateType::MemoryCard => super::card_restore::CARD_RESTORE_WHERE,
        _ => RESTORE_UNAVAILABLE,
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
            "No running emulator was detected. A snapshot only reads your save."
        }
        Some(EmulatorUseStatus::Running) => {
            "The emulator is running. Close it, then refresh save locations."
        }
        Some(EmulatorUseStatus::CardInUse) => {
            "This save is in use. Another application may have it open. Close applications accessing it, then refresh save locations."
        }
        Some(EmulatorUseStatus::Unknown) | None => {
            "EmuWiz could not check every running program, so it cannot confirm the emulator is closed. Nothing will be copied until it can."
        }
    }
}

/// One snapshot. `restore` is `Some(enabled)` when this save type has a
/// reviewed restore; the return value is whether Restore was clicked.
fn history_row(ui: &mut egui::Ui, entry: &SnapshotHistoryEntry, restore: Option<bool>) -> bool {
    let mut clicked = false;
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
        if let Some(enabled) = restore {
            clicked = ui
                .add_enabled(
                    enabled && entry.compatible(),
                    egui::Button::new("Preview restore…"),
                )
                .on_hover_text(
                    "Shows exactly what would change. Nothing is written until you confirm.",
                )
                .clicked();
        }
    });
    clicked
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

pub(super) fn tip(ui: &mut egui::Ui, key: &'static str, category: GuidanceCategory, message: &str) {
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

pub(super) fn format_unix_seconds(seconds: u64) -> String {
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
