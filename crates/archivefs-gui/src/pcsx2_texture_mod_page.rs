//! PCSX2 texture replacements embedded in Cheats & Mods.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::patch_manager::{
    Pcsx2Profile, Pcsx2TexturePackApplyResult, Pcsx2TexturePackPlan,
    Pcsx2TexturePackPreviewRequest, SharedApplyConfirmation, SharedApplyEntry, SharedApplyOptions,
    SharedApplyOutcome, SharedApplyResult, SharedApplyStatus, SharedRollbackConfirmation,
    SharedRollbackOptions, SharedRollbackOutcome, SharedRollbackPreview, SharedRollbackResult,
    SharedTransactionPlan, build_pcsx2_texture_pack_preview,
    build_pcsx2_texture_pack_transaction_plan, default_shared_backup_root,
    default_shared_history_root, execute_pcsx2_texture_pack_apply, execute_shared_rollback,
    generate_shared_operation_id, preview_shared_rollback, verified_pcsx2_texture_identity,
};
use eframe::egui;

use crate::ui::components as widgets;

#[derive(Default)]
pub(crate) struct Pcsx2TextureModPageState {
    key: Option<(PathBuf, String)>,
    stage: Option<Stage>,
}

enum Stage {
    Picking {
        receiver: Receiver<Option<PathBuf>>,
        archive: PathBuf,
        profile: Pcsx2Profile,
        report: Option<GameIdentityReport>,
    },
    Preview {
        plan: Pcsx2TexturePackPlan,
        transaction: SharedTransactionPlan,
        destination: PathBuf,
    },
    Applying {
        receiver: Receiver<Result<Pcsx2TexturePackApplyResult, String>>,
        destination: PathBuf,
    },
    /// The install worker finished; `result.journal.status` says how, and
    /// `rollback` is the core's automatic rollback (`None`: it did not run).
    Applied {
        result: SharedApplyResult,
        rollback: Option<SharedRollbackResult>,
        warnings: Vec<String>,
        destination: PathBuf,
    },
    RollbackPreview {
        preview: SharedRollbackPreview,
        destination: PathBuf,
        /// The result Undo was opened from, so a refused Undo can go back to it.
        back: Box<Stage>,
    },
    RollingBack(Receiver<Result<SharedRollbackResult, String>>),
    Failed(String),
}

