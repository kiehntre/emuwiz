//! PCSX2 texture replacements embedded in Cheats & Mods.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::patch_manager::{
    Pcsx2Profile, Pcsx2TexturePackApplyResult, Pcsx2TexturePackPlan,
    Pcsx2TexturePackPreviewRequest, SharedApplyConfirmation, SharedApplyOptions, SharedApplyResult,
    SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions, SharedRollbackPreview,
    SharedRollbackResult, SharedTransactionPlan, build_pcsx2_texture_pack_preview,
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
    Applied {
        result: SharedApplyResult,
        destination: PathBuf,
    },
    RollbackPreview {
        preview: SharedRollbackPreview,
        destination: PathBuf,
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
                    self.stage = Some(Stage::Applied {
                        result: result.apply,
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
                        Some(Stage::Failed(format!(
                            "Undo did not complete: {:?}",
                            result.status
                        )))
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
                destination,
            }) => {
                widgets::status_badge(ui, "Installed", widgets::StatusTone::Success);
                if let Some(journal) = result.journal_path.clone() {
                    if widgets::action_button(ui, "Undo", widgets::ActionStyle::Destructive, true)
                        .clicked()
                    {
                        match (
                            default_shared_backup_root(),
                            preview_shared_rollback(
                                &journal,
                                &destination,
                                &default_shared_backup_root().unwrap_or_default(),
                            ),
                        ) {
                            (Ok(_backup), preview) => {
                                state.stage = Some(Stage::RollbackPreview {
                                    preview,
                                    destination: destination.clone(),
                                })
                            }
                            (Err(error), _) => state.stage = Some(Stage::Failed(error.detail)),
                        }
                    } else {
                        state.stage = Some(Stage::Applied {
                            result,
                            destination,
                        });
                    }
                    let _ = journal;
                } else {
                    state.stage = Some(Stage::Applied {
                        result,
                        destination,
                    });
                }
            }
            Some(Stage::RollbackPreview {
                preview,
                destination,
            }) => {
                ui.label(if preview.available {
                    "Undo removes only unchanged EmuWiz-created files and restores safe backups."
                } else {
                    "Undo is refused because the destination changed."
                });
                if widgets::action_button(
                    ui,
                    "Confirm undo",
                    widgets::ActionStyle::Destructive,
                    preview.available,
                )
                .clicked()
                {
                    state.stage = Some(spawn_rollback(preview));
                } else {
                    state.stage = Some(Stage::RollbackPreview {
                        preview,
                        destination,
                    });
                }
            }
            Some(other) => {
                state.stage = Some(other);
            }
        }
    });
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
