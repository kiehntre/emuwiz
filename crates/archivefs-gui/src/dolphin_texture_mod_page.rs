//! The Dolphin texture-mod panel: install exactly one explicitly selected
//! PNG texture file, or an explicit versioned multi-file texture-pack
//! manifest, into
//! `<Dolphin profile>/Load/Textures/<verified GameID>/<original filename>.png`,
//! through the existing shared preview/transaction pipeline
//! (`archivefs_core::patch_manager::dolphin_texture_mod`).
//!
//! # Scope
//!
//! The multi-file path accepts only the explicit JSON manifest contract from
//! `archivefs_core::patch_manager`; it never guesses directories or unpacks
//! archives. See
//! `archivefs_core::patch_manager::dolphin_texture_mod`'s own module doc
//! comment for the full list this mirrors.
//!
//! # No filesystem writes here
//!
//! This module never calls `std::fs::write`/`std::fs::remove_file`/etc.
//! itself. Every mutation goes through
//! `archivefs_core::patch_manager::execute_shared_apply`/
//! `execute_shared_rollback`, run on a background thread and polled here -
//! exactly the same discipline every other shared-transaction-backed page
//! in this app already follows. Nothing here is automatic: no file picker
//! opens, no preview is built, and no install happens without the explicit
//! button click each step names.
//!
//! # Deliberately separate from `CheatWorkflowState`
//!
//! This is its own state, owned directly by `ArchiveFsApp`, not a field on
//! the Cheats & Mods cheat-workflow machinery - a texture-mod install is
//! not a cheat, and reusing that state would tie its lifetime to unrelated
//! cheat-workflow resets. It is instead keyed by
//! `{ archive_path, profile_id, verified_game_id }` (see
//! [`DolphinTextureModKey`]) so switching the selected archive, Dolphin
//! profile, or resolved GameID always starts fresh rather than reusing
//! state that describes a different game.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::patch_manager::{
    DOLPHIN_TEXTURE_MOD_SOURCE_MODE, DOLPHIN_TEXTURE_PACK_MAX_MANIFEST_BYTES, DolphinProfile,
    DolphinTextureModIdentity, DolphinTextureModPlan, DolphinTextureModPreviewRequest,
    DolphinTexturePackApplyResult, DolphinTexturePackBuildPreview, DolphinTexturePackBuildRequest,
    DolphinTexturePackManifest, DolphinTexturePackPlan, SharedApplyConfirmation,
    SharedApplyOptions, SharedApplyOutcome, SharedApplyResult, SharedApplyStatus,
    SharedRollbackConfirmation, SharedRollbackOptions, SharedRollbackPreview, SharedRollbackResult,
    SharedTransactionPlan, build_dolphin_texture_mod_preview, build_dolphin_texture_pack_manifest,
    build_dolphin_texture_pack_preview, build_dolphin_texture_pack_transaction_plan,
    build_shared_transaction_plan, default_shared_backup_root, default_shared_history_root,
    dolphin_texture_mod_destination_root, execute_dolphin_texture_pack_apply, execute_shared_apply,
    execute_shared_rollback, generate_shared_operation_id, preview_shared_rollback,
    validate_dolphin_texture_source, verified_dolphin_texture_identity,
};
use eframe::egui;

use crate::ui::components as widgets;

/// Identifies exactly which game/profile a [`DolphinTextureModPageState`]'s
/// current stage describes. Compared every frame against the caller's
/// current archive/profile/GameID (see [`DolphinTextureModPageState::sync`]);
/// any difference resets the whole page back to its idle starting point,
/// so state from a previously selected game or profile is never reused for
/// a different one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DolphinTextureModKey {
    archive_path: PathBuf,
    profile_id: String,
    verified_game_id: String,
}

enum DolphinTextureModStage {
    PickingFile {
        receiver: Receiver<Option<PathBuf>>,
        archive_path: PathBuf,
        identity: DolphinTextureModIdentity,
        destination_root: PathBuf,
    },
    PickingPackManifest {
        receiver: Receiver<Option<PathBuf>>,
        archive_path: PathBuf,
        identity: DolphinTextureModIdentity,
        destination_root: PathBuf,
    },
    PickingPackDirectory {
        receiver: Receiver<Option<PathBuf>>,
        archive_path: PathBuf,
        identity: DolphinTextureModIdentity,
        destination_root: PathBuf,
    },
    PackBuilderForm {
        source_root: PathBuf,
        archive_path: PathBuf,
        identity: DolphinTextureModIdentity,
        destination_root: PathBuf,
    },
    PackBuilderPreview {
        preview: DolphinTexturePackBuildPreview,
        source_root: PathBuf,
        archive_path: PathBuf,
        identity: DolphinTextureModIdentity,
        destination_root: PathBuf,
    },
    PackManifestSaved {
        path: PathBuf,
    },
    PreviewReady {
        plan: DolphinTextureModPlan,
        source_parent: PathBuf,
    },
    PackPreviewReady {
        plan: DolphinTexturePackPlan,
        manifest_path: PathBuf,
    },
    ConfirmationPending {
        plan: SharedTransactionPlan,
    },
    PackConfirmationPending {
        plan: SharedTransactionPlan,
    },
    Applying {
        receiver: Receiver<SharedApplyResult>,
        destination_root: PathBuf,
    },
    PackApplying {
        receiver: Receiver<DolphinTexturePackApplyResult>,
        destination_root: PathBuf,
    },
    Applied {
        result: SharedApplyResult,
        destination_root: PathBuf,
    },
    RollbackPreview {
        preview: SharedRollbackPreview,
    },
    RollingBack {
        receiver: Receiver<SharedRollbackResult>,
    },
    Failed {
        detail: String,
    },
}