impl Pcsx2TextureModPageState {
    fn sync(&mut self, archive: &Path, profile: &Pcsx2Profile) {
        let key = (archive.to_path_buf(), profile.profile_id.clone());
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.stage = None;
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            Some(Stage::Picking { .. } | Stage::Applying { .. } | Stage::RollingBack(_))
        )
    }

    pub(crate) fn poll(&mut self) -> bool {
        let Some(stage) = self.stage.take() else {
            return false;
        };
        match stage {
            Stage::Picking {
                receiver,
                archive,
                profile,
                report,
            } => match receiver.try_recv() {
                Ok(Some(path)) => {
                    let result = report
                        .as_ref()
                        .ok_or_else(|| "verified PS2 identity is required before Apply".to_string())
                        .and_then(|report| {
                            verified_pcsx2_texture_identity(&archive, report)
                                .map_err(|e| e.to_string())
                        })
                        .and_then(|identity| {
                            let request = Pcsx2TexturePackPreviewRequest {
                                selected_pack: path.clone(),
                                source_root: path,
                                identity,
                                profile,
                            };
                            build_pcsx2_texture_pack_preview(&request).map_err(|e| e.to_string())
                        });
                    self.stage = match result {
                        Ok(plan) => {
                            let destination = plan
                                .report
                                .entries
                                .first()
                                .map(|e| e.destination_root.clone())
                                .unwrap_or_default();
                            match build_pcsx2_texture_pack_transaction_plan(
                                &plan,
                                self.key
                                    .as_ref()
                                    .map(|(_, id)| id.as_str())
                                    .unwrap_or_default(),
                                &plan.inspection.source_root,
                            ) {
                                Ok(transaction) => Some(Stage::Preview {
                                    plan,
                                    transaction,
                                    destination,
                                }),
                                Err(e) => Some(Stage::Failed(e.detail)),
                            }
                        }
                        Err(error) => Some(Stage::Failed(error)),
                    };
                    true
                }
                Ok(None) => true,
                Err(TryRecvError::Empty) => {
                    self.stage = Some(Stage::Picking {
                        receiver,
                        archive,
                        profile,
                        report,
                    });
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(Stage::Failed("pack picker stopped unexpectedly".into()));
                    true
                }
            },
            Stage::Applying {
                receiver,
                destination,
            } => match receiver.try_recv() {
                Ok(Ok(result)) => {
                    let warnings = crate::shared_apply_warnings::lines_after_rollback(
                        &result.apply,
                        result.rollback.as_ref(),
                    );
                    self.stage = Some(Stage::Applied {
                        result: result.apply,
                        rollback: result.rollback,
                        warnings,
                        destination,
                    });
                    true
                }
                Ok(Err(error)) => {
                    self.stage = Some(Stage::Failed(error));
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Some(Stage::Applying {
                        receiver,
                        destination,
                    });
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(Stage::Failed("install worker stopped unexpectedly".into()));
                    true
                }
            },
            Stage::RollingBack(receiver) => match receiver.try_recv() {
                Ok(Ok(result)) => {
                    self.stage = if result.status == SharedApplyStatus::Success {
                        None
                    } else {
                        let mut detail = format!("Undo did not complete ({:?}).", result.status);
                        for line in rollback_problem_lines(&result.preview) {
                            detail.push('\n');
                            detail.push_str(&line);
                        }
                        Some(Stage::Failed(detail))
                    };
                    true
                }
                Ok(Err(error)) => {
                    self.stage = Some(Stage::Failed(error));
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Some(Stage::RollingBack(receiver));
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(Stage::Failed("undo worker stopped unexpectedly".into()));
                    true
                }
            },
            other => {
                self.stage = Some(other);
                false
            }
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The install worker's result, worded from the recorded journal status:
/// only `Success` is called installed.
fn show_applied_summary(
    ui: &mut egui::Ui,
    result: &SharedApplyResult,
    rollback: Option<&SharedRollbackResult>,
    warnings: &[String],
    destination: &Path,
) {
    match result.journal.status {
        SharedApplyStatus::Success => {
            widgets::status_badge(ui, "Installed", widgets::StatusTone::Success)
        }
        SharedApplyStatus::Failed => {
            widgets::status_badge(ui, "Install failed", widgets::StatusTone::Blocked)
        }
        SharedApplyStatus::PartialFailure | SharedApplyStatus::DryRun => {
            widgets::status_badge(ui, "Install incomplete", widgets::StatusTone::Warning)
        }
    };
    for line in incomplete_install_lines(result, rollback) {
        ui.colored_label(ui.visuals().warn_fg_color, line);
    }
    if recovery_required(result, rollback) {
        widgets::status_badge(ui, "Recovery required", widgets::StatusTone::Blocked);
        for line in recovery_lines(result, rollback, destination) {
            ui.colored_label(ui.visuals().warn_fg_color, line);
        }
    }
    if let Some(failure) = &result.journal_failure {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("The install history was not saved: {}", failure.detail),
        );
    }
    crate::shared_apply_warnings::show(ui, warnings);
}

/// What did and did not happen, for any status other than `Success`.
fn incomplete_install_lines(
    result: &SharedApplyResult,
    rollback: Option<&SharedRollbackResult>,
) -> Vec<String> {
    let status = result.journal.status;
    if status == SharedApplyStatus::Success {
        return Vec::new();
    }
    let entries = &result.journal.entries;
    let written = entries.iter().filter(|entry| written(entry)).count();
    let rollback_line = match rollback {
        Some(rollback) if rollback.status == SharedApplyStatus::Success => {
            "Automatic rollback completed and reversed this install's recorded changes, so there is nothing to undo.".to_string()
        }
        Some(rollback) => format!(
            "Automatic rollback did not complete ({:?}), so some files may still come from this pack. Check the destination before using this profile.",
            rollback.status
        ),
        None if written == 0 => {
            "Automatic rollback did not run because no file was written or changed.".to_string()
        }
        None => "Automatic rollback did not run, so the files written before the failure are still in place. Check the destination before using this profile.".to_string(),
    };
    vec![
        format!(
            "The install did not complete ({status:?}): {written} of {} file(s) were written or may have been changed on disk.",
            entries.len()
        ),
        rollback_line,
    ]
}

/// Whether this entry may have changed the destination on disk. A completed
/// write counts, and so does a write or verification failure that happened
/// after the pack's file reached the destination: the core records the
/// temporary path whenever a write got that far (checkpoint failed after the
/// replace, verification failed, a displaced file kept, or a published file
/// that changed straight afterwards). Counting one too many fails closed:
/// Check Undo is still gated by a fresh preview.
fn written(entry: &SharedApplyEntry) -> bool {
    match entry.outcome {
        SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting => true,
        SharedApplyOutcome::WriteFailed | SharedApplyOutcome::VerificationFailed => {
            entry.temporary_path.is_some() || entry.final_destination_digest.is_some()
        }
        _ => false,
    }
}

fn rollback_succeeded(rollback: Option<&SharedRollbackResult>) -> bool {
    rollback.is_some_and(|rollback| rollback.status == SharedApplyStatus::Success)
}

/// An install that wrote files and was not confirmed reversed: automatic
/// rollback failed, stopped part way, or did not run.
fn recovery_required(result: &SharedApplyResult, rollback: Option<&SharedRollbackResult>) -> bool {
    result.journal.status != SharedApplyStatus::Success
        && result.journal.entries.iter().any(written)
        && !rollback_succeeded(rollback)
}

/// What Undo this page may open for a finished install worker.
#[derive(Debug, PartialEq, Eq)]
enum UndoChoice {
    /// A recorded successful install.
    Undo,
    /// An install that needs recovery. Undo first re-reads the journal and
    /// every file, and the core refuses it unless each file is still exactly
    /// as the install or the failed rollback left it.
    CheckUndo,
    /// Nothing to undo: nothing was written, automatic rollback already
    /// reversed it, or no journal was saved.
    None,
}

fn undo_choice(result: &SharedApplyResult, rollback: Option<&SharedRollbackResult>) -> UndoChoice {
    if result.journal_path.is_none() {
        UndoChoice::None
    } else if result.journal.status == SharedApplyStatus::Success {
        UndoChoice::Undo
    } else if recovery_required(result, rollback) {
        UndoChoice::CheckUndo
    } else {
        UndoChoice::None
    }
}

/// Where each written file stands, and what the user can do by hand.
fn recovery_lines(
    result: &SharedApplyResult,
    rollback: Option<&SharedRollbackResult>,
    destination: &Path,
) -> Vec<String> {
    let mut lines = vec![format!(
        "This install was not fully reversed. Destination folder: {}",
        destination.display()
    )];
    for entry in result.journal.entries.iter().filter(|entry| written(entry)) {
        let plan = &entry.plan_entry;
        let path =
            Path::new(&plan.destination_root.display).join(&plan.destination_relative_path.display);
        let rolled_back = rollback.and_then(|rollback| {
            rollback.preview.entries.iter().find(|candidate| {
                candidate
                    .destination
                    .as_ref()
                    .is_some_and(|destination| Path::new(&destination.display) == path)
            })
        });
        let line = match rolled_back.map(|entry| entry.outcome) {
            Some(SharedRollbackOutcome::RestoredBackup) => {
                "your original was restored by automatic rollback.".to_string()
            }
            Some(SharedRollbackOutcome::RemovedInstalledFile) => {
                "the pack's file was removed by automatic rollback.".to_string()
            }
            outcome => {
                let replaced = match entry.outcome {
                    SharedApplyOutcome::ReplacedExisting => true,
                    SharedApplyOutcome::InstalledNew => false,
                    _ => entry.destination_existed_before_apply == Some(true),
                };
                let state = match (replaced || entry.backup_path.is_some(), &entry.backup_path) {
                    (_, Some(backup)) => format!(
                        "may still be the pack's file; your original is kept in EmuWiz's backup at {}",
                        backup.display
                    ),
                    (true, None) => {
                        "may still be the pack's file, and no backup of your original was recorded"
                            .to_string()
                    }
                    _ => "may still be the pack's file; it did not exist before this install"
                        .to_string(),
                };
                let rollback = match (outcome, rolled_back.and_then(|e| e.failure.as_ref())) {
                    (_, Some(failure)) => format!(" Rollback: {}", failure.detail),
                    (Some(outcome), None) => format!(" Rollback: {outcome:?}."),
                    (None, None) => String::new(),
                };
                format!("{state}.{rollback}")
            }
        };
        lines.push(format!("{}: {line}", path.display()));
    }
    lines.push(if result.journal_path.is_some() {
        "Check Undo re-reads the install record and these files, and runs only if every file is still as EmuWiz left it. Otherwise, restore or remove the files above by hand.".to_string()
    } else {
        "No install record was saved, so Undo is not possible. Restore or remove the files above by hand.".to_string()
    });
    lines
}

/// The files that stop Undo, with where they are and why.
fn rollback_problem_lines(preview: &SharedRollbackPreview) -> Vec<String> {
    preview
        .entries
        .iter()
        .filter(|entry| {
            !matches!(
                entry.outcome,
                SharedRollbackOutcome::Available
                    | SharedRollbackOutcome::NoChangeRequired
                    | SharedRollbackOutcome::RemovedInstalledFile
                    | SharedRollbackOutcome::RestoredBackup
            )
        })
        .map(|entry| {
            let file = entry
                .destination
                .as_ref()
                .map_or_else(|| "Install record".to_string(), |path| path.display.clone());
            let mut line = format!("{file}: {:?}.", entry.outcome);
            if let Some(backup) = &entry.backup {
                line.push_str(&format!(
                    " Your original is kept in EmuWiz's backup at {}.",
                    backup.display
                ));
            }
            if let Some(failure) = &entry.failure {
                line.push_str(&format!(" {}", failure.detail));
            }
            line
        })
        .collect()
}

pub(crate) fn show_pcsx2_texture_mod_panel(
    ui: &mut egui::Ui,
    state: &mut Pcsx2TextureModPageState,
    archive: &Path,
    profile: &Pcsx2Profile,
    report: Option<&GameIdentityReport>,
) {
    state.sync(archive, profile);
    widgets::section_header(
        ui,
        "PCSX2 Texture Mods",
        Some(
            "Inspect a bounded texture pack, preview exact changes, then install or undo through the shared transaction journal.",
        ),
    );
    widgets::card(ui, |ui| {
        ui.label(format!("Profile: {}", profile.provenance));
        if let Some(report) = report {
            match verified_pcsx2_texture_identity(archive, report) {
                Ok(identity) => ui.label(format!("Verified PS2 identity: {}", identity.serial)),
                Err(error) => {
                    ui.colored_label(egui::Color32::YELLOW, format!("Apply unavailable: {error}"))
                }
            };
        } else {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Apply unavailable until verified PS2 identity evidence is available.",
            );
        }
        if state.is_busy() {
            ui.label("Working…");
            return;
        }
        match state.stage.take() {
            None => {
                if widgets::action_button(ui, "Choose pack", widgets::ActionStyle::Primary, true)
                    .clicked()
                {
                    let (sender, receiver) = mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(rfd::FileDialog::new().pick_folder());
                    });
                    state.stage = Some(Stage::Picking {
                        receiver,
                        archive: archive.to_path_buf(),
                        profile: profile.clone(),
                        report: report.cloned(),
                    });
                }
            }
            Some(Stage::Failed(detail)) => {
                ui.colored_label(egui::Color32::RED, detail);
                if widgets::action_button(ui, "Start over", widgets::ActionStyle::Quiet, true)
                    .clicked()
                {
                    state.stage = None;
                }
            }
            Some(Stage::Preview {
                plan,
                transaction,
                destination,
            }) => {
                ui.label(format!("Pack: {}", plan.inspection.source_root.display()));
                ui.label(format!("Destination: {}", destination.display()));
                ui.label(format!("{} file(s), {} bytes · {} create · {} replace · {} already installed · {} conflict(s)", plan.inspection.files.len(), plan.inspection.total_bytes, plan.create_count(), plan.replace_count(), plan.already_installed_count(), plan.conflict_count()));
                if widgets::action_button(
                    ui,
                    "Confirm install",
                    widgets::ActionStyle::Primary,
                    plan.is_applyable(),
                )
                .clicked()
                {
                    state.stage = Some(spawn_apply(transaction, destination));
                } else {
                    state.stage = Some(Stage::Preview {
                        plan,
                        transaction,
                        destination,
                    });
                }
            }
            Some(Stage::Applied {
                result,
                rollback,
                warnings,
                destination,
            }) => {
                show_applied_summary(ui, &result, rollback.as_ref(), &warnings, &destination);
                let choice = undo_choice(&result, rollback.as_ref());
                let undo_label = match choice {
                    UndoChoice::Undo => Some("Undo"),
                    UndoChoice::CheckUndo => Some("Check Undo"),
                    UndoChoice::None => None,
                };
                let undo = undo_label.is_some_and(|label| {
                    widgets::action_button(ui, label, widgets::ActionStyle::Destructive, true)
                        .clicked()
                });
                let start_over = result.journal.status != SharedApplyStatus::Success
                    && widgets::action_button(ui, "Start over", widgets::ActionStyle::Quiet, true)
                        .clicked();
                let journal = result.journal_path.clone();
                let back = Stage::Applied {
                    result,
                    rollback,
                    warnings,
                    destination: destination.clone(),
                };
                state.stage = match (undo, journal) {
                    (true, Some(journal)) => match default_shared_backup_root() {
                        Ok(backup_root) => {
                            Some(undo_preview(&journal, destination, &backup_root, back))
                        }
                        Err(error) => Some(Stage::Failed(error.detail)),
                    },
                    // The stage stays empty, which starts over.
                    _ if start_over => None,
                    _ => Some(back),
                };
            }
            Some(Stage::RollbackPreview {
                preview,
                destination,
                back,
            }) => {
                if preview.available {
                    ui.label(
                        "Undo removes only unchanged EmuWiz-created files and restores safe backups.",
                    );
                } else {
                    widgets::status_badge(ui, "Undo refused", widgets::StatusTone::Blocked);
                    ui.label(
                        "Nothing was changed: these files are not as EmuWiz left them, so Undo cannot safely reverse them. Restore or remove them by hand.",
                    );
                    for line in rollback_problem_lines(&preview) {
                        ui.colored_label(ui.visuals().warn_fg_color, line);
                    }
                }
                if widgets::action_button(
                    ui,
                    "Confirm undo",
                    widgets::ActionStyle::Destructive,
                    preview.available,
                )
                .clicked()
                {
                    state.stage = Some(spawn_rollback(preview));
                } else if widgets::action_button(ui, "Back", widgets::ActionStyle::Quiet, true)
                    .clicked()
                {
                    state.stage = Some(*back);
                } else {
                    state.stage = Some(Stage::RollbackPreview {
                        preview,
                        destination,
                        back,
                    });
                }
            }
            Some(other) => {
                state.stage = Some(other);
            }
        }
    });
}

