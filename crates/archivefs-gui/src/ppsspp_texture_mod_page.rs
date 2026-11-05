//! PPSSPP texture replacement panel embedded in Cheats & Mods.

use crate::ui::components as widgets;
use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::patch_manager::{
    PpssppProfile, PpssppTexturePackApplyResult, PpssppTexturePackBuildRequest,
    PpssppTexturePackPlan, SharedApplyConfirmation, SharedApplyOptions, SharedApplyResult,
    SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions, SharedRollbackPreview,
    SharedRollbackResult, build_ppsspp_texture_pack_manifest, build_ppsspp_texture_pack_preview,
    build_ppsspp_texture_pack_transaction_plan, default_shared_backup_root,
    default_shared_history_root, execute_ppsspp_texture_pack_apply, execute_shared_rollback,
    generate_shared_operation_id, ppsspp_texture_destination_root, preview_shared_rollback,
    verified_ppsspp_texture_identity,
};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a PPSSPP profile discovery is reused, so the filesystem probe
/// does not run on every rendered frame.
const PROFILE_DISCOVERY_TTL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct PpssppTextureModPageState {
    source_root: Option<PathBuf>,
    plan: Option<PpssppTexturePackPlan>,
    transaction: Option<archivefs_core::patch_manager::SharedTransactionPlan>,
    applied: Option<SharedApplyResult>,
    rollback_preview: Option<SharedRollbackPreview>,
    error: Option<String>,
    profile_cache: Option<(Instant, Option<PpssppProfile>)>,
    /// The one background job (folder picker, install or Undo) in flight.
    worker: Option<Receiver<Work>>,
}

/// Result of the single in-flight background job.
enum Work {
    Picked(Option<PathBuf>),
    Installed(Result<Box<PpssppTexturePackApplyResult>, String>),
    RolledBack(Result<SharedRollbackResult, String>),
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl PpssppTextureModPageState {
    /// The first eligible PPSSPP profile, re-discovered at most once per
    /// [`PROFILE_DISCOVERY_TTL`].
    pub(crate) fn eligible_profile(
        &mut self,
        now: Instant,
        discover: impl FnOnce() -> Option<PpssppProfile>,
    ) -> Option<PpssppProfile> {
        if let Some((at, profile)) = &self.profile_cache
            && now.saturating_duration_since(*at) < PROFILE_DISCOVERY_TTL
        {
            return profile.clone();
        }
        let profile = discover();
        self.profile_cache = Some((now, profile.clone()));
        profile
    }

    fn finish_rollback(&mut self, result: SharedRollbackResult) {
        self.rollback_preview = None; // Any retry needs a new filesystem review.
        if result.status == SharedApplyStatus::Success {
            self.applied = None;
            self.error = None;
        } else {
            self.error = Some(format!(
                "Undo did not complete: {:?}. The installed receipt was retained; review recovery before retrying.",
                result.status,
            ));
        }
    }

    pub(crate) fn reset(&mut self) {
        // A running job owns filesystem writes; never drop its receiver.
        if self.worker.is_none() {
            *self = Self::default();
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.worker.is_some()
    }

    /// Drains the background job. Returns whether anything changed.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(receiver) = self.worker.take() else {
            return false;
        };
        match receiver.try_recv() {
            Err(TryRecvError::Empty) => {
                self.worker = Some(receiver);
                false
            }
            Err(TryRecvError::Disconnected) => {
                self.error = Some("The texture pack worker stopped unexpectedly. Nothing is reported as installed; review History before retrying.".into());
                true
            }
            Ok(Work::Picked(path)) => {
                if let Some(path) = path {
                    self.source_root = Some(path);
                    self.plan = None;
                    self.transaction = None;
                    self.error = None;
                }
                true
            }
            Ok(Work::Installed(Err(detail))) => {
                self.error = Some(format!("Install did not start: {detail}"));
                true
            }
            Ok(Work::Installed(Ok(result))) => {
                if result.apply.journal.status == SharedApplyStatus::Success {
                    self.error = None;
                    self.applied = Some(result.apply);
                } else {
                    self.error = Some(incomplete_install_message(&result));
                }
                true
            }
            Ok(Work::RolledBack(Err(detail))) => {
                self.error = Some(format!(
                    "Undo did not start: {detail}. The installed receipt was retained."
                ));
                true
            }
            Ok(Work::RolledBack(Ok(result))) => {
                self.finish_rollback(result);
                true
            }
        }
    }
}