#[derive(Default)]
pub(crate) struct DolphinTextureModPageState {
    key: Option<DolphinTextureModKey>,
    stage: Option<DolphinTextureModStage>,
    builder_name: String,
    builder_version: String,
}

impl DolphinTextureModPageState {
    fn reset(&mut self) {
        self.key = None;
        self.stage = None;
        self.builder_name.clear();
        self.builder_version.clear();
    }

    /// Resets to the idle starting point whenever `key` no longer matches
    /// the currently displayed game/profile - see this struct's own doc
    /// comment.
    fn sync(&mut self, key: DolphinTextureModKey) {
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.stage = None;
            self.builder_name.clear();
            self.builder_version.clear();
        }
    }

    /// Whether a background operation is in flight - the caller should
    /// keep repainting while this holds.
    pub(crate) fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            Some(
                DolphinTextureModStage::PickingFile { .. }
                    | DolphinTextureModStage::PickingPackManifest { .. }
                    | DolphinTextureModStage::PickingPackDirectory { .. }
                    | DolphinTextureModStage::Applying { .. }
                    | DolphinTextureModStage::PackApplying { .. }
                    | DolphinTextureModStage::RollingBack { .. }
            )
        )
    }

    /// Drains whichever background channel is currently active, if any.
    /// Returns whether anything changed (so the caller can request a
    /// repaint).
    pub(crate) fn poll(&mut self) -> bool {
        match self.stage.take() {
            Some(DolphinTextureModStage::PickingFile {
                receiver,
                archive_path,
                identity,
                destination_root,
            }) => match receiver.try_recv() {
                Ok(Some(path)) => {
                    self.stage = Some(build_preview_stage(
                        &path,
                        &archive_path,
                        &identity,
                        &destination_root,
                    ));
                    true
                }
                Ok(None) => {
                    // User cancelled the dialog - back to idle.
                    self.stage = None;
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Some(DolphinTextureModStage::PickingFile {
                        receiver,
                        archive_path,
                        identity,
                        destination_root,
                    });
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = None;
                    true
                }
            },
            Some(DolphinTextureModStage::PickingPackManifest {
                receiver,
                archive_path,
                identity,
                destination_root,
            }) => match receiver.try_recv() {
                Ok(Some(path)) => {
                    self.stage = Some(build_pack_preview_stage(
                        &path,
                        &archive_path,
                        &identity,
                        &destination_root,
                    ));
                    true
                }
                Ok(None) => {
                    self.stage = None;
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Some(DolphinTextureModStage::PickingPackManifest {
                        receiver,
                        archive_path,
                        identity,
                        destination_root,
                    });
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = None;
                    true
                }
            },
            Some(DolphinTextureModStage::PickingPackDirectory {
                receiver,
                archive_path,
                identity,
                destination_root,
            }) => match receiver.try_recv() {
                Ok(Some(source_root)) => {
                    self.stage = Some(DolphinTextureModStage::PackBuilderForm {
                        source_root,
                        archive_path,
                        identity,
                        destination_root,
                    });
                    true
                }
                Ok(None) => {
                    self.stage = None;
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Some(DolphinTextureModStage::PickingPackDirectory {
                        receiver,
                        archive_path,
                        identity,
                        destination_root,
                    });
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = None;
                    true
                }
            },
            Some(DolphinTextureModStage::Applying {
                receiver,
                destination_root,
            }) => match receiver.try_recv() {
                Ok(result) => {
                    self.stage = Some(DolphinTextureModStage::Applied {
                        result,
                        destination_root,
                    });
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Some(DolphinTextureModStage::Applying {
                        receiver,
                        destination_root,
                    });
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(DolphinTextureModStage::Failed {
                        detail: "the install worker stopped without reporting a result".to_string(),
                    });
                    true
                }
            },
            Some(DolphinTextureModStage::PackApplying {
                receiver,
                destination_root,
            }) => match receiver.try_recv() {
                Ok(result) => {
                    if result.apply.journal.status == SharedApplyStatus::Success {
                        self.stage = Some(DolphinTextureModStage::Applied {
                            result: result.apply,
                            destination_root,
                        });
                    } else {
                        let rollback = result
                            .rollback
                            .map(|rollback| format!(" Automatic rollback: {:?}.", rollback.status))
                            .unwrap_or_default();
                        self.stage = Some(DolphinTextureModStage::Failed {
                            detail: format!(
                                "texture-pack installation failed ({:?}).{}",
                                result.apply.journal.status, rollback
                            ),
                        });
                    }
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Some(DolphinTextureModStage::PackApplying {
                        receiver,
                        destination_root,
                    });
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(DolphinTextureModStage::Failed {
                        detail:
                            "the texture-pack install worker stopped without reporting a result"
                                .to_string(),
                    });
                    true
                }
            },
            Some(DolphinTextureModStage::RollingBack { receiver }) => match receiver.try_recv() {
                Ok(result) => {
                    self.stage = if result.status == SharedApplyStatus::Success {
                        None
                    } else {
                        Some(DolphinTextureModStage::Failed {
                            detail: format!("undo did not complete: {:?}", result.status),
                        })
                    };
                    true
                }
                Err(TryRecvError::Empty) => {
                    self.stage = Some(DolphinTextureModStage::RollingBack { receiver });
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(DolphinTextureModStage::Failed {
                        detail: "the undo worker stopped without reporting a result".to_string(),
                    });
                    true
                }
            },
            other => {
                self.stage = other;
                false
            }
        }
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Turns a freshly picked file into a real preview, re-validating the
/// source from scratch (never trusting anything about it beyond its path)
/// and building the exact narrow preview request this feature always uses.
fn build_preview_stage(
    path: &Path,
    archive_path: &Path,
    identity: &DolphinTextureModIdentity,
    destination_root: &Path,
) -> DolphinTextureModStage {
    let source = match validate_dolphin_texture_source(path) {
        Ok(source) => source,
        Err(error) => {
            return DolphinTextureModStage::Failed {
                detail: error.detail,
            };
        }
    };
    let source_parent = source
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    let request = DolphinTextureModPreviewRequest {
        selected_archive: archive_path.to_path_buf(),
        identity: identity.clone(),
        destination_root: destination_root.to_path_buf(),
        source,
    };
    match build_dolphin_texture_mod_preview(&request) {
        Ok(plan) => DolphinTextureModStage::PreviewReady {
            plan,
            source_parent,
        },
        Err(error) => DolphinTextureModStage::Failed {
            detail: error.detail,
        },
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Draws the panel. `identity_report` is the already-loaded, already
/// staleness-checked `GameIdentityReport` for `archive_path` (the caller's
/// own `ready_game_identity`-equivalent gate); `profile` is the currently
/// selected, already-discovered Dolphin profile - this function never
/// triggers a Dolphin scan itself.
pub(crate) fn show_dolphin_texture_mod_panel(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    archive_path: &Path,
    profile: &DolphinProfile,
    identity_report: Option<&GameIdentityReport>,
) {
    widgets::section_header(
        ui,
        "Dolphin texture mod",
        Some(
            "Install one PNG hires-texture file into this game's Load/Textures folder. \
             Planning only until you explicitly confirm.",
        ),
    );

    let Some(report) = identity_report else {
        state.reset();
        widgets::card(ui, |ui| {
            ui.label("Load ROM Identity & Evidence first.");
        });
        return;
    };
    let identity = match verified_dolphin_texture_identity(report, archive_path) {
        Ok(identity) => identity,
        Err(error) => {
            state.reset();
            widgets::banner(
                ui,
                "Identity unavailable",
                &error.detail,
                widgets::StatusTone::Pending,
            );
            return;
        }
    };
    let destination_root = match dolphin_texture_mod_destination_root(profile) {
        Ok(root) => root,
        Err(error) => {
            state.reset();
            widgets::banner(
                ui,
                "Dolphin profile unavailable",
                &error.detail,
                widgets::StatusTone::Pending,
            );
            return;
        }
    };

    state.sync(DolphinTextureModKey {
        archive_path: archive_path.to_path_buf(),
        profile_id: profile.profile_id.clone(),
        verified_game_id: identity.game_id.clone(),
    });

    match state.stage.take() {
        None => show_idle(ui, state, archive_path, &identity, &destination_root),
        Some(DolphinTextureModStage::PickingFile {
            receiver,
            archive_path,
            identity,
            destination_root,
        }) => {
            widgets::card(ui, |ui| {
                ui.label("Waiting for file selection…");
            });
            state.stage = Some(DolphinTextureModStage::PickingFile {
                receiver,
                archive_path,
                identity,
                destination_root,
            });
        }
        Some(DolphinTextureModStage::PickingPackManifest {
            receiver,
            archive_path,
            identity,
            destination_root,
        }) => {
            widgets::card(ui, |ui| {
                ui.label("Waiting for texture-pack manifest selection…");
            });
            state.stage = Some(DolphinTextureModStage::PickingPackManifest {
                receiver,
                archive_path,
                identity,
                destination_root,
            });
        }
        Some(DolphinTextureModStage::PickingPackDirectory {
            receiver,
            archive_path,
            identity,
            destination_root,
        }) => {
            widgets::card(ui, |ui| {
                ui.label("Waiting for texture-pack directory selection…");
            });
            state.stage = Some(DolphinTextureModStage::PickingPackDirectory {
                receiver,
                archive_path,
                identity,
                destination_root,
            });
        }
        Some(DolphinTextureModStage::PackBuilderForm {
            source_root,
            archive_path,
            identity,
            destination_root,
        }) => show_pack_builder_form(
            ui,
            state,
            source_root,
            archive_path,
            identity,
            destination_root,
        ),
        Some(DolphinTextureModStage::PackBuilderPreview {
            preview,
            source_root,
            archive_path,
            identity,
            destination_root,
        }) => show_pack_builder_preview(
            ui,
            state,
            preview,
            source_root,
            archive_path,
            identity,
            destination_root,
        ),
        Some(DolphinTextureModStage::PackManifestSaved { path }) => {
            let mut done = false;
            widgets::card(ui, |ui| {
                widgets::status_badge(ui, "Manifest saved", widgets::StatusTone::Success);
                widgets::path_value(ui, "Manifest", &path);
                ui.label(
                    "Use Choose texture-pack manifest when you are ready to preview installation.",
                );
                if widgets::action_button(ui, "Done", widgets::ActionStyle::Quiet, true).clicked() {
                    done = true;
                }
            });
            if !done {
                state.stage = Some(DolphinTextureModStage::PackManifestSaved { path });
            }
        }
        Some(DolphinTextureModStage::PreviewReady {
            plan,
            source_parent,
        }) => show_preview(ui, state, &plan, &source_parent, &profile.profile_id),
        Some(DolphinTextureModStage::PackPreviewReady {
            plan,
            manifest_path,
        }) => show_pack_preview(ui, state, &plan, &manifest_path, &profile.profile_id),
        Some(DolphinTextureModStage::ConfirmationPending { plan }) => {
            show_confirmation(ui, state, plan, &destination_root)
        }
        Some(DolphinTextureModStage::PackConfirmationPending { plan }) => {
            show_pack_confirmation(ui, state, plan, &destination_root)
        }
        Some(DolphinTextureModStage::Applying {
            receiver,
            destination_root,
        }) => {
            widgets::card(ui, |ui| {
                ui.label("Installing…");
            });
            state.stage = Some(DolphinTextureModStage::Applying {
                receiver,
                destination_root,
            });
        }
        Some(DolphinTextureModStage::PackApplying {
            receiver,
            destination_root,
        }) => {
            widgets::card(ui, |ui| {
                ui.label("Installing texture pack…");
            });
            state.stage = Some(DolphinTextureModStage::PackApplying {
                receiver,
                destination_root,
            });
        }
        Some(DolphinTextureModStage::Applied {
            result,
            destination_root,
        }) => show_applied(ui, state, result, destination_root),
        Some(DolphinTextureModStage::RollbackPreview { preview }) => {
            show_rollback_preview(ui, state, preview)
        }
        Some(DolphinTextureModStage::RollingBack { receiver }) => {
            widgets::card(ui, |ui| {
                ui.label("Undoing…");
            });
            state.stage = Some(DolphinTextureModStage::RollingBack { receiver });
        }
        Some(DolphinTextureModStage::Failed { detail }) => {
            widgets::banner(
                ui,
                "Texture install failed",
                &detail,
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Try again", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                state.stage = None;
            }
        }
    }
}

fn show_idle(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    archive_path: &Path,
    identity: &DolphinTextureModIdentity,
    destination_root: &Path,
) {
    widgets::card(ui, |ui| {
        ui.label(format!(
            "Verified GameID: {} · Destination: {}",
            identity.game_id,
            destination_root.display()
        ));
        if widgets::action_button(ui, "Choose PNG", widgets::ActionStyle::Secondary, true).clicked()
        {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter("PNG texture", &["png"])
                    .pick_file();
                let _ = sender.send(picked);
            });
            state.stage = Some(DolphinTextureModStage::PickingFile {
                receiver,
                archive_path: archive_path.to_path_buf(),
                identity: identity.clone(),
                destination_root: destination_root.to_path_buf(),
            });
        }
        if widgets::action_button(
            ui,
            "Choose texture-pack manifest",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked()
        {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter("Dolphin texture-pack manifest", &["json"])
                    .pick_file();
                let _ = sender.send(picked);
            });
            state.stage = Some(DolphinTextureModStage::PickingPackManifest {
                receiver,
                archive_path: archive_path.to_path_buf(),
                identity: identity.clone(),
                destination_root: destination_root.to_path_buf(),
            });
        }
        if widgets::action_button(
            ui,
            "Build texture-pack manifest",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked()
        {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let picked = rfd::FileDialog::new().pick_folder();
                let _ = sender.send(picked);
            });
            state.stage = Some(DolphinTextureModStage::PickingPackDirectory {
                receiver,
                archive_path: archive_path.to_path_buf(),
                identity: identity.clone(),
                destination_root: destination_root.to_path_buf(),
            });
        }
    });
}