/// Undo always starts from the core's own preview of the saved journal,
/// which re-reads every file and backup; the page never decides on its own.
fn undo_preview(journal: &Path, destination: PathBuf, backup_root: &Path, back: Stage) -> Stage {
    Stage::RollbackPreview {
        preview: preview_shared_rollback(journal, &destination, backup_root),
        destination,
        back: Box::new(back),
    }
}

fn spawn_apply(plan: SharedTransactionPlan, destination: PathBuf) -> Stage {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let history_root = default_shared_history_root().map_err(|e| e.detail)?;
            let backup_root = default_shared_backup_root().map_err(|e| e.detail)?;
            Ok::<_, String>(execute_pcsx2_texture_pack_apply(
                &plan,
                &SharedApplyOptions {
                    dry_run: false,
                    confirmation: Some(SharedApplyConfirmation {
                        plan_id: plan.plan_id.clone(),
                        general_approved: true,
                        replacement_approved: true,
                    }),
                    operation_id: generate_shared_operation_id(),
                    timestamp_unix_seconds: now(),
                    current_context: plan.context.clone(),
                    history_root,
                    backup_root,
                },
            ))
        })();
        let _ = sender.send(result);
    });
    Stage::Applying {
        receiver,
        destination,
    }
}

fn spawn_rollback(preview: SharedRollbackPreview) -> Stage {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let history_root = default_shared_history_root().map_err(|e| e.detail)?;
            let backup_root = default_shared_backup_root().map_err(|e| e.detail)?;
            Ok::<_, String>(execute_shared_rollback(
                &preview,
                &SharedRollbackOptions {
                    confirmation: SharedRollbackConfirmation {
                        preview_id: preview.preview_id.clone(),
                        approved: true,
                    },
                    rollback_operation_id: generate_shared_operation_id(),
                    timestamp_unix_seconds: now(),
                    history_root,
                    backup_root,
                },
            ))
        })();
        let _ = sender.send(result);
    });
    Stage::RollingBack(receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_apply_warnings::test_support::*;
    use archivefs_core::patch_manager::{
        Pcsx2InstallationType, Pcsx2ProfileScope, SharedApplyFailure, SharedApplyFailureKind,
    };

    const ARCHIVE: &str = "/disposable/game.iso";

    fn profile() -> Pcsx2Profile {
        Pcsx2Profile {
            profile_id: "pcsx2-disposable".into(),
            installation_type: Pcsx2InstallationType::Native,
            scope: Pcsx2ProfileScope::User,
            configuration_path: PathBuf::from("/disposable/PCSX2"),
            provenance: "test fixture",
            eligible: true,
            blockers: Vec::new(),
            patch_directories: Vec::new(),
            configuration_identity: None,
            executable_candidates: Vec::new(),
        }
    }

    /// The page after the install worker reported `apply` and `rollback`.
    fn state_after(
        apply: SharedApplyResult,
        rollback: Option<SharedRollbackResult>,
    ) -> Pcsx2TextureModPageState {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Ok(Pcsx2TexturePackApplyResult { apply, rollback }))
            .unwrap();
        let mut state = Pcsx2TextureModPageState {
            key: Some((PathBuf::from(ARCHIVE), profile().profile_id)),
            stage: Some(Stage::Applying {
                receiver,
                destination: PathBuf::from("/disposable/destination"),
            }),
        };
        assert!(state.poll());
        assert!(matches!(state.stage, Some(Stage::Applied { .. })));
        state
    }

    /// One frame of the whole panel, as the user sees it.
    fn panel_text(state: &mut Pcsx2TextureModPageState) -> String {
        let profile = profile();
        render(|ui| show_pcsx2_texture_mod_panel(ui, state, Path::new(ARCHIVE), &profile, None))
    }

    fn panel_after(apply: SharedApplyResult, rollback: Option<SharedRollbackResult>) -> String {
        panel_text(&mut state_after(apply, rollback))
    }

    fn journalled(mut result: SharedApplyResult) -> SharedApplyResult {
        result.journal_path = Some(PathBuf::from("/disposable/history/op.json"));
        result
    }

    /// Nothing was written: the status the engine records when it stops
    /// before the first file.
    fn failed_result() -> SharedApplyResult {
        let mut result = warned_result();
        result.journal.status = SharedApplyStatus::Failed;
        result.journal.entries[0].outcome = SharedApplyOutcome::WriteFailed;
        result.journal.entries[0].warnings.clear();
        result
    }

    fn assert_not_installed(text: &str) {
        assert!(!text.contains("Installed"), "{text}");
        assert!(text.contains("Start over"), "{text}");
    }

    fn assert_no_undo(text: &str) {
        assert!(!text.contains("Undo"), "{text}");
    }

    const LIVE_FILE: &str = "/disposable/destination/pack/replaced.bin";
    const BACKUP: &str = "/disposable/backups/replaced.bin";

    fn with_backup(mut result: SharedApplyResult) -> SharedApplyResult {
        result.journal.entries[0].backup_path = Some(
            archivefs_core::patch_manager::SharedTransactionPath::from_path(Path::new(BACKUP)),
        );
        result
    }

    fn assert_recovery_required(text: &str) {
        assert!(text.contains("Recovery required"), "{text}");
        assert!(
            text.contains("Destination folder: /disposable/destination"),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "{LIVE_FILE}: may still be the pack's file; your original is kept in EmuWiz's backup at {BACKUP}"
            )),
            "{text}"
        );
        assert!(!text.contains("reversed this install"), "{text}");
    }

    #[test]
    fn a_successful_install_is_installed_and_offers_undo() {
        let result = journalled(warned_result());
        assert_eq!(undo_choice(&result, None), UndoChoice::Undo);
        let text = panel_after(result, None);
        assert!(text.contains("Installed"), "{text}");
        assert!(text.contains("Undo"), "{text}");
        assert!(!text.contains("Check Undo"), "{text}");
        assert!(!text.contains("did not complete"), "{text}");
        assert!(!text.contains("Recovery required"), "{text}");
        assert!(!text.contains("Start over"), "{text}");
        assert_shows_warning(&text);
    }

    #[test]
    fn a_rolled_back_partial_install_is_not_installed_and_cannot_be_undone_again() {
        let result = journalled(with_backup(partly_failed_result()));
        let rollback = succeeded_rollback();
        assert_eq!(undo_choice(&result, Some(&rollback)), UndoChoice::None);
        let text = panel_after(result, Some(rollback));
        assert_not_installed(&text);
        assert_no_undo(&text);
        assert!(text.contains("Install incomplete"), "{text}");
        assert!(text.contains("PartialFailure"), "{text}");
        assert!(
            text.contains("1 of 1 file(s) were written or may have been changed on disk"),
            "{text}"
        );
        assert!(
            text.contains(
                "Automatic rollback completed and reversed this install's recorded changes"
            ),
            "{text}"
        );
        assert!(!text.contains("Recovery required"), "{text}");
        // #280's per-file rollback wording is kept.
        assert_rollback_succeeded(&text);
    }

    #[test]
    fn a_failed_automatic_rollback_requires_recovery_and_offers_a_checked_undo() {
        let result = journalled(with_backup(partly_failed_result()));
        let mut rollback = failed_rollback();
        rollback.preview.entries[0].failure = Some(SharedApplyFailure {
            kind: SharedApplyFailureKind::WriteFailed,
            path: None,
            detail: "restore write failed".into(),
        });
        assert_eq!(undo_choice(&result, Some(&rollback)), UndoChoice::CheckUndo);
        let text = panel_after(result, Some(rollback));
        assert_not_installed(&text);
        assert!(
            text.contains("Automatic rollback did not complete (Failed)"),
            "{text}"
        );
        assert_recovery_required(&text);
        assert!(text.contains("Rollback: restore write failed"), "{text}");
        assert!(text.contains("Check Undo"), "{text}");
        assert!(
            text.contains("runs only if every file is still as EmuWiz left it"),
            "{text}"
        );
        assert_rollback_failed(&text);
    }

    #[test]
    fn a_partly_finished_rollback_reports_each_files_own_outcome() {
        let mut result = journalled(with_backup(partly_failed_result()));
        let mut created = result.journal.entries[0].clone();
        created.plan_entry.destination_relative_path =
            archivefs_core::patch_manager::SharedTransactionPath::from_path(Path::new(
                "pack/new.bin",
            ));
        created.outcome = SharedApplyOutcome::InstalledNew;
        created.backup_path = None;
        created.warnings.clear();
        result.journal.entries.push(created);
        let mut rollback = succeeded_rollback();
        rollback.status = SharedApplyStatus::PartialFailure;
        let mut retained = rollback.preview.entries[0].clone();
        retained.destination = Some(
            archivefs_core::patch_manager::SharedTransactionPath::from_path(Path::new(
                "/disposable/destination/pack/new.bin",
            )),
        );
        retained.outcome = SharedRollbackOutcome::Retained;
        rollback.preview.entries.push(retained);
        assert_eq!(undo_choice(&result, Some(&rollback)), UndoChoice::CheckUndo);
        let text = panel_after(result, Some(rollback));
        assert_not_installed(&text);
        assert!(text.contains("Recovery required"), "{text}");
        assert!(
            text.contains(&format!(
                "{LIVE_FILE}: your original was restored by automatic rollback."
            )),
            "{text}"
        );
        assert!(
            text.contains(
                "/disposable/destination/pack/new.bin: may still be the pack's file; it did not exist before this install. Rollback: Retained."
            ),
            "{text}"
        );
        assert!(text.contains("Check Undo"), "{text}");
    }

    #[test]
    fn an_unknown_rollback_outcome_requires_recovery() {
        let result = journalled(with_backup(partly_failed_result()));
        assert_eq!(undo_choice(&result, None), UndoChoice::CheckUndo);
        let text = panel_after(result, None);
        assert_not_installed(&text);
        assert!(
            text.contains(
                "Automatic rollback did not run, so the files written before the failure are still in place"
            ),
            "{text}"
        );
        assert_recovery_required(&text);
        assert!(text.contains("Check Undo"), "{text}");
        assert_rollback_did_not_run(&text);
    }

    #[test]
    fn without_a_saved_journal_recovery_is_manual() {
        let mut result = with_backup(partly_failed_result());
        result.journal_failure = Some(SharedApplyFailure {
            kind: SharedApplyFailureKind::JournalFailed,
            path: None,
            detail: "disk full".into(),
        });
        assert_eq!(undo_choice(&result, None), UndoChoice::None);
        let text = panel_after(result, None);
        assert_not_installed(&text);
        assert_recovery_required(&text);
        assert!(
            text.contains("The install history was not saved: disk full"),
            "{text}"
        );
        assert!(
            text.contains("No install record was saved, so Undo is not possible"),
            "{text}"
        );
        assert!(!text.contains("Check Undo"), "{text}");
    }

    #[test]
    fn a_failed_install_that_wrote_nothing_needs_no_recovery() {
        let result = journalled(failed_result());
        assert_eq!(undo_choice(&result, None), UndoChoice::None);
        let text = panel_after(result, None);
        assert_not_installed(&text);
        assert_no_undo(&text);
        assert!(text.contains("Install failed"), "{text}");
        assert!(
            text.contains("0 of 1 file(s) were written or may have been changed on disk"),
            "{text}"
        );
        assert!(text.contains("no file was written or changed"), "{text}");
        assert!(!text.contains("Recovery required"), "{text}");
    }

    /// The core can report WriteFailed after the pack's file already replaced
    /// the live file (its durable checkpoint failed). With one file the status
    /// is Failed, so automatic rollback never runs: the page must still say
    /// recovery is required, name the backup, and offer the gated Check Undo.
    #[test]
    fn a_write_failure_after_the_file_was_replaced_requires_recovery() {
        let mut result = with_backup(journalled(failed_result()));
        result.journal.entries[0].temporary_path = Some(
            archivefs_core::patch_manager::SharedTransactionPath::from_path(Path::new(
                "/disposable/destination/pack/.tmp",
            )),
        );
        assert_eq!(undo_choice(&result, None), UndoChoice::CheckUndo);
        let text = panel_after(result, None);
        assert_not_installed(&text);
        assert_recovery_required(&text);
        assert!(
            text.contains("1 of 1 file(s) were written or may have been changed on disk"),
            "{text}"
        );
        assert!(!text.contains("no file was written or changed"), "{text}");
        assert!(text.contains(BACKUP), "{text}");
        assert!(text.contains("Check Undo"), "{text}");
    }

    #[test]
    fn a_refused_undo_changes_nothing_and_names_each_blocked_file() {
        let mut preview = rollback_with(
            SharedRollbackOutcome::DestinationChanged,
            SharedApplyStatus::Failed,
        )
        .preview;
        preview.available = false;
        preview.entries[0].backup = Some(
            archivefs_core::patch_manager::SharedTransactionPath::from_path(Path::new(BACKUP)),
        );
        let back = state_after(journalled(with_backup(partly_failed_result())), None)
            .stage
            .unwrap();
        let mut state = Pcsx2TextureModPageState {
            key: Some((PathBuf::from(ARCHIVE), profile().profile_id)),
            stage: Some(Stage::RollbackPreview {
                preview,
                destination: PathBuf::from("/disposable/destination"),
                back: Box::new(back),
            }),
        };
        let text = panel_text(&mut state);
        assert!(text.contains("Undo refused"), "{text}");
        assert!(text.contains("Nothing was changed"), "{text}");
        assert!(
            text.contains(&format!(
                "{LIVE_FILE}: DestinationChanged. Your original is kept in EmuWiz's backup at {BACKUP}."
            )),
            "{text}"
        );
        assert!(text.contains("Back"), "{text}");
        // Nothing ran: the refused preview is still showing.
        assert!(matches!(
            state.stage,
            Some(Stage::RollbackPreview { ref preview, .. }) if !preview.available
        ));
    }

    #[test]
    fn a_failed_manual_undo_names_the_files_it_left() {
        let (sender, receiver) = mpsc::channel();
        let mut rollback = failed_rollback();
        rollback.preview.entries[0].failure = Some(SharedApplyFailure {
            kind: SharedApplyFailureKind::RollbackBlocked,
            path: None,
            detail: "This file could not be restored.".into(),
        });
        sender.send(Ok(rollback)).unwrap();
        let mut state = Pcsx2TextureModPageState {
            key: None,
            stage: Some(Stage::RollingBack(receiver)),
        };
        assert!(state.poll());
        let Some(Stage::Failed(detail)) = state.stage else {
            panic!("expected a failure");
        };
        assert!(
            detail.contains("Undo did not complete (Failed)."),
            "{detail}"
        );
        assert!(
            detail.contains(&format!(
                "{LIVE_FILE}: Failed. This file could not be restored."
            )),
            "{detail}"
        );
    }

    /// End to end through the real core, in disposable directories only: a
    /// genuine partial install, a genuine failed automatic rollback, then
    /// the page's recovery and the core's own Undo preview and rollback.
    mod end_to_end {
        use super::*;
        use archivefs_core::patch_manager::{
            Pcsx2TextureIdentity, set_test_rollback_restore_fault,
        };
        use std::fs;

        const SERIAL: &str = "SLUS-12345";

        struct Fixture {
            _root: tempfile::TempDir,
            root: PathBuf,
        }

        impl Fixture {
            /// A two-file synthetic pack and a profile whose texture folder
            /// already holds different `a.png` and `b.webp`, so both entries
            /// are replacements with backups.
            fn new() -> Self {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path().to_path_buf();
                fs::create_dir(root.join("pack")).unwrap();
                fs::write(root.join("pack/a.png"), b"pack-a").unwrap();
                fs::write(root.join("pack/b.webp"), b"pack-b").unwrap();
                let fixture = Self { _root: dir, root };
                fs::create_dir_all(fixture.textures()).unwrap();
                fs::write(fixture.file("a.png"), b"original-a").unwrap();
                fs::write(fixture.file("b.webp"), b"original-b").unwrap();
                fixture
            }

            fn profile(&self) -> Pcsx2Profile {
                Pcsx2Profile {
                    configuration_path: self.root.join("config"),
                    ..profile()
                }
            }

            fn destination(&self) -> PathBuf {
                self.root.join("config/textures")
            }

            fn textures(&self) -> PathBuf {
                self.destination().join(SERIAL)
            }

            fn file(&self, name: &str) -> PathBuf {
                self.textures().join(name)
            }

            fn backups(&self) -> PathBuf {
                self.root.join("backups")
            }

            fn history(&self) -> PathBuf {
                self.root.join("history")
            }

            fn transaction(&self) -> SharedTransactionPlan {
                let plan = build_pcsx2_texture_pack_preview(&Pcsx2TexturePackPreviewRequest {
                    selected_pack: self.root.join("pack"),
                    source_root: self.root.join("pack"),
                    identity: Pcsx2TextureIdentity {
                        archive_path: self.root.join("game.iso"),
                        serial: SERIAL.into(),
                    },
                    profile: self.profile(),
                })
                .unwrap();
                assert_eq!(plan.replace_count(), 2);
                build_pcsx2_texture_pack_transaction_plan(
                    &plan,
                    &self.profile().profile_id,
                    &plan.inspection.source_root,
                )
                .unwrap()
            }

            fn apply(&self, plan: &SharedTransactionPlan) -> Pcsx2TexturePackApplyResult {
                execute_pcsx2_texture_pack_apply(
                    plan,
                    &SharedApplyOptions {
                        dry_run: false,
                        confirmation: Some(SharedApplyConfirmation {
                            plan_id: plan.plan_id.clone(),
                            general_approved: true,
                            replacement_approved: true,
                        }),
                        operation_id: generate_shared_operation_id(),
                        timestamp_unix_seconds: 1,
                        current_context: plan.context.clone(),
                        history_root: self.history(),
                        backup_root: self.backups(),
                    },
                )
            }

            /// `a.png` is replaced; `b.webp` changes on disk after the
            /// preview, so its Apply is refused: a real PartialFailure.
            /// `fail_rollback` makes the automatic rollback's restore fail.
            fn partial_install(&self, fail_rollback: bool) -> Pcsx2TexturePackApplyResult {
                let plan = self.transaction();
                fs::write(self.file("b.webp"), b"changed-b").unwrap();
                set_test_rollback_restore_fault(fail_rollback);
                let result = self.apply(&plan);
                set_test_rollback_restore_fault(false);
                assert_eq!(
                    result.apply.journal.status,
                    SharedApplyStatus::PartialFailure
                );
                result
            }

            /// The page after the real install worker's result arrived.
            fn page(&self, result: Pcsx2TexturePackApplyResult) -> Pcsx2TextureModPageState {
                let (sender, receiver) = mpsc::channel();
                sender.send(Ok(result)).unwrap();
                let mut state = Pcsx2TextureModPageState {
                    key: Some((PathBuf::from(ARCHIVE), self.profile().profile_id)),
                    stage: Some(Stage::Applying {
                        receiver,
                        destination: self.destination(),
                    }),
                };
                assert!(state.poll());
                state
            }

            fn text(&self, state: &mut Pcsx2TextureModPageState) -> String {
                let profile = self.profile();
                render(|ui| {
                    show_pcsx2_texture_mod_panel(ui, state, Path::new(ARCHIVE), &profile, None)
                })
            }

            /// What Check Undo opens: the core's preview against the
            /// disposable backup folder.
            fn check_undo(&self, state: &mut Pcsx2TextureModPageState) {
                let Some(back @ Stage::Applied { .. }) = state.stage.take() else {
                    panic!("expected the applied stage");
                };
                let Stage::Applied {
                    ref result,
                    ref rollback,
                    ..
                } = back
                else {
                    unreachable!()
                };
                assert_eq!(
                    undo_choice(result, rollback.as_ref()),
                    UndoChoice::CheckUndo
                );
                let journal = result.journal_path.clone().unwrap();
                state.stage = Some(undo_preview(
                    &journal,
                    self.destination(),
                    &self.backups(),
                    back,
                ));
            }

            fn preview(state: &Pcsx2TextureModPageState) -> &SharedRollbackPreview {
                match &state.stage {
                    Some(Stage::RollbackPreview { preview, .. }) => preview,
                    _ => panic!("expected the Undo preview"),
                }
            }

            /// Every file under the texture folder, with its bytes.
            fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
                let mut files: Vec<_> = fs::read_dir(self.textures())
                    .unwrap()
                    .map(|entry| {
                        let entry = entry.unwrap();
                        (
                            entry.file_name().to_string_lossy().into_owned(),
                            fs::read(entry.path()).unwrap(),
                        )
                    })
                    .collect();
                files.sort();
                files
            }

            fn backup_of_a(&self, result: &SharedApplyResult) -> PathBuf {
                result.journal.entries[0]
                    .backup_path
                    .as_ref()
                    .unwrap()
                    .to_path_buf()
                    .unwrap()
            }
        }

        fn files(pairs: &[(&str, &[u8])]) -> Vec<(String, Vec<u8>)> {
            pairs
                .iter()
                .map(|(name, bytes)| (name.to_string(), bytes.to_vec()))
                .collect()
        }

        /// The state a failed automatic rollback leaves, and the page for it.
        fn after_failed_rollback(fixture: &Fixture) -> (Pcsx2TextureModPageState, PathBuf) {
            let result = fixture.partial_install(true);
            let rollback = result.rollback.as_ref().expect("automatic rollback ran");
            assert_ne!(rollback.status, SharedApplyStatus::Success);
            assert_eq!(
                fixture.snapshot(),
                files(&[("a.png", b"pack-a"), ("b.webp", b"changed-b")])
            );
            let backup = fixture.backup_of_a(&result.apply);
            assert_eq!(fs::read(&backup).unwrap(), b"original-a");
            let mut state = fixture.page(result);
            let text = fixture.text(&mut state);
            assert!(!text.contains("Installed"), "{text}");
            assert!(text.contains("Install incomplete"), "{text}");
            assert!(text.contains("Recovery required"), "{text}");
            assert!(
                text.contains("Automatic rollback did not complete"),
                "{text}"
            );
            assert!(
                text.contains(&format!(
                    "{}: may still be the pack's file; your original is kept in EmuWiz's backup at {}",
                    fixture.file("a.png").display(),
                    backup.display()
                )),
                "{text}"
            );
            // b.webp was never written, so it is not listed as remaining.
            assert!(
                !text.contains(&format!("{}:", fixture.file("b.webp").display())),
                "{text}"
            );
            assert!(text.contains("Check Undo"), "{text}");
            (state, backup)
        }

        #[test]
        fn a_successful_install_is_installed_with_undo() {
            let fixture = Fixture::new();
            let result = fixture.apply(&fixture.transaction());
            assert_eq!(result.apply.journal.status, SharedApplyStatus::Success);
            assert!(result.rollback.is_none());
            let text = fixture.text(&mut fixture.page(result));
            assert!(text.contains("Installed"), "{text}");
            assert!(text.contains("Undo"), "{text}");
            assert!(!text.contains("Recovery required"), "{text}");
            assert_eq!(
                fixture.snapshot(),
                files(&[("a.png", b"pack-a"), ("b.webp", b"pack-b")])
            );
        }

        #[test]
        fn a_fully_rolled_back_install_is_not_installed_and_offers_no_undo() {
            let fixture = Fixture::new();
            let result = fixture.partial_install(false);
            assert_eq!(
                result.rollback.as_ref().map(|r| r.status),
                Some(SharedApplyStatus::Success)
            );
            assert_eq!(
                fixture.snapshot(),
                files(&[("a.png", b"original-a"), ("b.webp", b"changed-b")])
            );
            let text = fixture.text(&mut fixture.page(result));
            assert!(!text.contains("Installed"), "{text}");
            assert!(!text.contains("Undo"), "{text}");
            assert!(!text.contains("Recovery required"), "{text}");
            assert!(
                text.contains("reversed this install's recorded changes"),
                "{text}"
            );
        }

        #[test]
        fn after_a_failed_rollback_a_retried_undo_restores_the_original_and_nothing_else() {
            let fixture = Fixture::new();
            let (mut state, _backup) = after_failed_rollback(&fixture);
            fixture.check_undo(&mut state);
            let text = fixture.text(&mut state);
            assert!(!text.contains("Undo refused"), "{text}");
            assert!(text.contains("Confirm undo"), "{text}");
            let preview = Fixture::preview(&state).clone();
            assert!(preview.available);
            let result = execute_shared_rollback(
                &preview,
                &SharedRollbackOptions {
                    confirmation: SharedRollbackConfirmation {
                        preview_id: preview.preview_id.clone(),
                        approved: true,
                    },
                    rollback_operation_id: generate_shared_operation_id(),
                    timestamp_unix_seconds: 2,
                    history_root: fixture.history(),
                    backup_root: fixture.backups(),
                },
            );
            assert_eq!(result.status, SharedApplyStatus::Success);
            // The original is back; the file Apply never touched keeps the
            // user's change; nothing else appeared.
            assert_eq!(
                fixture.snapshot(),
                files(&[("a.png", b"original-a"), ("b.webp", b"changed-b")])
            );
            let (sender, receiver) = mpsc::channel();
            sender.send(Ok(result)).unwrap();
            state.stage = Some(Stage::RollingBack(receiver));
            assert!(state.poll());
            assert!(
                state.stage.is_none(),
                "a finished Undo returns to the start"
            );
        }

        fn assert_refused(fixture: &Fixture, state: &mut Pcsx2TextureModPageState, outcome: &str) {
            let before = fixture.snapshot();
            fixture.check_undo(state);
            assert!(!Fixture::preview(state).available);
            let text = fixture.text(state);
            assert!(text.contains("Undo refused"), "{text}");
            assert!(text.contains("Nothing was changed"), "{text}");
            assert!(
                text.contains(&format!("{}: {outcome}.", fixture.file("a.png").display())),
                "{text}"
            );
            assert!(text.contains("Back"), "{text}");
            assert_eq!(fixture.snapshot(), before, "a refused Undo changed files");
        }

        #[test]
        fn a_missing_backup_refuses_undo_without_guessing() {
            let fixture = Fixture::new();
            let (mut state, backup) = after_failed_rollback(&fixture);
            fs::remove_file(&backup).unwrap();
            assert_refused(&fixture, &mut state, "BackupMissing");
            assert_eq!(fs::read(fixture.file("a.png")).unwrap(), b"pack-a");
        }

        #[test]
        fn a_modified_backup_refuses_undo_without_guessing() {
            let fixture = Fixture::new();
            let (mut state, backup) = after_failed_rollback(&fixture);
            fs::write(&backup, b"tampered").unwrap();
            assert_refused(&fixture, &mut state, "BackupChanged");
            assert_eq!(fs::read(&backup).unwrap(), b"tampered");
        }

        #[test]
        fn a_file_changed_after_the_failed_rollback_refuses_undo() {
            let fixture = Fixture::new();
            let (mut state, backup) = after_failed_rollback(&fixture);
            fs::write(fixture.file("a.png"), b"edited-by-user").unwrap();
            assert_refused(&fixture, &mut state, "DestinationChanged");
            assert_eq!(fs::read(&backup).unwrap(), b"original-a");
        }
    }
}
