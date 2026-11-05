//! Restore and Undo for one single-file native save (RetroArch SRAM).
//!
//! Journey: pick a verified snapshot -> Preview (identity, freshness, exactly
//! what changes, emulator closure) -> explicit Confirm -> guarded restore ->
//! Undo. Every write goes through `archivefs_core::save_snapshots` (backup,
//! journal-before-publish, atomic rename, post-write verification) under
//! `single_file_guard`, which re-proves the emulator is closed at the moment
//! of the write. This module never writes a save itself. Only SRAM files with
//! verified cartridge association reach it; cards, directories and savestates
//! keep their own routes or stay inspection-only.

use super::save_snapshots::{SnapshotTarget, format_unix_seconds};
use archivefs_core::save_snapshots::{
    SaveArtifactType, SaveQuiescenceRequirement, SaveRestoreApplyOptions, SaveRestorePreflight,
    SaveRestoreReceipt, SaveSnapshot, SaveTargetBinding, build_single_file_restore_preflight,
    history::restore_quiescence,
    list_single_file_restore_receipts,
    single_file_guard::{restore_single_file_guarded, undo_single_file_guarded},
    tree_restore::safety::QuiescenceProvider,
    verify_snapshot,
};
use eframe::egui;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// Why a target may be restored here at all.
pub(super) fn restorable(target: &SnapshotTarget) -> bool {
    target.location.artifact_type == SaveArtifactType::Sram
}

#[derive(Clone, Debug)]
pub(super) struct Dirs {
    pub(super) backup_root: PathBuf,
    pub(super) receipt_root: PathBuf,
}

/// Everything the confirmation shows, captured at preview time.
#[derive(Clone, Debug)]
pub(super) struct Preview {
    pub(super) snapshot: SaveSnapshot,
    pub(super) target: SnapshotTarget,
    pub(super) preflight: SaveRestorePreflight,
    pub(super) snapshot_bytes: u64,
    pub(super) current_bytes: Option<u64>,
    pub(super) current_modified: Option<u64>,
    pub(super) current_newer: bool,
    pub(super) identical: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) enum Flow {
    #[default]
    Idle,
    Confirm(Box<Preview>),
    Done {
        receipt: Box<SaveRestoreReceipt>,
        target: Box<SnapshotTarget>,
        opened_during: bool,
        undone: bool,
    },
}

impl Flow {
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Idle => None,
            Self::Confirm(preview) => Some(&preview.target.location.path),
            Self::Done { target, .. } => Some(&target.location.path),
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    pub(super) dirs: Option<Dirs>,
    dirs_resolved: bool,
    pub(super) job: Option<u64>,
    pub(super) flow: Flow,
    /// (save path, success, message) for the last action.
    pub(super) feedback: Option<(PathBuf, bool, String)>,
}

impl State {
    #[cfg(test)]
    pub(super) fn dirs_for_tests(&mut self, dirs: Option<Dirs>) {
        self.dirs = dirs;
        self.dirs_resolved = true;
    }

    pub(super) fn dirs(&mut self) -> Option<Dirs> {
        if !self.dirs_resolved {
            self.dirs_resolved = true;
            #[cfg(not(test))]
            {
                self.dirs = archivefs_core::save_snapshots::default_single_file_restore_roots()
                    .ok()
                    .map(|(backup_root, receipt_root)| Dirs {
                        backup_root,
                        receipt_root,
                    });
            }
        }
        self.dirs.clone()
    }
}

pub(super) enum Job {
    Prepare {
        snapshot: Box<SaveSnapshot>,
        target: Box<SnapshotTarget>,
    },
    Apply {
        preview: Box<Preview>,
        dirs: Dirs,
    },
    Undo {
        receipt: Box<SaveRestoreReceipt>,
        target: Box<SnapshotTarget>,
    },
}