fn show_pack_builder_form(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    source_root: PathBuf,
    archive_path: PathBuf,
    identity: DolphinTextureModIdentity,
    destination_root: PathBuf,
) {
    let mut scan = false;
    let mut cancel = false;
    widgets::card(ui, |ui| {
        ui.strong("Build a texture-pack manifest");
        widgets::path_value(ui, "Source directory", &source_root);
        ui.label(format!("Verified target GameID: {}", identity.game_id));
        ui.horizontal(|ui| {
            ui.label("Pack name");
            ui.text_edit_singleline(&mut state.builder_name);
        });
        ui.horizontal(|ui| {
            ui.label("Version (optional)");
            ui.text_edit_singleline(&mut state.builder_version);
        });
        if state.builder_name.trim().is_empty() {
            ui.label("Enter a pack name before scanning.");
        }
        if widgets::action_button(
            ui,
            "Scan directory",
            widgets::ActionStyle::Primary,
            !state.builder_name.trim().is_empty(),
        )
        .clicked()
        {
            scan = true;
        }
        if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
            cancel = true;
        }
    });
    if cancel {
        state.stage = None;
    }
    if scan {
        let request = DolphinTexturePackBuildRequest {
            source_root: source_root.clone(),
            identity: identity.clone(),
            name: state.builder_name.trim().to_string(),
            version: (!state.builder_version.trim().is_empty())
                .then(|| state.builder_version.trim().to_string()),
        };
        match build_dolphin_texture_pack_manifest(&request) {
            Ok(preview) => {
                state.stage = Some(DolphinTextureModStage::PackBuilderPreview {
                    preview,
                    source_root,
                    archive_path,
                    identity,
                    destination_root,
                })
            }
            Err(error) => {
                state.stage = Some(DolphinTextureModStage::Failed {
                    detail: error.detail,
                })
            }
        }
    }
}