pub(crate) fn show_ppsspp_texture_mod_panel(
    ui: &mut egui::Ui,
    state: &mut PpssppTextureModPageState,
    archive_path: &Path,
    profile: &PpssppProfile,
    identity_report: Option<&GameIdentityReport>,
) {
    widgets::section_header(
        ui,
        "PPSSPP Texture Mods",
        Some("Choose a bounded texture pack, inspect the exact plan, then confirm installation."),
    );
    let Some(report) = identity_report else {
        widgets::card(ui, |ui| {
            ui.label("Load ROM Identity & Evidence first. Apply is disabled until PSP identity is verified.");
        });
        return;
    };
    let identity = match verified_ppsspp_texture_identity(report, archive_path) {
        Ok(v) => v,
        Err(e) => {
            state.reset();
            widgets::banner(
                ui,
                "Identity unavailable",
                &e.to_string(),
                widgets::StatusTone::Pending,
            );
            return;
        }
    };
    let destination = match ppsspp_texture_destination_root(profile) {
        Ok(v) => v,
        Err(e) => {
            widgets::banner(
                ui,
                "PPSSPP profile unavailable",
                &e.to_string(),
                widgets::StatusTone::Pending,
            );
            return;
        }
    };
    widgets::card(ui, |ui| {
        ui.label(format!("Verified Disc ID: {}", identity.disc_id));
        ui.label(format!("Destination root: {}", destination.display()));
        if let Some(source) = &state.source_root {
            ui.label(format!("Pack: {}", source.display()));
        }
        if widgets::action_button(
            ui,
            "Choose pack",
            widgets::ActionStyle::Primary,
            state.applied.is_none() && !state.is_busy(),
        )
        .clicked()
        {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(Work::Picked(rfd::FileDialog::new().pick_folder()));
            });
            state.worker = Some(receiver);
        }
        if state.source_root.is_some()
            && state.plan.is_none()
            && !state.is_busy()
            && widgets::action_button(ui, "Inspect", widgets::ActionStyle::Secondary, true)
                .clicked()
        {
            let source = state.source_root.clone().expect("checked");
            match build_ppsspp_texture_pack_manifest(&PpssppTexturePackBuildRequest {
                source_root: source.clone(),
                identity: identity.clone(),
                name: source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("PPSSPP texture pack")
                    .to_string(),
            }) {
                Ok(build) if build.complete => match build_ppsspp_texture_pack_preview(
                    &archivefs_core::patch_manager::PpssppTexturePackPreviewRequest {
                        selected_archive: archive_path.to_path_buf(),
                        identity: identity.clone(),
                        destination_root: destination.clone(),
                        source_root: source,
                        manifest: build.manifest,
                    },
                ) {
                    Ok(plan) => state.plan = Some(plan),
                    Err(e) => state.error = Some(e.to_string()),
                },
                Ok(build) => {
                    state.error = Some(format!(
                        "Pack inspection rejected entries: {}",
                        build
                            .rejected
                            .iter()
                            .map(|v| v.reason.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    ))
                }
                Err(e) => state.error = Some(e.to_string()),
            }
        }
        if let Some(plan) = &state.plan {
            ui.label(format!(
                "Preview: {} entries; {} create, {} replace, {} already installed",
                plan.report.entries.len(),
                plan.report
                    .entries
                    .iter()
                    .filter(|e| matches!(
                        e.state,
                        archivefs_core::patch_manager::PreviewState::InstallNew
                    ))
                    .count(),
                plan.report
                    .entries
                    .iter()
                    .filter(|e| matches!(
                        e.state,
                        archivefs_core::patch_manager::PreviewState::ReplaceDifferent
                    ))
                    .count(),
                plan.report
                    .entries
                    .iter()
                    .filter(|e| matches!(
                        e.state,
                        archivefs_core::patch_manager::PreviewState::AlreadyInstalled
                    ))
                    .count()
            ));
            if state.transaction.is_none()
                && widgets::action_button(
                    ui,
                    "Confirm and install",
                    widgets::ActionStyle::Primary,
                    plan.is_applyable() && !state.is_busy(),
                )
                .clicked()
            {
                state.transaction = build_ppsspp_texture_pack_transaction_plan(
                    plan,
                    &profile.profile_id,
                    state.source_root.as_deref().expect("plan source"),
                )
                .ok();
            }
        }
        if let Some(plan) = state.transaction.take() {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let result = (|| {
                    let history_root = default_shared_history_root().map_err(|e| e.detail)?;
                    let backup_root = default_shared_backup_root().map_err(|e| e.detail)?;
                    Ok(Box::new(execute_ppsspp_texture_pack_apply(
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
                    )))
                })();
                let _ = sender.send(Work::Installed(result));
            });
            state.worker = Some(receiver);
        }
        if state.is_busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working… keep this page open until the result is shown.");
            });
        }
        if let Some(result) = &state.applied {
            show_applied_summary(ui, result);
            if let Some(path) = &result.journal_path
                && state.rollback_preview.is_none()
                && !state.is_busy()
            {
                match default_shared_backup_root() {
                    Ok(backups) => {
                        state.rollback_preview =
                            Some(preview_shared_rollback(path, &destination, &backups));
                    }
                    Err(error) => {
                        state.error = Some(format!("Undo is unavailable: {}", error.detail));
                    }
                }
            }
        }
        if let Some(preview) = state.rollback_preview.take() {
            if preview.available
                && widgets::action_button(
                    ui,
                    "Undo",
                    widgets::ActionStyle::Secondary,
                    !state.is_busy(),
                )
                .clicked()
            {
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = (|| {
                        let history_root = default_shared_history_root().map_err(|e| e.detail)?;
                        let backup_root = default_shared_backup_root().map_err(|e| e.detail)?;
                        Ok(execute_shared_rollback(
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
                    let _ = sender.send(Work::RolledBack(result));
                });
                state.worker = Some(receiver);
            } else {
                state.rollback_preview = Some(preview);
            }
        }
        if let Some(error) = &state.error {
            widgets::banner(
                ui,
                "Texture pack unavailable",
                error,
                widgets::StatusTone::Warning,
            );
        }
    });
}

fn incomplete_install_message(
    result: &archivefs_core::patch_manager::PpssppTexturePackApplyResult,
) -> String {
    let mut message = format!(
        "Install did not complete: {:?}",
        result.apply.journal.status
    );
    crate::shared_apply_warnings::append_lines(
        &mut message,
        crate::shared_apply_warnings::lines_after_rollback(&result.apply, result.rollback.as_ref()),
    );
    message
}

fn show_applied_summary(ui: &mut egui::Ui, result: &SharedApplyResult) {
    ui.label("Installed transactionally; provenance and rollback journal retained.");
    crate::shared_apply_warnings::show(ui, &crate::shared_apply_warnings::lines(result));
}

#[cfg(test)]
mod tests;