pub(super) enum Outcome {
    Prepared(Result<Box<Preview>, String>),
    Applied {
        target: Box<SnapshotTarget>,
        result: Result<(Box<SaveRestoreReceipt>, bool), String>,
    },
    Undone {
        receipt: Box<SaveRestoreReceipt>,
        target: Box<SnapshotTarget>,
        result: Result<bool, String>,
    },
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Backend worker entry point.
pub(super) fn run(job: Job) -> Outcome {
    run_with(
        job,
        &archivefs_core::save_snapshots::history::default_closure_observer(),
    )
}

pub(super) fn run_with(job: Job, observer: &dyn QuiescenceProvider) -> Outcome {
    match job {
        Job::Prepare { snapshot, target } => {
            Outcome::Prepared(prepare(*snapshot, *target, observer))
        }
        Job::Apply { preview, dirs } => {
            let target = Box::new(preview.target.clone());
            Outcome::Applied {
                target,
                result: apply(&preview, &dirs, observer),
            }
        }
        Job::Undo { receipt, target } => {
            let result = undo(&receipt, &target, observer);
            Outcome::Undone {
                receipt,
                target,
                result,
            }
        }
    }
}

fn prepare(
    snapshot: SaveSnapshot,
    target: SnapshotTarget,
    observer: &dyn QuiescenceProvider,
) -> Result<Box<Preview>, String> {
    if !restorable(&target) {
        return Err("This kind of save cannot be restored from here.".into());
    }
    verify_snapshot(&snapshot).map_err(|error| {
        format!("This snapshot no longer matches its checksums, so it will not be used. ({error})")
    })?;
    let manifest = &snapshot.manifest;
    if manifest.original_save_path != target.location.path
        || manifest.game_identity.as_deref() != Some(target.game_identity.as_str())
    {
        return Err(
            "This snapshot is not recorded for this game's save, so it will not be used.".into(),
        );
    }
    let binding = SaveTargetBinding::from_snapshot(&snapshot);
    let quiescence = restore_quiescence(target.emulator, &target.location.path, observer);
    let preflight = build_single_file_restore_preflight(&snapshot, &binding, quiescence);
    let snapshot_bytes = manifest
        .artifacts
        .first()
        .map_or(0, |artifact| artifact.size_bytes);
    let snapshot_modified = manifest
        .artifacts
        .first()
        .and_then(|a| a.modified_unix_seconds);
    let metadata = fs::symlink_metadata(&target.location.path).ok();
    let current_modified = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    Ok(Box::new(Preview {
        identical: preflight.target_fingerprint.as_deref()
            == Some(preflight.snapshot_sha256.as_str()),
        current_bytes: metadata.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
        current_newer: current_modified
            .zip(snapshot_modified)
            .is_some_and(|(current, snap)| current > snap),
        current_modified,
        snapshot_bytes,
        snapshot,
        target,
        preflight,
    }))
}

fn apply(
    preview: &Preview,
    dirs: &Dirs,
    observer: &dyn QuiescenceProvider,
) -> Result<(Box<SaveRestoreReceipt>, bool), String> {
    if !preview.preflight.ready() || preview.identical {
        return Err("This restore was not confirmable. Nothing was changed.".into());
    }
    let options = SaveRestoreApplyOptions {
        backup_root: dirs.backup_root.clone(),
        receipt_root: dirs.receipt_root.clone(),
        now_unix_seconds: now(),
    };
    restore_single_file_guarded(
        &preview.snapshot,
        &preview.preflight,
        preview.target.emulator,
        observer,
        &options,
    )
    .map(|done| (Box::new(done.result.receipt), done.opened_during_restore))
    .map_err(|error| format!("The save was not restored. {}", explain(&error)))
}

fn undo(
    receipt: &SaveRestoreReceipt,
    target: &SnapshotTarget,
    observer: &dyn QuiescenceProvider,
) -> Result<bool, String> {
    undo_single_file_guarded(receipt, target.emulator, observer)
        .map(|done| done.opened_during_undo)
        .map_err(|error| format!("Undo did not change the save. {}", explain(&error)))
}

fn explain(error: &archivefs_core::save_snapshots::SaveRestoreError) -> String {
    use archivefs_core::save_snapshots::{SaveRestoreError, SaveRestoreRefusal};
    match error {
        SaveRestoreError::Refused(SaveRestoreRefusal::RollbackFailed) => {
            "The restore failed and could not be reversed safely. Your pre-restore backup is kept in EmuWiz's restore-backups folder; do not start the emulator until you have checked the save.".into()
        }
        SaveRestoreError::Refused(reason) => format!("Reason: {reason}. Nothing was changed."),
        other => format!("{other}. Check the save before starting the emulator."),
    }
}

/// Newest receipt for `path` that is still undoable: it targets exactly this
/// file and the file still holds the bytes that restore published. Receipts
/// that do not match (for example one left by a restore that never published)
/// are skipped, so they cannot hide an earlier valid Undo.
pub(super) fn undoable_receipt(
    receipt_root: &Path,
    backup_root: &Path,
    path: &Path,
) -> Option<Box<SaveRestoreReceipt>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 * 1024
    {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut file = fs::File::open(path).ok()?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let hash: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    list_single_file_restore_receipts(receipt_root, backup_root)
        .into_iter()
        .find(|receipt| {
            receipt.target_binding.target_path == path
                && receipt.rollback_eligible
                && receipt.post_restore_actual_sha256 == hash
        })
        .map(Box::new)
}

/// Apply a worker result to the UI state. Returns the activity message.
pub(super) fn receive(app: &mut super::App, id: u64, outcome: Outcome) -> String {
    let state = &mut app.saves_states.single_file;
    if state.job == Some(id) {
        state.job = None;
    }
    // Re-read history so the new file state and Undo are shown.
    app.saves_states.snapshots.loaded_for = None;
    let state = &mut app.saves_states.single_file;
    match outcome {
        Outcome::Prepared(Ok(preview)) => {
            let message = "Restore preview ready. Nothing has been changed.".to_string();
            state.flow = Flow::Confirm(preview);
            state.feedback = None;
            message
        }
        Outcome::Prepared(Err(error)) => {
            state.flow = Flow::Idle;
            state.feedback = None;
            error
        }
        Outcome::Applied { target, result } => match result {
            Ok((receipt, opened)) => {
                let message = if opened {
                    "The save was restored, but a program may have opened it during the restore. Close the emulator and check the save; Undo is available.".to_string()
                } else {
                    "Save restored and verified. Your previous save was backed up; Undo is available.".to_string()
                };
                state.feedback = Some((target.location.path.clone(), !opened, message.clone()));
                state.flow = Flow::Done {
                    receipt,
                    target,
                    opened_during: opened,
                    undone: false,
                };
                message
            }
            Err(error) => {
                state.feedback = Some((target.location.path.clone(), false, error.clone()));
                state.flow = Flow::Idle;
                error
            }
        },
        Outcome::Undone {
            receipt,
            target,
            result,
        } => match result {
            Ok(opened) => {
                let message = if opened {
                    "The previous save was put back, but a program may have opened it meanwhile. Check the save before playing.".to_string()
                } else {
                    "Undo complete: the save you had before the restore is back and verified."
                        .to_string()
                };
                state.feedback = Some((target.location.path.clone(), !opened, message.clone()));
                state.flow = Flow::Done {
                    receipt,
                    target,
                    opened_during: opened,
                    undone: true,
                };
                message
            }
            Err(error) => {
                state.feedback = Some((target.location.path.clone(), false, error.clone()));
                if !matches!(state.flow, Flow::Done { .. }) {
                    state.flow = Flow::Done {
                        receipt,
                        target,
                        opened_during: false,
                        undone: false,
                    };
                }
                error
            }
        },
    }
}

/// Start a Prepare job for the chosen snapshot.
pub(super) fn begin_preview(app: &mut super::App, snapshot: SaveSnapshot, target: SnapshotTarget) {
    app.start_single_file_restore(
        "Previewing a save restore",
        Job::Prepare {
            snapshot: Box::new(snapshot),
            target: Box::new(target),
        },
    );
}

/// Preview / Done / feedback for the record at `path`.
pub(super) fn show_flow(ui: &mut egui::Ui, app: &mut super::App, path: &Path) {
    let busy = app.saves_states.single_file.job.is_some();
    if let Some((feedback_path, ok, message)) = &app.saves_states.single_file.feedback
        && feedback_path == path
    {
        let color = if *ok {
            crate::ui::theme::success(ui)
        } else {
            crate::ui::theme::warning(ui)
        };
        ui.colored_label(color, message);
    }
    let flow = app.saves_states.single_file.flow.clone();
    if flow.path() != Some(path) {
        return;
    }
    match flow {
        Flow::Idle => {}
        Flow::Confirm(preview) => {
            let mut confirm = false;
            let mut cancel = false;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong("Review this restore");
                ui.label(format!("Save file: {}", preview.target.location.path.display()));
                ui.label(format!(
                    "Snapshot taken {} · {} bytes · checksum verified.",
                    format_unix_seconds(preview.snapshot.manifest.snapshot_unix_seconds),
                    preview.snapshot_bytes
                ));
                match preview.current_bytes {
                    Some(bytes) => ui.label(format!(
                        "Your current save: {bytes} bytes{}.",
                        preview
                            .current_modified
                            .map(|t| format!(", last changed {}", format_unix_seconds(t)))
                            .unwrap_or_default()
                    )),
                    None => ui.label("Your current save file is missing."),
                };
                if preview.identical {
                    ui.label("The snapshot is identical to your current save. There is nothing to restore.");
                } else {
                    ui.label("Restoring replaces this one file with the snapshot. Your current save is first copied to a verified backup, and Undo puts it back. No other save is touched.");
                }
                if preview.current_newer && !preview.identical {
                    ui.colored_label(
                        crate::ui::theme::warning(ui),
                        "Your current save is newer than this snapshot: any progress made since the snapshot will be replaced (it stays in the backup).",
                    );
                }
                for refusal in &preview.preflight.refusals {
                    ui.colored_label(crate::ui::theme::warning(ui), format!("Blocked: {refusal}."));
                }
                if preview.preflight.quiescence != SaveQuiescenceRequirement::ConfirmedClosed {
                    ui.weak("EmuWiz checks again immediately before writing, and refuses if RetroArch is running or the save is open.");
                }
                ui.horizontal_wrapped(|ui| {
                    confirm = ui
                        .add_enabled(
                            !busy && preview.preflight.ready() && !preview.identical,
                            egui::Button::new("Confirm restore"),
                        )
                        .clicked();
                    cancel = ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked();
                });
            });
            if cancel {
                app.saves_states.single_file.flow = Flow::Idle;
            } else if confirm && let Some(dirs) = app.saves_states.single_file.dirs() {
                app.start_single_file_restore("Restoring a save", Job::Apply { preview, dirs });
            }
        }
        Flow::Done {
            receipt,
            target,
            undone,
            opened_during,
        } => {
            if opened_during {
                ui.colored_label(
                    crate::ui::theme::warning(ui),
                    "A program may have opened this save while it was being changed. Close the emulator and check the save before playing.",
                );
            }
            if undone {
                if ui.button("Dismiss").clicked() {
                    app.saves_states.single_file.flow = Flow::Idle;
                }
            } else if ui
                .add_enabled(!busy, egui::Button::new("Undo this restore"))
                .on_hover_text("Puts back the save you had before this restore, if the file has not changed since.")
                .clicked()
            {
                app.start_single_file_restore("Undoing a save restore", Job::Undo { receipt, target });
            }
        }
    }
}

#[cfg(test)]
mod tests;