fn show_pack_builder_preview(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    preview: DolphinTexturePackBuildPreview,
    source_root: PathBuf,
    archive_path: PathBuf,
    identity: DolphinTextureModIdentity,
    destination_root: PathBuf,
) {
    let mut save = false;
    let mut back = false;
    widgets::card(ui, |ui| {
        ui.strong(format!("Manifest preview: {}", preview.manifest.name));
        if let Some(version) = &preview.manifest.version {
            ui.label(format!("Version: {version}"));
        }
        ui.label(format!(
            "Verified target GameID: {}",
            preview.manifest.target_game_id
        ));
        ui.label(format!(
            "Accepted PNG files: {} · Total bytes: {}",
            preview.manifest.files.len(),
            preview.total_bytes
        ));
        if preview.rejected.is_empty() {
            widgets::status_badge(ui, "Ready to save", widgets::StatusTone::Success);
        } else {
            widgets::banner(
                ui,
                "Some files were not accepted",
                &format!(
                    "{} file(s) rejected; nested paths and unsupported files are listed below.",
                    preview.rejected.len()
                ),
                widgets::StatusTone::Pending,
            );
            for item in preview.rejected.iter().take(10) {
                ui.label(format!(
                    "{} — {}",
                    item.relative_path.display(),
                    item.reason
                ));
            }
        }
        if widgets::action_button(
            ui,
            "Save manifest",
            widgets::ActionStyle::Primary,
            preview.complete,
        )
        .clicked()
        {
            save = true;
        }
        if widgets::action_button(ui, "Back", widgets::ActionStyle::Quiet, true).clicked() {
            back = true;
        }
    });
    if back {
        state.stage = Some(DolphinTextureModStage::PackBuilderForm {
            source_root,
            archive_path,
            identity,
            destination_root,
        });
    }
    if save {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Dolphin texture-pack manifest", &["json"])
            .save_file()
        {
            match save_pack_manifest(&path, &preview.manifest) {
                Ok(()) => state.stage = Some(DolphinTextureModStage::PackManifestSaved { path }),
                Err(error) => {
                    state.stage = Some(DolphinTextureModStage::Failed {
                        detail: format!("could not save manifest: {error}"),
                    })
                }
            }
        }
    }
}

