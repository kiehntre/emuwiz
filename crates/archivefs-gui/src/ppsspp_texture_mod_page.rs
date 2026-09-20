//! PPSSPP texture replacement panel embedded in Cheats & Mods.

use crate::ui::components as widgets;
use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::patch_manager::{
    PpssppProfile, PpssppTexturePackBuildRequest, PpssppTexturePackPlan, SharedApplyConfirmation,
    SharedApplyOptions, SharedApplyResult, SharedApplyStatus, SharedRollbackConfirmation,
    SharedRollbackOptions, SharedRollbackPreview, build_ppsspp_texture_pack_manifest,
    build_ppsspp_texture_pack_preview, build_ppsspp_texture_pack_transaction_plan,
    default_shared_backup_root, default_shared_history_root, execute_ppsspp_texture_pack_apply,
    execute_shared_rollback, generate_shared_operation_id, ppsspp_texture_destination_root,
    preview_shared_rollback, verified_ppsspp_texture_identity,
};
use eframe::egui;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct PpssppTextureModPageState {
    source_root: Option<PathBuf>,
    plan: Option<PpssppTexturePackPlan>,
    transaction: Option<archivefs_core::patch_manager::SharedTransactionPlan>,
    applied: Option<SharedApplyResult>,
    rollback_preview: Option<SharedRollbackPreview>,
    error: Option<String>,
}

impl PpssppTextureModPageState {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
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
            state.applied.is_none(),
        )
        .clicked()
        {
            state.source_root = rfd::FileDialog::new().pick_folder();
            state.plan = None;
            state.transaction = None;
            state.error = None;
        }
        if state.source_root.is_some()
            && state.plan.is_none()
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
                    plan.is_applyable(),
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
            let history = default_shared_history_root();
            let backups = default_shared_backup_root();
            if let (Ok(history_root), Ok(backup_root)) = (history, backups) {
                let result = execute_ppsspp_texture_pack_apply(
                    &plan,
                    &SharedApplyOptions {
                        dry_run: false,
                        confirmation: Some(SharedApplyConfirmation {
                            plan_id: plan.plan_id.clone(),
                            general_approved: true,
                            replacement_approved: true,
                        }),
                        operation_id: generate_shared_operation_id(),
                        timestamp_unix_seconds: 1_700_000_000,
                        current_context: plan.context.clone(),
                        history_root,
                        backup_root,
                    },
                );
                if result.apply.journal.status == SharedApplyStatus::Success {
                    state.applied = Some(result.apply);
                } else {
                    state.error = Some(format!(
                        "Install did not complete: {:?}",
                        result.apply.journal.status
                    ));
                }
            }
        }
        if let Some(result) = &state.applied {
            ui.label("Installed transactionally; provenance and rollback journal retained.");
            if let Some(path) = &result.journal_path
                && state.rollback_preview.is_none()
                && let Ok(backups) = default_shared_backup_root()
            {
                state.rollback_preview = Some(preview_shared_rollback(path, &destination, &backups));
            }
        }
        if let Some(preview) = state.rollback_preview.take() {
            if preview.available
                && widgets::action_button(ui, "Undo", widgets::ActionStyle::Secondary, true)
                    .clicked()
            {
                if let (Ok(history), Ok(backups)) =
                    (default_shared_history_root(), default_shared_backup_root())
                {
                    let _ = execute_shared_rollback(
                        &preview,
                        &SharedRollbackOptions {
                            confirmation: SharedRollbackConfirmation {
                                preview_id: preview.preview_id.clone(),
                                approved: true,
                            },
                            rollback_operation_id: generate_shared_operation_id(),
                            timestamp_unix_seconds: 1_700_000_001,
                            history_root: history,
                            backup_root: backups,
                        },
                    );
                    state.applied = None;
                }
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