fn save_pack_manifest(path: &Path, manifest: &DolphinTexturePackManifest) -> Result<(), String> {
    if path.exists() {
        return Err("refusing to overwrite an existing manifest".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "manifest has no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let data = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("manifest"),
        now_unix_seconds()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    use std::io::Write;
    file.write_all(&data).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    if path.exists() {
        let _ = std::fs::remove_file(&temp);
        return Err("refusing to overwrite an existing manifest".to_string());
    }
    let result = std::fs::hard_link(&temp, path).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&temp);
    result
}

fn build_pack_preview_stage(
    manifest_path: &Path,
    archive_path: &Path,
    identity: &DolphinTextureModIdentity,
    destination_root: &Path,
) -> DolphinTextureModStage {
    let manifest = match read_pack_manifest(manifest_path)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            serde_json::from_str::<DolphinTexturePackManifest>(&text)
                .map_err(|error| error.to_string())
        }) {
        Ok(manifest) => manifest,
        Err(detail) => {
            return DolphinTextureModStage::Failed {
                detail: format!("could not read texture-pack manifest: {detail}"),
            };
        }
    };
    let source_root = manifest.source_root.clone();
    let request = archivefs_core::patch_manager::DolphinTexturePackPreviewRequest {
        selected_archive: archive_path.to_path_buf(),
        identity: identity.clone(),
        destination_root: destination_root.to_path_buf(),
        source_root,
        manifest,
    };
    match build_dolphin_texture_pack_preview(&request) {
        Ok(plan) => DolphinTextureModStage::PackPreviewReady {
            plan,
            manifest_path: manifest_path.to_path_buf(),
        },
        Err(error) => DolphinTextureModStage::Failed {
            detail: error.detail,
        },
    }
}

fn read_pack_manifest(path: &Path) -> Result<String, std::io::Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "manifest must be a regular non-symlink file",
        ));
    }
    if metadata.len() > DOLPHIN_TEXTURE_PACK_MAX_MANIFEST_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "manifest exceeds the 1 MiB safety bound",
        ));
    }
    std::fs::read_to_string(path)
}

fn show_pack_preview(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    plan: &DolphinTexturePackPlan,
    manifest_path: &Path,
    profile_id: &str,
) {
    widgets::card(ui, |ui| {
        ui.strong(format!("Texture pack: {}", plan.manifest.name));
        if let Some(version) = &plan.manifest.version {
            ui.label(format!("Version: {version}"));
        }
        widgets::path_value(ui, "Manifest", manifest_path);
        ui.label(format!(
            "Verified target GameID: {}",
            plan.manifest.target_game_id
        ));
        ui.label(format!("Files in manifest: {}", plan.manifest.files.len()));
        ui.label(format!("Files to install: {}", plan.install_count()));
        ui.label(format!(
            "Already installed identically: {}",
            plan.already_installed_count()
        ));
        ui.label(format!("Replacements: {}", plan.replacement_count()));
        let hard_conflicts = plan.report.conflicts.len();
        let blocked = plan.report.summary.blocked;
        if hard_conflicts > 0 || blocked > 0 {
            widgets::banner(
                ui,
                "Texture pack cannot be installed",
                &format!(
                    "{hard_conflicts} hard conflict(s), {blocked} file(s) blocked by safety checks."
                ),
                widgets::StatusTone::Blocked,
            );
        } else if plan.is_applyable() {
            widgets::status_badge(ui, "Ready to install", widgets::StatusTone::Success);
            if widgets::action_button(
                ui,
                "Review and install",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                match build_dolphin_texture_pack_transaction_plan(
                    plan,
                    profile_id,
                    &plan.manifest.source_root,
                ) {
                    Ok(transaction) => {
                        state.stage = Some(DolphinTextureModStage::PackConfirmationPending {
                            plan: transaction,
                        });
                    }
                    Err(error) => {
                        state.stage = Some(DolphinTextureModStage::Failed {
                            detail: error.detail,
                        });
                    }
                }
            }
        }
        if widgets::action_button(
            ui,
            "Choose a different manifest",
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
        {
            state.stage = None;
        }
    });
}

fn show_pack_confirmation(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    plan: SharedTransactionPlan,
    destination_root: &Path,
) {
    let mut cancel = false;
    let mut confirm = false;
    widgets::card(ui, |ui| {
        ui.label(format!(
            "Confirm installing this texture pack? {} file(s) will be processed. Nothing is written until you confirm.",
            plan.entries.len()
        ));
        for entry in plan.entries.iter().take(5) {
            if let Ok(relative) = entry.destination_relative_path.to_path_buf() {
                widgets::path_value(ui, "Destination", &destination_root.join(relative));
            }
        }
        if plan.entries.len() > 5 {
            ui.label(format!("…and {} more file(s)", plan.entries.len() - 5));
        }
        ui.horizontal(|ui| {
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                cancel = true;
            }
            if widgets::action_button(ui, "Confirm install", widgets::ActionStyle::Primary, true)
                .clicked()
            {
                confirm = true;
            }
        });
    });
    if cancel {
        state.stage = None;
    } else if confirm {
        state.stage = Some(spawn_pack_apply(plan, destination_root.to_path_buf()));
    } else {
        state.stage = Some(DolphinTextureModStage::PackConfirmationPending { plan });
    }
}

fn spawn_pack_apply(
    plan: SharedTransactionPlan,
    destination_root: PathBuf,
) -> DolphinTextureModStage {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<DolphinTexturePackApplyResult, String> {
            let history_root = default_shared_history_root().map_err(|error| error.detail)?;
            let backup_root = default_shared_backup_root().map_err(|error| error.detail)?;
            let options = SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: plan.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: now_unix_seconds(),
                current_context: plan.context.clone(),
                history_root,
                backup_root,
            };
            Ok(execute_dolphin_texture_pack_apply(&plan, &options))
        })();
        if let Ok(result) = result {
            let _ = sender.send(result);
        }
    });
    DolphinTextureModStage::PackApplying {
        receiver,
        destination_root,
    }
}

fn show_preview(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    plan: &DolphinTextureModPlan,
    source_parent: &Path,
    profile_id: &str,
) {
    let report = plan.report();
    let entry = report.entries.first();

    widgets::card(ui, |ui| {
        if let Some(entry) = entry {
            if let Some(source) = &entry.source_path {
                widgets::path_value(ui, "Source", source);
            }
            if let Some(destination) = &entry.destination_path {
                widgets::path_value(ui, "Destination", destination);
            }
            ui.label(format!("Status: {:?}", entry.destination_state));
        }

        match plan {
            DolphinTextureModPlan::Install { .. } => {
                widgets::status_badge(ui, "Eligible to install", widgets::StatusTone::Success);
                if widgets::action_button(ui, "Install", widgets::ActionStyle::Primary, true)
                    .clicked()
                {
                    match build_shared_transaction_plan(
                        report,
                        profile_id,
                        DOLPHIN_TEXTURE_MOD_SOURCE_MODE,
                        source_parent,
                    ) {
                        Ok(plan) => {
                            state.stage =
                                Some(DolphinTextureModStage::ConfirmationPending { plan });
                        }
                        Err(error) => {
                            state.stage = Some(DolphinTextureModStage::Failed {
                                detail: error.detail,
                            });
                        }
                    }
                }
            }
            DolphinTextureModPlan::AlreadyInstalled { .. } => {
                widgets::status_badge(ui, "Already installed", widgets::StatusTone::Info);
            }
            DolphinTextureModPlan::Conflict { .. } => {
                widgets::banner(
                    ui,
                    "Different file already installed",
                    "A different file already exists at this exact destination. It is never \
                     automatically replaced - remove or rename it yourself first if you want to \
                     install this texture here.",
                    widgets::StatusTone::Blocked,
                );
            }
            DolphinTextureModPlan::Blocked { .. } => {
                let blockers: Vec<String> = entry
                    .map(|entry| {
                        entry
                            .blockers
                            .iter()
                            .map(|blocker| format!("{:?}", blocker.kind))
                            .collect()
                    })
                    .unwrap_or_default();
                widgets::banner(
                    ui,
                    "Cannot install here",
                    &blockers.join(", "),
                    widgets::StatusTone::Blocked,
                );
            }
        }

        if widgets::action_button(
            ui,
            "Choose a different PNG",
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
        {
            state.stage = None;
        }
    });
}

fn show_confirmation(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    plan: SharedTransactionPlan,
    destination_root: &Path,
) {
    let mut cancel = false;
    let mut confirm = false;
    widgets::card(ui, |ui| {
        ui.label("Confirm installing this texture? Nothing is written until you confirm.");
        if let Some(first) = plan.entries.first()
            && let Ok(relative) = first.destination_relative_path.to_path_buf()
        {
            widgets::path_value(ui, "Will install to", &destination_root.join(relative));
        }
        ui.horizontal(|ui| {
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                cancel = true;
            }
            if widgets::action_button(ui, "Confirm install", widgets::ActionStyle::Primary, true)
                .clicked()
            {
                confirm = true;
            }
        });
    });

    if cancel {
        state.stage = None;
        return;
    }
    if confirm {
        let destination_root = destination_root.to_path_buf();
        state.stage = Some(spawn_apply(plan, destination_root));
    } else {
        state.stage = Some(DolphinTextureModStage::ConfirmationPending { plan });
    }
}

fn spawn_apply(plan: SharedTransactionPlan, destination_root: PathBuf) -> DolphinTextureModStage {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<SharedApplyResult, String> {
            let history_root = default_shared_history_root().map_err(|error| error.detail)?;
            let backup_root = default_shared_backup_root().map_err(|error| error.detail)?;
            let options = SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: plan.plan_id.clone(),
                    general_approved: true,
                    // A different existing file is never automatically
                    // replaced by this feature - see the module doc
                    // comment on `DolphinTextureModPlan::Conflict`. Only
                    // `Install`-classified plans ever reach here.
                    replacement_approved: false,
                }),
                operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: now_unix_seconds(),
                current_context: plan.context.clone(),
                history_root,
                backup_root,
            };
            Ok(execute_shared_apply(&plan, &options))
        })();
        let _ = sender.send(match result {
            Ok(result) => result,
            Err(_) => {
                // `SharedApplyResult` has no error-only constructor reachable
                // here; a history/backup root failure is exceedingly rare
                // (it only fails when the database path itself cannot be
                // resolved) and is surfaced via the disconnected-channel
                // path in `poll` instead of a fabricated result.
                return;
            }
        });
    });
    DolphinTextureModStage::Applying {
        receiver,
        destination_root,
    }
}

fn show_applied(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    result: SharedApplyResult,
    destination_root: PathBuf,
) {
    let mut undo_clicked = false;
    widgets::card(ui, |ui| {
        let (headline, tone) = match result.journal.status {
            SharedApplyStatus::Success => ("Installed", widgets::StatusTone::Success),
            _ => (
                "Install did not fully succeed",
                widgets::StatusTone::Blocked,
            ),
        };
        widgets::status_badge(ui, headline, tone);
        if let Some(path) = &result.journal_path {
            widgets::path_value(ui, "Journal", path);
        }
        let can_undo = result.journal_path.is_some()
            && result
                .journal
                .entries
                .first()
                .is_some_and(|entry| entry.outcome == SharedApplyOutcome::InstalledNew);
        if can_undo
            && widgets::action_button(ui, "Undo", widgets::ActionStyle::Destructive, true).clicked()
        {
            undo_clicked = true;
        }
    });

    if undo_clicked && let Some(journal_path) = result.journal_path.clone() {
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                state.stage = Some(DolphinTextureModStage::Failed {
                    detail: error.detail,
                });
                return;
            }
        };
        let preview = preview_shared_rollback(&journal_path, &destination_root, &backup_root);
        state.stage = Some(DolphinTextureModStage::RollbackPreview { preview });
        return;
    }
    state.stage = Some(DolphinTextureModStage::Applied {
        result,
        destination_root,
    });
}

fn show_rollback_preview(
    ui: &mut egui::Ui,
    state: &mut DolphinTextureModPageState,
    preview: SharedRollbackPreview,
) {
    let mut cancel = false;
    let mut confirm = false;
    widgets::card(ui, |ui| {
        ui.label(if preview.available {
            "Undo this install? This removes the installed texture file."
        } else {
            "This install can no longer be safely undone (it may already have changed)."
        });
        ui.horizontal(|ui| {
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                cancel = true;
            }
            if widgets::action_button(
                ui,
                "Confirm undo",
                widgets::ActionStyle::Destructive,
                preview.available,
            )
            .clicked()
            {
                confirm = true;
            }
        });
    });

    if cancel {
        state.stage = None;
        return;
    }
    if confirm {
        let history_root = match default_shared_history_root() {
            Ok(root) => root,
            Err(error) => {
                state.stage = Some(DolphinTextureModStage::Failed {
                    detail: error.detail,
                });
                return;
            }
        };
        let backup_root = match default_shared_backup_root() {
            Ok(root) => root,
            Err(error) => {
                state.stage = Some(DolphinTextureModStage::Failed {
                    detail: error.detail,
                });
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        let options = SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: now_unix_seconds(),
            history_root,
            backup_root,
        };
        std::thread::spawn(move || {
            let result = execute_shared_rollback(&preview, &options);
            let _ = sender.send(result);
        });
        state.stage = Some(DolphinTextureModStage::RollingBack { receiver });
    } else {
        state.stage = Some(DolphinTextureModStage::RollbackPreview { preview });
    }
}

#[cfg(test)]
mod tests;
